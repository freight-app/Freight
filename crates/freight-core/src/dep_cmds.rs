//! Pure helpers for the `freight add` / `remove` / `update` / `fetch` / `tree`
//! commands. The CLI shells live in the `freight` binary; this module only
//! exposes side-effect-free or single-purpose mutators that the shells compose.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value, value};

use crate::build::deps::resolve_dep_graph;
use crate::error::FreightError;
use crate::fetch::git;
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
) -> Result<(), FreightError> {
    let src = std::fs::read_to_string(manifest_path)?;
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| FreightError::ManifestParse(e.to_string()))?;

    let section = if dev { "dev-dependencies" } else { "dependencies" };

    if !doc.contains_key(section) {
        doc[section] = Item::Table(Table::new());
    }

    let table = doc[section]
        .as_table_mut()
        .ok_or_else(|| FreightError::ManifestParse(format!("[{section}] is not a table")))?;

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
            if let Some(b) = &d.branch {
                inline.insert("branch", Value::from(b.as_str()));
            }
            if let Some(t) = &d.tag {
                inline.insert("tag", Value::from(t.as_str()));
            }
            if let Some(r) = &d.rev {
                inline.insert("rev", Value::from(r.as_str()));
            }
            if let Some(v) = &d.version {
                inline.insert("version", Value::from(v.as_str()));
            }
            if let Some(bs) = &d.backend {
                inline.insert("backend", Value::from(bs.as_str()));
            }
            if !d.include.is_empty() {
                let mut arr = toml_edit::Array::new();
                for s in &d.include { arr.push(s.as_str()); }
                inline.insert("include", Value::Array(arr));
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
pub fn manifest_remove_dep(manifest_path: &Path, name: &str) -> Result<bool, FreightError> {
    let src = std::fs::read_to_string(manifest_path)?;
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| FreightError::ManifestParse(e.to_string()))?;

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

// ── Git dep fetch / update ───────────────────────────────────────────────────

/// Outcome of a single git dep fetch / update operation.
pub struct GitDepOutcome {
    pub name: String,
    pub action: GitDepAction,
}

pub enum GitDepAction {
    Cloned,
    AlreadyPresent,
    Updated,
    Skipped,
}

/// Clone any git deps that are not yet present under `.deps/`.
/// Already-present directories are left untouched (use [`update_git_deps`] to refresh them).
pub fn fetch_git_deps(project_dir: &Path) -> Result<Vec<GitDepOutcome>, FreightError> {
    let manifest = load_manifest(project_dir)?;
    let deps_dir = project_dir.join(".deps");
    let mut outcomes = Vec::new();

    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else { continue };
        let Some(url) = &d.git else { continue };

        let dest = deps_dir.join(name);
        if dest.exists() {
            outcomes.push(GitDepOutcome { name: name.clone(), action: GitDepAction::AlreadyPresent });
            continue;
        }

        std::fs::create_dir_all(&deps_dir)?;
        git::clone_dep(
            &dest,
            url,
            d.branch.as_deref(),
            d.tag.as_deref(),
            d.rev.as_deref(),
        )?;
        outcomes.push(GitDepOutcome { name: name.clone(), action: GitDepAction::Cloned });
    }

    Ok(outcomes)
}

/// Fetch updates for all git deps already present in `.deps/`.
/// Deps pinned with `rev` are skipped.
/// Deps not yet cloned are skipped (run `freight fetch` first).
pub fn update_git_deps(project_dir: &Path, only: Option<&str>) -> Result<Vec<GitDepOutcome>, FreightError> {
    let manifest = load_manifest(project_dir)?;
    let deps_dir = project_dir.join(".deps");
    let mut outcomes = Vec::new();

    for (name, dep) in &manifest.dependencies {
        if let Some(filter) = only {
            if name.as_str() != filter { continue; }
        }

        let Dependency::Detailed(d) = dep else { continue };
        let Some(_url) = &d.git else { continue };

        let dest = deps_dir.join(name);
        if !dest.exists() {
            outcomes.push(GitDepOutcome { name: name.clone(), action: GitDepAction::Skipped });
            continue;
        }

        if d.rev.is_some() {
            outcomes.push(GitDepOutcome { name: name.clone(), action: GitDepAction::Skipped });
            continue;
        }

        git::update_dep(&dest, d.branch.as_deref(), d.tag.as_deref(), None)?;
        outcomes.push(GitDepOutcome { name: name.clone(), action: GitDepAction::Updated });
    }

    Ok(outcomes)
}

/// Pre-fetch all `http` and `github` deps into `.deps/`.
///
/// Already-fetched directories (sentinel `.freight-fetched` present) are skipped.
/// Returns the names of deps that were fetched or were already present.
pub fn fetch_url_deps(project_dir: &Path) -> Result<Vec<(String, bool)>, FreightError> {
    use crate::event::silent;
    use crate::fetch::http;
    let manifest = load_manifest(project_dir)?;
    let progress = silent();
    let mut outcomes = Vec::new();

    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else { continue };
        let Some(url) = &d.url else { continue };

        let already = project_dir.join(".deps").join(name).join(".freight-fetched").exists();
        if !already {
            http::fetch_url_dep(name, url, d.sha256.as_deref(), project_dir, &progress)?;
        }
        outcomes.push((name.clone(), already));
    }

    Ok(outcomes)
}

