//! `freight fmt` — run the project formatter over all source files.

use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::{
    DetectedTool, collect_sources, detect_tools, load_formatter_templates, select_formatter,
};

use crate::output::{print_error, print_success, print_warning};

fn print_settings_ref(tool: &DetectedTool) {
    use owo_colors::OwoColorize;
    let t = &tool.template;
    if t.settings.is_empty() { return; }
    println!("  {} [formatter] settings for {}:", "hint:".dimmed(), t.name.bold());
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

pub fn cmd_fmt(check: bool) {
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

    let templates = load_formatter_templates();
    if templates.is_empty() {
        print_warning("no formatter templates found in toolchains/");
        return;
    }

    let detected = detect_tools(&templates);
    let formatter = match select_formatter(&detected, &manifest.formatter) {
        Some(f) => f,
        None => {
            if let Some(name) = &manifest.formatter.name {
                print_error(&format!("formatter '{name}' not found on PATH"));
            } else {
                print_error("no formatter found on PATH — install clang-format or another formatter");
            }
            return;
        }
    };

    if manifest.formatter.name.is_none() && manifest.formatter.settings.is_empty() {
        print_settings_ref(formatter);
    }

    let src_dir = project_dir.join("src");
    let files = collect_sources(&src_dir, &formatter.template.extensions);
    if files.is_empty() {
        print_warning("no source files found to format");
        return;
    }

    let mode = if check { "check" } else { "fix" };
    use owo_colors::OwoColorize;
    let verb = if check { "Checking".bold().cyan().to_string() } else { "Formatting".bold().cyan().to_string() };
    println!(
        "  {} {} files with {} {}",
        verb,
        files.len(),
        formatter.template.name,
        formatter.version,
    );

    let status = formatter
        .command(&manifest.formatter.settings, mode, &files)
        .status();

    match status {
        Ok(s) if s.success() => {
            if check {
                print_success("all files match the style");
            } else {
                print_success("formatting complete");
            }
        }
        Ok(_) => {
            if check {
                print_error("style check failed — run `freight fmt` to apply formatting");
            } else {
                print_error("formatter exited with errors");
            }
        }
        Err(e) => print_error(&format!("failed to run formatter: {e}")),
    }
}
