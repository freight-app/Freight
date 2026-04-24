use std::env;
use std::process::Command;

use crane_core::build::{
    build_project, build_workspace, clean_project, clean_workspace, test_project, test_workspace,
};
use crane_core::manifest::{find_manifest_dir, load_workspace_manifest};

use crate::output::{print_error, print_success};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return true when the nearest `crane.toml` (found by walking up from cwd)
/// has a `[workspace]` section. Falls through to the regular project path on
/// any I/O or parse error.
fn at_workspace_root() -> bool {
    let Ok(cwd) = env::current_dir() else { return false };
    let Some(dir) = find_manifest_dir(&cwd) else { return false };
    load_workspace_manifest(&dir).is_some()
}

// ── Commands ──────────────────────────────────────────────────────────────────

pub fn cmd_build(release: bool) {
    let profile = if release { "release" } else { "dev" };

    if at_workspace_root() {
        match build_workspace(profile) {
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

    match build_project(profile) {
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

pub fn cmd_run(release: bool, run_args: &[String]) {
    let profile = if release { "release" } else { "dev" };

    if at_workspace_root() {
        print_error("`crane run` is not supported at workspace root — cd into a member directory");
        return;
    }

    let output = match build_project(profile) {
        Ok(o) => o,
        Err(e) => { println!(); print_error(&e.to_string()); return; }
    };

    match output.binaries.as_slice() {
        [] => {
            print_error("no binary target produced — add a [[bin]] section to crane.toml");
        }
        [bin] => {
            println!();
            use owo_colors::OwoColorize;
            println!("    {} {}", "Running".bold().green(), bin.display());
            println!();
            let status = Command::new(bin).args(run_args).status();
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
        _ => {
            print_error("multiple [[bin]] targets — specify which to run (not yet supported)");
            for b in &output.binaries {
                eprintln!("  {}", b.display());
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

pub fn cmd_test(filter: Option<&str>) {
    if at_workspace_root() {
        match test_workspace("dev", filter) {
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

    match test_project("dev", filter) {
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
