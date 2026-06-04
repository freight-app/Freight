//! `freight lint` — run the project linter over all source files.

use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// Apply auto-fixes where possible
    #[arg(long)]
    pub fix: bool,
}

impl Args {
    pub fn run(self) {
        cmd_lint(self.fix);
    }
}

use freight::manifest::types::Manifest;
use freight::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};
use freight::toolchain::{
    collect_sources, detect_tools, load_linter_templates, select_linter, DetectedTool,
};

use crate::output::{print_error, print_success, print_warning};

fn print_settings_ref(tool: &DetectedTool) {
    use owo_colors::OwoColorize;
    let t = &tool.template;
    if t.settings.is_empty() {
        return;
    }
    println!(
        "  {} [linter] settings for {}:",
        "hint:".dimmed(),
        t.name.bold()
    );
    let mut keys: Vec<&String> = t.settings.keys().collect();
    keys.sort();
    for key in keys {
        if let Some(vals) = t.values.get(key) {
            println!("    {key} = \"…\"  ({})", vals.join(" | ").dimmed());
        } else {
            println!("    {key} = \"…\"");
        }
    }
}

/// Lint a single project. Returns `true` on success.
fn lint_project(project_dir: &Path, manifest: &Manifest, fix: bool) -> bool {
    let templates = load_linter_templates();
    let detected = detect_tools(&templates);
    let linter = match select_linter(&detected, &manifest.linter) {
        Some(l) => l,
        None => {
            if let Some(name) = &manifest.linter.name {
                print_error(&format!("linter '{name}' not found on PATH"));
            } else {
                print_error("no linter found on PATH — install clang-tidy or another linter");
            }
            return false;
        }
    };

    if manifest.linter.name.is_none() && manifest.linter.settings.is_empty() {
        print_settings_ref(linter);
    }

    let src_dir = project_dir.join("src");
    let files = collect_sources(&src_dir, &linter.template.extensions);
    if files.is_empty() {
        print_warning(&format!(
            "no source files found to lint in {}",
            project_dir.display()
        ));
        return true;
    }

    let mode = if fix { "fix" } else { "check" };
    use owo_colors::OwoColorize;
    let verb = if fix {
        "Fixing".bold().cyan().to_string()
    } else {
        "Linting".bold().cyan().to_string()
    };
    println!(
        "  {} {} files with {} {}",
        verb,
        files.len(),
        linter.template.name,
        linter.version,
    );

    match linter
        .command(&manifest.linter.settings, mode, &files)
        .status()
    {
        Ok(s) if s.success() => true,
        Ok(_) => false,
        Err(e) => {
            print_error(&format!("failed to run linter: {e}"));
            false
        }
    }
}

pub fn cmd_lint(fix: bool) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return;
        }
    };

    if let Some(ws) = load_workspace_manifest(&cwd) {
        let mut all_ok = true;
        for member in &ws.members {
            let member_dir = cwd.join(member);
            let manifest = match load_manifest(&member_dir) {
                Ok(m) => m,
                Err(e) => {
                    print_error(&format!("{member}: {e}"));
                    all_ok = false;
                    continue;
                }
            };
            use owo_colors::OwoColorize;
            println!("  {} {}", "member".bright_black(), member.bold());
            if !lint_project(&member_dir, &manifest, fix) {
                all_ok = false;
            }
        }
        if all_ok {
            if fix {
                print_success("fixes applied");
            } else {
                print_success("no issues found");
            }
        } else if fix {
            print_error("linter exited with errors while applying fixes");
        } else {
            print_error("linting found issues — run `freight lint --fix` to apply safe fixes");
        }
        return;
    }

    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => {
            print_error("no freight.toml found");
            return;
        }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    let ok = lint_project(&project_dir, &manifest, fix);
    if ok {
        if fix {
            print_success("fixes applied");
        } else {
            print_success("no issues found");
        }
    } else if fix {
        print_error("linter exited with errors while applying fixes");
    } else {
        print_error("linting found issues — run `freight lint --fix` to apply safe fixes");
    }
}
