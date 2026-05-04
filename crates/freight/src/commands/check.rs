use std::path::Path;

use freight_core::error::FreightError;
use freight_core::manifest::{
    find_manifest_dir, load_manifest, load_workspace_manifest, validate, validate_dep_compat,
    Manifest,
};
use freight_core::toolchain::{load_templates, templates_dir};

use crate::output::{print_error, print_status, print_success, print_warning};

pub fn cmd_check() {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot determine current directory: {e}")); return; }
    };

    let manifest_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => {
            print_error("no freight.toml found in this directory or any parent");
            return;
        }
    };

    // Workspace root: validate each member.
    if let Some(ws) = load_workspace_manifest(&manifest_dir) {
        println!("Workspace with {} member(s):", ws.members.len());
        let templates = templates_dir().map(|d| load_templates(&d)).unwrap_or_default();
        let mut all_ok = true;
        for member in &ws.members {
            let member_dir = manifest_dir.join(member.trim_end_matches('/'));
            if !check_one(&member_dir, &templates) {
                all_ok = false;
            }
        }
        if all_ok {
            print_success("all workspace members are valid");
        }
        return;
    }

    // Single project.
    let templates = templates_dir().map(|d| load_templates(&d)).unwrap_or_default();
    check_one(&manifest_dir, &templates);
}

fn check_one(manifest_dir: &Path, templates: &[freight_core::toolchain::CompilerTemplate]) -> bool {
    let manifest = match load_manifest(manifest_dir) {
        Ok(m) => m,
        Err(FreightError::ManifestParse(msg)) => {
            print_error(&format!("{}: freight.toml could not be parsed: {msg}", manifest_dir.display()));
            return false;
        }
        Err(e) => { print_error(&e.to_string()); return false; }
    };

    let mut errors = validate(&manifest, templates);
    errors.extend(validate_dep_compat(&manifest, manifest_dir, templates));

    if errors.is_empty() {
        print_success(&format!("{}: freight.toml is valid", manifest.package.name));
        print_manifest_summary(&manifest);
        true
    } else {
        let count = errors.len();
        print_error(&format!(
            "{}: {} {}",
            manifest.package.name,
            count,
            if count == 1 { "error" } else { "errors" }
        ));
        for e in &errors {
            eprintln!("  {:16} {}", e.context, e.message);
        }
        false
    }
}

fn print_manifest_summary(m: &Manifest) {
    print_status("package", &format!("{} {}", m.package.name, m.package.version));

    if !m.language.is_empty() {
        let mut langs: Vec<String> = m.language.iter().map(|(key, settings)| {
            let std_part = settings.std.as_deref().map(|s| format!(" ({s})")).unwrap_or_default();
            format!("{key}{std_part}")
        }).collect();
        langs.sort();
        print_status("language", &langs.join(", "));
    }

    let target_count = m.bins.len() + m.lib.is_some() as usize;
    let target_desc = format_targets(m);
    print_status("targets", &format!("{target_count} — {target_desc}"));

    let dep_count = m.dependencies.len();
    let dev_dep_count = m.dev_dependencies.len();
    if dep_count > 0 || dev_dep_count > 0 {
        print_status("deps", &format!("{dep_count} runtime, {dev_dep_count} dev"));
    }

    let profiles: Vec<&str> = [
        m.profile.dev.is_some().then_some("dev"),
        m.profile.release.is_some().then_some("release"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !profiles.is_empty() {
        print_status("profiles", &profiles.join(", "));
    }

    if !m.features.is_empty() {
        let names: Vec<&str> = m.features.keys().map(String::as_str).collect();
        print_status("features", &names.join(", "));
    }

    if !m.compiler.overrides.is_empty() {
        print_warning(&format!(
            "{} extension override(s) active",
            m.compiler.overrides.len()
        ));
    }
}

fn format_targets(m: &Manifest) -> String {
    let mut parts = Vec::new();
    if !m.bins.is_empty() {
        let names: Vec<&str> = m.bins.iter().map(|b| b.name.as_str()).collect();
        parts.push(format!("bin: {}", names.join(", ")));
    }
    if let Some(lib) = &m.lib {
        parts.push(format!("lib: {} ({:?})", lib.src, lib.lib_type));
    }
    parts.join("; ")
}
