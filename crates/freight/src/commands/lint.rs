//! `freight lint` — run the project linter over all source files.

use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::{
    DetectedTool, collect_sources, detect_tools, load_linter_templates, select_linter,
};

use crate::output::{print_error, print_success, print_warning};

fn print_settings_ref(tool: &DetectedTool) {
    use owo_colors::OwoColorize;
    let t = &tool.template;
    if t.settings.is_empty() { return; }
    println!("  {} [linter] settings for {}:", "hint:".dimmed(), t.name.bold());
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

pub fn cmd_lint(fix: bool) {
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
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let templates = load_linter_templates();
    if templates.is_empty() {
        print_warning("no linter templates found in toolchains/");
        return;
    }

    let detected = detect_tools(&templates);
    let linter = match select_linter(&detected, &manifest.linter) {
        Some(l) => l,
        None => {
            if let Some(name) = &manifest.linter.name {
                print_error(&format!("linter '{name}' not found on PATH"));
            } else {
                print_error("no linter found on PATH — install clang-tidy or another linter");
            }
            return;
        }
    };

    if manifest.linter.name.is_none() && manifest.linter.settings.is_empty() {
        print_settings_ref(linter);
    }

    let src_dir = project_dir.join("src");
    let files = collect_sources(&src_dir, &linter.template.extensions);
    if files.is_empty() {
        print_warning("no source files found to lint");
        return;
    }

    let mode = if fix { "fix" } else { "check" };
    use owo_colors::OwoColorize;
    let verb = if fix { "Fixing".bold().cyan().to_string() } else { "Linting".bold().cyan().to_string() };
    println!(
        "  {} {} files with {} {}",
        verb,
        files.len(),
        linter.template.name,
        linter.version,
    );

    let status = linter
        .command(&manifest.linter.settings, mode, &files)
        .status();

    match status {
        Ok(s) if s.success() => {
            if fix {
                print_success("fixes applied");
            } else {
                print_success("no issues found");
            }
        }
        Ok(_) => {
            if fix {
                print_error("linter exited with errors while applying fixes");
            } else {
                print_error("linting found issues — run `freight lint --fix` to apply safe fixes");
            }
        }
        Err(e) => print_error(&format!("failed to run linter: {e}")),
    }
}
