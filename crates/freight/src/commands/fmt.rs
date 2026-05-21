//! `freight fmt` — run the project formatter over all source files.

use std::path::Path;

use freight_core::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};
use freight_core::manifest::types::Manifest;
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

/// Format a single project. Returns `true` on success.
fn fmt_project(project_dir: &Path, manifest: &Manifest, check: bool) -> bool {
    let templates = load_formatter_templates();
    let detected = detect_tools(&templates);
    let formatter = match select_formatter(&detected, &manifest.formatter) {
        Some(f) => f,
        None => {
            if let Some(name) = &manifest.formatter.name {
                print_error(&format!("formatter '{name}' not found on PATH"));
            } else {
                print_error("no formatter found on PATH — install clang-format or another formatter");
            }
            return false;
        }
    };

    if manifest.formatter.name.is_none() && manifest.formatter.settings.is_empty() {
        print_settings_ref(formatter);
    }

    let src_dir = project_dir.join("src");
    let files = collect_sources(&src_dir, &formatter.template.extensions);
    if files.is_empty() {
        print_warning(&format!("no source files found to format in {}", project_dir.display()));
        return true;
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

    match formatter.command(&manifest.formatter.settings, mode, &files).status() {
        Ok(s) if s.success() => true,
        Ok(_) => false,
        Err(e) => { print_error(&format!("failed to run formatter: {e}")); false }
    }
}

pub fn cmd_fmt(check: bool) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };

    if let Some(ws) = load_workspace_manifest(&cwd) {
        let mut all_ok = true;
        for member in &ws.members {
            let member_dir = cwd.join(member);
            let manifest = match load_manifest(&member_dir) {
                Ok(m) => m,
                Err(e) => { print_error(&format!("{member}: {e}")); all_ok = false; continue; }
            };
            use owo_colors::OwoColorize;
            println!("  {} {}", "member".bright_black(), member.bold());
            if !fmt_project(&member_dir, &manifest, check) {
                all_ok = false;
            }
        }
        if all_ok {
            if check {
                print_success("all workspace members match the style");
            } else {
                print_success("formatting complete");
            }
        } else if check {
            print_error("style check failed — run `freight fmt` to apply formatting");
        } else {
            print_error("formatter exited with errors");
        }
        return;
    }

    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no freight.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let ok = fmt_project(&project_dir, &manifest, check);
    if ok {
        if check {
            print_success("all files match the style");
        } else {
            print_success("formatting complete");
        }
    } else if check {
        print_error("style check failed — run `freight fmt` to apply formatting");
    } else {
        print_error("formatter exited with errors");
    }
}
