pub mod find;
pub mod types;
pub mod validate;

pub use find::find_manifest_dir;
pub use types::Manifest;
pub use validate::{validate, validate_dep_compat, ValidationError};

use std::path::Path;

use crate::error::CraneError;
use crate::output::{print_error, print_status, print_success, print_warning};

/// Parse a `Manifest` from a TOML string (used in tests and `crane check`).
pub fn load_manifest_str(src: &str) -> Result<Manifest, CraneError> {
    toml_edit::de::from_str(src)
        .map_err(|e: toml_edit::de::Error| CraneError::ManifestParse(e.to_string()))
}

/// Load `crane.toml` from `dir`.
pub fn load_manifest(dir: &Path) -> Result<Manifest, CraneError> {
    let path = dir.join("crane.toml");
    let src = std::fs::read_to_string(&path).map_err(|_| {
        CraneError::ManifestNotFound(dir.to_string_lossy().into_owned())
    })?;
    load_manifest_str(&src)
}

/// Implementation of `crane check`.
pub fn cmd_check() {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot determine current directory: {e}")); return; }
    };

    let manifest_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => {
            print_error("no crane.toml found in this directory or any parent");
            return;
        }
    };

    let manifest = match load_manifest(&manifest_dir) {
        Ok(m) => m,
        Err(CraneError::ManifestParse(msg)) => {
            print_error("crane.toml could not be parsed:");
            eprintln!("  {msg}");
            return;
        }
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let templates = crate::toolchain::templates_dir()
        .map(|d| crate::toolchain::load_templates(&d))
        .unwrap_or_default();

    let mut errors = validate(&manifest, &templates);
    errors.extend(validate_dep_compat(&manifest, &manifest_dir, &templates));

    if errors.is_empty() {
        print_success("crane.toml is valid");
        println!();
        print_manifest_summary(&manifest);
    } else {
        let count = errors.len();
        print_error(&format!(
            "crane.toml: {count} {}",
            if count == 1 { "error" } else { "errors" }
        ));
        println!();
        for e in &errors {
            eprintln!("  {:16} {}", e.context, e.message);
        }
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
