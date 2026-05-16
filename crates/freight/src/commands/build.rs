use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use std::collections::{BTreeSet, HashMap};

use freight_core::build::{
    bench_project_with, bench_workspace_with,
    build_project_at, build_project_with, build_workspace_with, clean_project, clean_workspace,
    emit_asm_project_with, test_project_with, test_workspace_with,
    resolve_dep_graph, ResolvedDep,
};
use freight_core::event::{BuildEvent, Progress};
use freight_core::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};

use crate::output::{
    print_error, print_status, print_success, print_warning,
    GraphEdge, GraphCluster, GraphFormat, render_mermaid_graph, render_dot_graph,
};

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
        BuildEvent::Timing { .. } => {}   // collected separately; not printed inline
        BuildEvent::EmittedAsm { path } => {
            println!("{:>12} {}", "Emitted".dimmed(), path.display());
        }
    })
}

/// Like [`make_progress`] but also collects [`BuildEvent::Timing`] events into
/// the returned `Arc<Mutex<Vec<(PathBuf, u64)>>>` for post-build reporting.
fn make_timed_progress() -> (Progress, std::sync::Arc<std::sync::Mutex<Vec<(PathBuf, u64)>>>) {
    use std::sync::{Arc, Mutex};
    use owo_colors::OwoColorize;

    let timings: Arc<Mutex<Vec<(PathBuf, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let timings_sink = Arc::clone(&timings);

    let progress: Progress = Arc::new(move |event| match event {
        BuildEvent::Timing { ref path, ns } => {
            timings_sink.lock().unwrap().push((path.clone(), ns));
        }
        BuildEvent::BuildStarted { name, profile } => print_status("Building", &format!("{name} [{profile}]")),
        BuildEvent::Compiling { path } => print_status("Compiling", &path.display().to_string()),
        BuildEvent::Fresh { path } => println!("{:>12} {}", "Fresh".dimmed(), path.display()),
        BuildEvent::Linking { name } => print_status("Linking", &name),
        BuildEvent::Archiving { name } => print_status("Archiving", &name),
        BuildEvent::RunningScript { cached } => {
            if cached { println!("{:>12} build script (cached)", "Running".dimmed()); }
            else { print_status("Running", "build script"); }
        }
        BuildEvent::FetchingDep { name, source } => print_status("Fetching", &format!("{name} ({source})")),
        BuildEvent::ResolvingDep { name, via } => println!("{:>12} {} ({})", "Resolving".dimmed(), name, via),
        BuildEvent::BuildingForeignDep { name, backend } => print_status("Building", &format!("{name} ({backend})")),
        BuildEvent::Warning(msg) => print_warning(&msg),
        BuildEvent::EmittedAsm { path } => println!("{:>12} {}", "Emitted".dimmed(), path.display()),
        _ => {}
    });

    (progress, timings)
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

pub fn cmd_build(release: bool, package: Option<&str>, features: &[String], use_defaults: bool, sanitize: &[String], emit: &[String], time_passes: bool) {
    let profile = if release { "release" } else { "dev" };

    if time_passes {
        // Safety: single-threaded here; rayon workers not yet started.
        unsafe { std::env::set_var("FREIGHT_TIME_PASSES", "1"); }
    }

    let (progress, timings) = if time_passes {
        make_timed_progress()
    } else {
        (make_progress(), std::sync::Arc::new(std::sync::Mutex::new(vec![])))
    };

    let build_ok;
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
                build_ok = true;
            }
            Err(e) => { println!(); print_error(&e.to_string()); build_ok = false; }
        }
    } else {
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
                build_ok = true;
            }
            Err(e) => { println!(); print_error(&e.to_string()); build_ok = false; }
        }
    }

    if build_ok {
        if emit.iter().any(|e| e.eq_ignore_ascii_case("asm")) {
            if let Err(e) = emit_asm_project_with(profile, &progress) {
                print_error(&format!("--emit asm failed: {e}"));
            }
        }
        if time_passes {
            let mut t = timings.lock().unwrap();
            t.sort_by(|a, b| b.1.cmp(&a.1));
            print_timing_table(&t);
        }
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

// ── freight build --graph ──────────────────────────────────���──────────────────

pub fn cmd_build_graph(release: bool, _package: Option<&str>, features: &[String], _use_defaults: bool, format: &str) {
    use owo_colors::OwoColorize;

    let profile = if release { "release" } else { "dev" };

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no freight.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&format!("failed to load manifest: {e}")); return; }
    };

    let activated: BTreeSet<String> = features.iter().cloned().collect();

    let resolved = match resolve_dep_graph(&project_dir, &manifest, false, &activated) {
        Ok(r) => r,
        Err(e) => { print_error(&format!("dependency resolution failed: {e}")); return; }
    };

    // Assign a build stage to every resolved dep.
    // resolved is already in topological order (leaves first), so we can
    // compute stage[dep] = max(stage[freight_dep]) + 1 in a single pass.
    let mut stage_of: HashMap<String, usize> = HashMap::new();
    for dep in &resolved {
        // Stage = one above the highest stage of any freight dep this dep needs.
        let max_dep_stage = dep.manifest.dependencies.keys()
            .filter_map(|n| stage_of.get(n).copied())
            .max();
        stage_of.insert(dep.name.clone(), max_dep_stage.map_or(0, |s| s + 1));
    }

    // Group by stage.
    let max_stage = stage_of.values().copied().max().unwrap_or(0);
    let mut stages: Vec<Vec<&ResolvedDep>> = vec![vec![]; max_stage + 1];
    for dep in &resolved {
        let s = stage_of[&dep.name];
        stages[s].push(dep);
    }

    let fmt = GraphFormat::parse(format);

    if fmt != GraphFormat::Text {
        // Build clusters (one per stage) and edges.
        let mut clusters: Vec<GraphCluster> = Vec::new();
        let mut edges: Vec<GraphEdge> = Vec::new();

        for (stage_idx, stage_deps) in stages.iter().enumerate() {
            let label = format!("Stage {stage_idx}");
            let nodes = stage_deps.iter()
                .map(|d| format!("{}\n{}", d.name, d.manifest.package.version))
                .collect();
            clusters.push(GraphCluster { id: format!("stage{stage_idx}"), label, nodes });

            for dep in stage_deps.iter() {
                for needed in dep.manifest.dependencies.keys() {
                    if stage_of.contains_key(needed) {
                        edges.push(GraphEdge {
                            from: format!("{}\n{}", needed, resolved.iter().find(|r| &r.name == needed).map_or("", |r| &r.manifest.package.version)),
                            to:   format!("{}\n{}", dep.name, dep.manifest.package.version),
                        });
                    }
                }
            }
        }

        // Root project node and its edges.
        let root_node = format!("{}\n{}", manifest.package.name, manifest.package.version);
        let mut root_needs: Vec<String> = Vec::new();
        for dep in &resolved {
            if manifest.dependencies.contains_key(&dep.name) {
                edges.push(GraphEdge {
                    from: format!("{}\n{}", dep.name, dep.manifest.package.version),
                    to:   root_node.clone(),
                });
                root_needs.push(dep.name.clone());
            }
        }

        // Link node.
        let bin_names: Vec<String> = if manifest.bins.is_empty() {
            vec![manifest.package.name.clone()]
        } else {
            manifest.bins.iter().map(|b| b.name.clone()).collect()
        };
        let link_node = format!("link: {}", bin_names.join(", "));
        edges.push(GraphEdge { from: root_node.clone(), to: link_node.clone() });

        let ungrouped = vec![root_node, link_node];
        let title = format!("{} build graph [{}]", manifest.package.name, profile);
        match fmt {
            GraphFormat::Mermaid => render_mermaid_graph(&title, &clusters, &edges, &ungrouped),
            GraphFormat::Dot     => render_dot_graph(&title, &clusters, &edges, &ungrouped),
            GraphFormat::Text    => unreachable!(),
        }
        return;
    }

    // Print header.
    println!(
        "{} {}  {}",
        manifest.package.name.bold().bright_blue(),
        manifest.package.version.bright_black(),
        format!("[{profile}]").bright_black()
    );

    let rule = "─".repeat(48).bright_black().to_string();

    // Print each dep stage.
    for (stage_idx, stage_deps) in stages.iter().enumerate() {
        if stage_deps.is_empty() { continue; }

        println!();
        let needs: Vec<String> = stage_deps.iter()
            .flat_map(|d| d.manifest.dependencies.keys()
                .filter(|n| stage_of.get(*n).copied().unwrap_or(usize::MAX) < stage_idx)
                .cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter().collect();

        let label = if needs.is_empty() {
            format!("Stage {stage_idx}  (parallel)")
        } else {
            format!("Stage {stage_idx}  (parallel · needs: {})", needs.join(", "))
        };
        println!("{rule}");
        println!("{}", label.bold());

        for (di, dep) in stage_deps.iter().enumerate() {
            let is_last_dep = di == stage_deps.len() - 1;
            let dep_conn  = if is_last_dep { "└── " } else { "├── " };
            let src_prefix = if is_last_dep { "    " } else { "│   " };

            let origin = dep.dir.strip_prefix(&project_dir)
                .map(|p| format!("({})", p.display()))
                .unwrap_or_else(|_| format!("({})", dep.dir.display()));

            println!(
                "{}{}  {}  {}",
                dep_conn.bright_black(),
                dep.name.bold().bright_blue(),
                dep.manifest.package.version.bright_black(),
                origin.yellow()
            );

            // Collect and print source files for this dep.
            let src_dir = dep.dir.join("src");
            let srcs = collect_graph_sources(&src_dir);
            for (si, src) in srcs.iter().enumerate() {
                let is_last_src = si == srcs.len() - 1;
                let src_conn = if is_last_src { "└── " } else { "├── " };
                let rel = src.strip_prefix(&dep.dir).unwrap_or(src);
                println!("{}{}{}", src_prefix.bright_black(), src_conn.bright_black(), rel.display().to_string().bright_black());
            }
        }
    }

    // Root project sources (final compile stage).
    let root_src_dir = project_dir.join("src");
    let root_srcs = collect_graph_sources(&root_src_dir);
    if !root_srcs.is_empty() {
        println!();
        println!("{rule}");
        println!("{}", format!("Stage {}  (root)", max_stage + 1).bold());
        for (i, src) in root_srcs.iter().enumerate() {
            let is_last = i == root_srcs.len() - 1;
            let conn = if is_last { "└── " } else { "├── " };
            let rel = src.strip_prefix(&project_dir).unwrap_or(src);
            println!("{}{}", conn.bright_black(), rel.display());
        }
    }

    // Link step.
    println!();
    println!("{rule}");
    println!("{}", "Link".bold());
    let target_dir = project_dir.join("target").join(profile);

    // Binaries from [[bin]] targets.
    let bins: Vec<String> = manifest.bins.iter()
        .map(|b| b.name.clone())
        .collect();
    let bin_names = if bins.is_empty() {
        vec![manifest.package.name.clone()]
    } else {
        bins
    };

    for bin in &bin_names {
        let exe = target_dir.join(bin);
        println!("└── {}", exe.display().to_string().bright_blue().bold());
    }

    // List dep libs.
    if !resolved.is_empty() {
        let libs: Vec<String> = resolved.iter()
            .map(|d| format!("lib{}.a", d.name))
            .collect();
        println!("    {}", libs.join("  ").bright_black());
    }

    println!("{rule}");
}

