use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use freight_core::build::{
    bench_project_with, bench_workspace_with,
    build_project_at, build_project_with, build_workspace_with, clean_project, clean_workspace,
    test_project_with, test_workspace_with,
};
use freight_core::event::{BuildEvent, Progress};
use freight_core::manifest::{find_manifest_dir, load_workspace_manifest};

use crate::output::{print_error, print_status, print_success, print_warning};

// ── Progress ──────────────────────────────────────────────────────────────────

fn make_progress() -> Progress {
    use std::sync::Arc;
    use owo_colors::OwoColorize;
    Arc::new(|event| match event {
        BuildEvent::BuildStarted { name, profile } => {
            print_status("Building", &format!("{name} [{profile}]"));
        }
        BuildEvent::Compiling { path } => {
            print_status("Compiling", &path.display().to_string());
        }
        BuildEvent::Fresh { path } => {
            println!("{:>12} {}", "Fresh".dimmed(), path.display());
        }
        BuildEvent::Linking { name } => {
            print_status("Linking", &name);
        }
        BuildEvent::Archiving { name } => {
            print_status("Archiving", &name);
        }
        BuildEvent::RunningScript { cached } => {
            if cached {
                println!("{:>12} build script (cached)", "Running".dimmed());
            } else {
                print_status("Running", "build script");
            }
        }
        BuildEvent::FetchingDep { name, source } => {
            print_status("Fetching", &format!("{name} ({source})"));
        }
        BuildEvent::ResolvingDep { name, via } => {
            println!("{:>12} {} ({})", "Resolving".dimmed(), name, via);
        }
        BuildEvent::BuildingForeignDep { name, backend } => {
            print_status("Building", &format!("{name} ({backend})"));
        }
        BuildEvent::Warning(msg) => {
            print_warning(&msg);
        }
        BuildEvent::TestLinking { name } => {
            print_status("Linking", &name);
        }
        BuildEvent::TestRunning { name } => {
            print_status("Running", &name);
        }
        BuildEvent::TestResult { name, passed } => {
            if passed {
                println!("{:>12} {} ... ok", "test".bold(), name);
            } else {
                println!("{:>12} {} ... FAILED", "test".bold().red(), name);
            }
        }
        BuildEvent::BenchLinking { name } => {
            print_status("Linking", &name);
        }
        BuildEvent::BenchRunning { name } => {
            print_status("Benchmarking", &name);
        }
        BuildEvent::BenchResult { name, mean_ns } => {
            println!("{:>12} {} … {}", "bench".bold().cyan(), name, fmt_duration(mean_ns));
        }
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return true when the nearest `freight.toml` (found by walking up from cwd)
/// has a `[workspace]` section. Falls through to the regular project path on
/// any I/O or parse error.
fn at_workspace_root() -> bool {
    let Ok(cwd) = env::current_dir() else { return false };
    let Some(dir) = find_manifest_dir(&cwd) else { return false };
    load_workspace_manifest(&dir).is_some()
}

// ── Commands ──────────────────────────────────────────────────────────────────

pub fn cmd_build(release: bool, package: Option<&str>, features: &[String], use_defaults: bool, sanitize: &[String]) {
    let profile = if release { "release" } else { "dev" };

    let progress = make_progress();
    if at_workspace_root() {
        match build_workspace_with(profile, package, features, use_defaults, &progress) {
            Ok(outputs) => {
                println!();
                for o in &outputs {
                    print_success(&format!(
                        "{} ({} compiled, {} up to date)",
                        o.package_name, o.compiled, o.skipped,
                    ));
                    for bin in &o.binaries {
                        println!("    {}", bin.display());
                    }
                }
            }
            Err(e) => { println!(); print_error(&e.to_string()); }
        }
        return;
    }

    if package.is_some() {
        print_error("`-p` can only be used at a workspace root");
        return;
    }

    match build_project_with(profile, features, use_defaults, sanitize, &progress) {
        Ok(output) => {
            println!();
            print_success(&format!(
                "{} ({} compiled, {} up to date)",
                output.package_name, output.compiled, output.skipped,
            ));
            for bin in &output.binaries {
                println!("    {}", bin.display());
            }
        }
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

pub fn cmd_run(release: bool, package: Option<&str>, bin: Option<&str>, features: &[String], use_defaults: bool, run_args: &[String], sanitize: &[String]) {
    let profile = if release { "release" } else { "dev" };

    if at_workspace_root() {
        let Some(pkg) = package else {
            print_error("`freight run` is not supported at workspace root — use `-p <package>` to select a member");
            return;
        };
        let member_dir = match find_workspace_member_dir(pkg) {
            Some(d) => d,
            None => { print_error(&format!("package `{pkg}` not found in workspace")); return; }
        };
        let output = match build_project_at(&member_dir, profile, features, use_defaults, None, sanitize, &make_progress()) {
            Ok(o) => o,
            Err(e) => { println!(); print_error(&e.to_string()); return; }
        };
        run_binary(output, bin, run_args);
        return;
    }

    if package.is_some() {
        print_error("`-p` can only be used at a workspace root");
        return;
    }

    let output = match build_project_with(profile, features, use_defaults, sanitize, &make_progress()) {
        Ok(o) => o,
        Err(e) => { println!(); print_error(&e.to_string()); return; }
    };

    run_binary(output, bin, run_args);
}

fn run_binary(output: freight_core::build::BuildOutput, bin: Option<&str>, run_args: &[String]) {
    let candidate: Option<std::path::PathBuf> = match bin {
        Some(name) => {
            let matched: Vec<_> = output.binaries.iter()
                .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
                .cloned()
                .collect();
            match matched.as_slice() {
                [b] => Some(b.clone()),
                [] => {
                    print_error(&format!("no binary named {name:?} — available: {}",
                        output.binaries.iter()
                            .filter_map(|p| p.file_name()?.to_str())
                            .collect::<Vec<_>>().join(", ")
                    ));
                    return;
                }
                _ => Some(matched[0].clone()),
            }
        }
        None => match output.binaries.as_slice() {
            [] => {
                print_error("no binary target produced — add a [[bin]] section to freight.toml");
                return;
            }
            [b] => Some(b.clone()),
            _ => {
                print_error(&format!(
                    "multiple [[bin]] targets — use --bin <name> to select one: {}",
                    output.binaries.iter()
                        .filter_map(|p| p.file_name()?.to_str())
                        .collect::<Vec<_>>().join(", ")
                ));
                return;
            }
        },
    };

    if let Some(bin_path) = candidate {
        println!();
        use owo_colors::OwoColorize;
        println!("    {} {}", "Running".bold().green(), bin_path.display());
        println!();
        let status = Command::new(&bin_path).args(run_args).status();
        match status {
            Ok(s) if !s.success() => {
                if let Some(code) = s.code() {
                    print_error(&format!("process exited with code {code}"));
                }
            }
            Err(e) => print_error(&format!("failed to run binary: {e}")),
            Ok(_) => {}
        }
    }
}

fn find_workspace_member_dir(pkg: &str) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let ws_dir = find_manifest_dir(&cwd)?;
    let ws = load_workspace_manifest(&ws_dir)?;
    ws.members.iter().find_map(|m| {
        let dir = ws_dir.join(m.trim_end_matches('/'));
        if dir.file_name().and_then(|n| n.to_str()) == Some(pkg) { Some(dir) } else { None }
    })
}

pub fn cmd_clean() {
    if at_workspace_root() {
        match clean_workspace() {
            Ok(()) => print_success("cleaned all workspace member target/ directories"),
            Err(e) => { println!(); print_error(&e.to_string()); }
        }
        return;
    }

    match clean_project() {
        Ok(()) => print_success("cleaned target/"),
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

pub fn cmd_watch(release: bool) {
    let profile = if release { "release" } else { "dev" };

    let Ok(cwd) = env::current_dir() else {
        print_error("cannot read working directory");
        return;
    };
    let Some(project_dir) = find_manifest_dir(&cwd) else {
        print_error("no freight.toml found");
        return;
    };

    // Collect paths to watch.
    let mut watch_paths: Vec<PathBuf> = Vec::new();
    let src_dir = project_dir.join("src");
    if src_dir.exists() { watch_paths.push(src_dir); }
    let manifest = project_dir.join("freight.toml");
    if manifest.exists() { watch_paths.push(manifest); }
    let script = project_dir.join("build.freight");
    if script.exists() { watch_paths.push(script); }
    let include_dir = project_dir.join("include");
    if include_dir.exists() { watch_paths.push(include_dir); }

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match RecommendedWatcher::new(tx, notify::Config::default()) {
        Ok(w) => w,
        Err(e) => { print_error(&format!("failed to initialise file watcher: {e}")); return; }
    };

    for path in &watch_paths {
        if let Err(e) = watcher.watch(path, RecursiveMode::Recursive) {
            print_error(&format!("cannot watch {}: {e}", path.display()));
            return;
        }
    }

    use owo_colors::OwoColorize;
    println!("  {} source files — press Ctrl+C to stop", "Watching".bold().cyan());

    // Initial build.
    run_build(profile, &project_dir);

    // Debounce: collect events for 200 ms then rebuild once.
    let debounce = Duration::from_millis(200);
    loop {
        // Block until the first event arrives.
        match rx.recv() {
            Err(_) => break, // watcher dropped
            Ok(Err(e)) => { print_error(&format!("watch error: {e}")); continue; }
            Ok(Ok(ev)) => {
                if !is_relevant(&ev) { continue; }
            }
        }
        // Drain further events that arrive within the debounce window.
        loop {
            match rx.recv_timeout(debounce) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        run_build(profile, &project_dir);
    }
}

fn is_relevant(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn run_build(profile: &str, project_dir: &std::path::Path) {
    use owo_colors::OwoColorize;
    println!("\n  {} …", "Rebuilding".bold().cyan());
    match build_project_with(profile, &[], true, &[], &make_progress()) {
        Ok(output) => {
            println!();
            print_success(&format!(
                "{} ({} compiled, {} up to date)",
                output.package_name, output.compiled, output.skipped,
            ));
            let _ = project_dir; // used via build_project's cwd detection
        }
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

pub fn cmd_test(filter: Option<&str>, release: bool, package: Option<&str>, features: &[String], use_defaults: bool, sanitize: &[String]) {
    let profile = if release { "release" } else { "dev" };
    let progress = make_progress();
    if at_workspace_root() {
        match test_workspace_with(profile, filter, package, features, use_defaults, &progress) {
            Ok(summary) => {
                println!();
                if summary.total == 0 {
                    println!("no test files found in any workspace member");
                    return;
                }
                if summary.failed == 0 {
                    print_success(&format!(
                        "test result: ok. {} passed; 0 failed", summary.passed,
                    ));
                } else {
                    print_error(&format!(
                        "test result: FAILED. {} passed; {} failed",
                        summary.passed, summary.failed,
                    ));
                }
            }
            Err(e) => { println!(); print_error(&e.to_string()); }
        }
        return;
    }

    if package.is_some() {
        print_error("`-p` can only be used at a workspace root");
        return;
    }

    match test_project_with(profile, filter, features, use_defaults, sanitize, &progress) {
        Ok(summary) => {
            println!();
            if summary.total == 0 {
                println!("no test files found under tests/");
                return;
            }
            if summary.failed == 0 {
                print_success(&format!(
                    "test result: ok. {} passed; 0 failed", summary.passed,
                ));
            } else {
                print_error(&format!(
                    "test result: FAILED. {} passed; {} failed",
                    summary.passed, summary.failed,
                ));
            }
        }
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

pub fn cmd_bench(filter: Option<&str>, package: Option<&str>, features: &[String], use_defaults: bool) {
    let progress = make_progress();
    if at_workspace_root() {
        match bench_workspace_with(filter, package, features, use_defaults, &progress) {
            Ok(summary) => {
                println!();
                if summary.results.is_empty() {
                    println!("no bench files found in any workspace member");
                    return;
                }
                print_bench_table(&summary.results);
            }
            Err(e) => { println!(); print_error(&e.to_string()); }
        }
        return;
    }

    if package.is_some() {
        print_error("`-p` can only be used at a workspace root");
        return;
    }

    match bench_project_with(filter, features, use_defaults, &progress) {
        Ok(summary) => {
            println!();
            if summary.results.is_empty() {
                println!("no bench files found under benches/");
                return;
            }
            print_bench_table(&summary.results);
        }
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

fn print_bench_table(results: &[freight_core::build::BenchResult]) {
    use owo_colors::OwoColorize;
    let name_width = results.iter().map(|r| r.name.len()).max().unwrap_or(10).max(10);
    println!("{:>12}  {:<width$}  {:>12}  {:>12}  {:>12}  {}",
        "bench".bold().cyan(),
        "name", "mean", "min", "max", "runs",
        width = name_width,
    );
    println!("{}", "─".repeat(name_width + 52));
    for r in results {
        println!("{:>12}  {:<width$}  {:>12}  {:>12}  {:>12}  {}",
            "",
            r.name,
            fmt_duration(r.mean_ns),
            fmt_duration(r.min_ns),
            fmt_duration(r.max_ns),
            r.runs,
            width = name_width,
        );
    }
}

fn fmt_duration(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.3} s ", ns as f64 / 1_000_000_000.0)
    } else if ns >= 1_000_000 {
        format!("{:.3} ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.3} µs", ns as f64 / 1_000.0)
    } else {
        format!("{ns} ns")
    }
}
