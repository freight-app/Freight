//! Pure helpers for the `crane add` / `remove` / `update` / `fetch` / `tree`
//! commands. The CLI shells live in the `crane` binary; this module only
//! exposes side-effect-free or single-purpose mutators that the shells compose.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value, value};

use crate::build::deps::resolve_dep_graph;
use crate::error::CraneError;
use crate::lock::LockFile;
use crate::manifest::types::{Dependency, Manifest};
use crate::manifest::{find_manifest_dir, load_manifest};
use crate::toolchain::{detect_all_cached, load_templates, templates_dir};

pub use crate::manifest::types::DetailedDep;

// ── Manifest mutation ────────────────────────────────────────────────────────

/// Insert (or overwrite) `name = <dep>` in `[dependencies]` or
/// `[dev-dependencies]`. Preserves formatting of unrelated entries.
pub fn manifest_add_dep(
    manifest_path: &Path,
    name: &str,
    dep: &Dependency,
    dev: bool,
) -> Result<(), CraneError> {
    let src = std::fs::read_to_string(manifest_path)?;
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| CraneError::ManifestParse(e.to_string()))?;

    let section = if dev { "dev-dependencies" } else { "dependencies" };

    if !doc.contains_key(section) {
        doc[section] = Item::Table(Table::new());
    }

    let table = doc[section]
        .as_table_mut()
        .ok_or_else(|| CraneError::ManifestParse(format!("[{section}] is not a table")))?;

    match dep {
        Dependency::Simple(ver) => {
            table[name] = value(ver.as_str());
        }
        Dependency::Detailed(d) => {
            let mut inline = toml_edit::InlineTable::new();
            if let Some(p) = &d.path {
                inline.insert("path", Value::from(p.as_str()));
            }
            if let Some(s) = &d.system {
                inline.insert("system", Value::from(s.as_str()));
            }
            if let Some(g) = &d.git {
                inline.insert("git", Value::from(g.as_str()));
            }
            if let Some(v) = &d.version {
                inline.insert("version", Value::from(v.as_str()));
            }
            table[name] = Item::Value(Value::InlineTable(inline));
        }
    }

    std::fs::write(manifest_path, doc.to_string())?;
    Ok(())
}

/// Remove `name` from both `[dependencies]` and `[dev-dependencies]`. Returns
/// `Ok(true)` when the dep was found and removed, `Ok(false)` when missing.
/// An emptied section is dropped from the document.
pub fn manifest_remove_dep(manifest_path: &Path, name: &str) -> Result<bool, CraneError> {
    let src = std::fs::read_to_string(manifest_path)?;
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| CraneError::ManifestParse(e.to_string()))?;

    let mut removed = false;
    for &section in &["dependencies", "dev-dependencies"] {
        let is_empty = {
            let table = match doc.get_mut(section).and_then(|i| i.as_table_mut()) {
                Some(t) => t,
                None => continue,
            };
            if table.remove(name).is_some() {
                removed = true;
            }
            table.is_empty()
        };
        if is_empty && removed {
            doc.remove(section);
        }
    }

    if removed {
        std::fs::write(manifest_path, doc.to_string())?;
    }
    Ok(removed)
}

// ── Lock regeneration ────────────────────────────────────────────────────────

/// Outcome of [`regen_lock`] — distinguishes "wrote" from "skipped because
/// deps weren't fetchable" so the CLI can show the right message.
pub enum RegenLockOutcome {
    Wrote,
    /// Deps weren't all available locally (e.g. registry deps without
    /// crane.dev). The lock file was left untouched.
    Skipped,
}

/// Regenerate `crane.lock` from the current manifest. Returns
/// [`RegenLockOutcome::Skipped`] when the dep graph cannot be fully resolved,
/// so the caller can surface a `crane fetch` hint without aborting.
pub fn regen_lock(project_dir: &Path) -> Result<RegenLockOutcome, CraneError> {
    let manifest = load_manifest(project_dir)?;

    let tdir = templates_dir().ok_or_else(|| {
        CraneError::CompilerNotFound(
            "compiler-templates directory not found; set CRANE_TEMPLATES_DIR".into(),
        )
    })?;
    let templates = load_templates(&tdir);
    let _ = detect_all_cached(&templates); // warm the version cache as a side effect

    let resolved = match resolve_dep_graph(project_dir, &manifest, false) {
        Ok(r) => r,
        Err(_) => return Ok(RegenLockOutcome::Skipped),
    };

    let lock = LockFile::generate(project_dir, &manifest, &resolved);
    lock.save(project_dir)?;
    Ok(RegenLockOutcome::Wrote)
}

// ── Read-only helpers for `crane tree` ───────────────────────────────────────

/// Resolve the project's manifest from cwd via `find_manifest_dir`.
/// Returns the directory and the parsed manifest.
pub fn locate_project() -> Result<(std::path::PathBuf, Manifest), CraneError> {
    let cwd = std::env::current_dir()?;
    let dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let manifest = load_manifest(&dir)?;
    Ok((dir, manifest))
}