fn collect_graph_sources(src_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    const SOURCE_EXTS: &[&str] = &["c", "cc", "cpp", "cxx", "c++", "cu", "hip", "m", "mm"];
    let mut files = Vec::new();
    collect_graph_sources_rec(src_dir, SOURCE_EXTS, &mut files);
    files.sort();
    files
}

fn collect_graph_sources_rec(dir: &std::path::Path, exts: &[&str], out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_graph_sources_rec(&path, exts, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.contains(&ext) {
                out.push(path);
            }
        }
    }
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

fn print_timing_table(timings: &[(PathBuf, u64)]) {
    use owo_colors::OwoColorize;
    if timings.is_empty() { return; }
    println!();
    let name_width = timings.iter()
        .map(|(p, _)| p.display().to_string().len())
        .max().unwrap_or(20).max(20).min(60);
    println!("{:>12}  {:<width$}  {:>10}", "time-passes".bold().yellow(), "file", "time", width = name_width);
    println!("{}", "─".repeat(name_width + 26));
    for (path, ns) in timings {
        println!("{:>12}  {:<width$}  {:>10}",
            "",
            truncate_left(&path.display().to_string(), name_width),
            fmt_duration(*ns),
            width = name_width,
        );
    }
}

fn truncate_left(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() }
    else { format!("…{}", &s[s.len() - max + 1..]) }
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