/// Resolve version-only package deps by preferring system packages and falling
/// back to the project-local vcpkg install tree.
pub enum PackageDepAction {
    SystemPresent,
    Fetched,
    AlreadyPresent,
}

pub struct PackageDepOutcome {
    pub name: String,
    pub action: PackageDepAction,
}

pub fn fetch_package_deps(project_dir: &Path) -> Result<Vec<PackageDepOutcome>, FreightError> {
    use crate::event::silent;
    use crate::fetch::vcpkg;
    use crate::meta::pkg_config_query;

    let manifest = load_manifest(project_dir)?;
    let mut outcomes = Vec::new();

    for (name, dep) in &manifest.dependencies {
        let Some(version) = package_dep_version(dep) else { continue };
        let query = package_query(name, version);

        if pkg_config_query(&query).is_ok() {
            outcomes.push(PackageDepOutcome { name: name.clone(), action: PackageDepAction::SystemPresent });
            continue;
        }

        let triplet = vcpkg::default_triplet();
        let already = vcpkg::installed_root(project_dir)
            .join(".freight")
            .join(format!("{name}.{triplet}.fetched"))
            .exists();
        if !already {
            vcpkg::fetch_vcpkg_dep(name, name, Some(&triplet), project_dir, &silent())?;
        }

        outcomes.push(PackageDepOutcome {
            name: name.clone(),
            action: if already { PackageDepAction::AlreadyPresent } else { PackageDepAction::Fetched },
        });
    }

    Ok(outcomes)
}

fn package_dep_version(dep: &Dependency) -> Option<&str> {
    match dep {
        Dependency::Simple(version) => Some(version.as_str()),
        Dependency::Detailed(d)
            if d.version.is_some()
                && d.path.is_none()
                && d.system.is_none()
                && d.git.is_none()
                && d.url.is_none()
                && d.pkg_config.is_none() => d.version.as_deref(),
        _ => None,
    }
}

fn package_query(name: &str, version: &str) -> String {
    let version = version.trim();
    if version.is_empty() || version == "*" {
        return name.to_string();
    }
    if matches!(version.as_bytes().first(), Some(b'<' | b'>' | b'=' | b'!')) {
        format!("{name} {version}")
    } else {
        format!("{name} >= {version}")
    }
}

/// Remove the `.freight-fetched` sentinel for the named url dep so
/// `freight fetch` (or the next build) will re-download it.
pub fn invalidate_url_dep(project_dir: &Path, name: &str) -> bool {
    let sentinel = project_dir.join(".deps").join(name).join(".freight-fetched");
    if sentinel.exists() {
        let _ = std::fs::remove_file(&sentinel);
        true
    } else {
        false
    }
}

// ── Lock regeneration ────────────────────────────────────────────────────────

/// Outcome of [`regen_lock`] — distinguishes "wrote" from "skipped because
/// deps weren't fetchable" so the CLI can show the right message.
pub enum RegenLockOutcome {
    Wrote,
    /// Deps weren't all available locally (e.g. registry deps without
    /// freight.dev). The lock file was left untouched.
    Skipped,
}

/// Regenerate `freight.lock` from the current manifest. Returns
/// [`RegenLockOutcome::Skipped`] when the dep graph cannot be fully resolved,
/// so the caller can surface a `freight fetch` hint without aborting.
pub fn regen_lock(project_dir: &Path) -> Result<RegenLockOutcome, FreightError> {
    let manifest = load_manifest(project_dir)?;

    let tdir = templates_dir().ok_or_else(|| {
        FreightError::CompilerNotFound(
            "toolchains directory not found; set CRANE_TEMPLATES_DIR".into(),
        )
    })?;
    let templates = load_templates(&tdir);
    let _ = detect_all_cached(&templates); // warm the version cache as a side effect

    let empty = std::collections::BTreeSet::new();
    let resolved = match resolve_dep_graph(project_dir, &manifest, false, &empty) {
        Ok(r) => r,
        Err(_) => return Ok(RegenLockOutcome::Skipped),
    };

    let lock = LockFile::generate(project_dir, &manifest, &resolved);
    lock.save(project_dir)?;
    Ok(RegenLockOutcome::Wrote)
}

// ── Read-only helpers for `freight tree` ───────────────────────────────────────

/// Resolve the project's manifest from cwd via `find_manifest_dir`.
/// Returns the directory and the parsed manifest.
pub fn locate_project() -> Result<(std::path::PathBuf, Manifest), FreightError> {
    let cwd = std::env::current_dir()?;
    let dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let manifest = load_manifest(&dir)?;
    Ok((dir, manifest))
}
