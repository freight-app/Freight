use std::process::Command;

use crane_core::build::{build_project, clean_project, test_project};

use crate::output::{print_error, print_success};

pub fn cmd_build(release: bool) {
    let profile = if release { "release" } else { "dev" };
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
        Err(e) => {
            println!();
            print_error(&e.to_string());
        }
    }
}

pub fn cmd_run(release: bool, run_args: &[String]) {
    let profile = if release { "release" } else { "dev" };
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
    match clean_project() {
        Ok(()) => print_success("cleaned target/"),
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

pub fn cmd_test(filter: Option<&str>) {
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
        Err(e) => {
            println!();
            print_error(&e.to_string());
        }
    }
}
