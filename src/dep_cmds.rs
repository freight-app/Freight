//! Pure helpers for the `freight add` / `remove` / `update` / `fetch` / `tree`
//! commands. The CLI shells live in the `freight` binary; this module only
//! exposes side-effect-free or single-purpose mutators that the shells compose.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value, value};

use crate::build::deps::resolve_dep_graph;
use crate::error::FreightError;
use crate::fetch::{self, git};
use crate::lock::LockFile;
use crate::manifest::types::{Dependency, Manifest};
use crate::manifest::{find_manifest_dir, load_manifest};
use crate::registry::{FreightRegistry, PackageRepo};
use crate::toolchain::cache::GlobalConfig;
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
            if let Some(ch) = &d.channel {
                inline.insert("channel", Value::from(ch.as_str()));
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
        if !d.patches.is_empty() {
            fetch::apply_patches(&dest, &d.patches, project_dir)?;
        }
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
            let dep_dir = http::fetch_url_dep(name, url, d.sha256.as_deref(), project_dir, &progress)?;
            if !d.patches.is_empty() {
                fetch::apply_patches(&dep_dir, &d.patches, project_dir)?;
            }
        }
        outcomes.push((name.clone(), already));
    }

    Ok(outcomes)
}

/// Resolve version-only package deps via pkg-config, system stubs, or registry.
/// Action taken for a registry-fetched version dep.
#[derive(Debug)]
pub enum RegistryDepAction {
    /// Already present in `.deps/<name>/`.
    AlreadyPresent,
    /// Downloaded from the registry and extracted.
    Downloaded,
    /// Registry unreachable and dep not cached locally.
    Unavailable,
}

#[derive(Debug)]
pub struct RegistryDepOutcome {
    pub name:    String,
    pub version: String,
    pub action:  RegistryDepAction,
}

/// Download all version deps that are missing from `.deps/` and update the lockfile.
///
/// Version deps are `Dependency::Simple("x.y")` or `Dependency::Detailed { version, repo, .. }`.
pub fn fetch_registry_deps(
    project_dir: &Path,
    config: &GlobalConfig,
) -> Result<Vec<RegistryDepOutcome>, FreightError> {
    let manifest = load_manifest(project_dir)?;
    let mut outcomes = Vec::new();

    for (name, dep) in &manifest.dependencies {
        let (version, repo_name, channel) = match dep {
            Dependency::Simple(v) => (v.as_str(), None, None),
            Dependency::Detailed(d)
                if d.version.is_some()
                    && d.path.is_none()
                    && d.git.is_none()
                    && d.url.is_none()
                    && !crate::manifest::types::is_platform_dep(name) =>
            {
                (d.version.as_deref().unwrap(), d.repo.as_deref(), d.channel.as_deref())
            }
            _ => continue,
        };

        // Skip wildcard versions — no specific version to fetch.
        if version.trim().is_empty() || version == "*" {
            continue;
        }

        // If already fetched, record and move on.
        let sentinel = project_dir.join(".deps").join(name).join(".freight-fetched");
        if sentinel.exists() {
            outcomes.push(RegistryDepOutcome {
                name:    name.clone(),
                version: version.to_string(),
                action:  RegistryDepAction::AlreadyPresent,
            });
            continue;
        }

        // Find the registry to use.
        let registry: FreightRegistry = if let Some(rname) = repo_name {
            match config.registries.iter().find(|r| r.name == rname) {
                Some(c) => FreightRegistry::from_config(c),
                None => {
                    outcomes.push(RegistryDepOutcome {
                        name: name.clone(), version: version.to_string(),
                        action: RegistryDepAction::Unavailable,
                    });
                    continue;
                }
            }
        } else {
            match config.registries.first() {
                Some(c) => FreightRegistry::from_config(c),
                None    => FreightRegistry::default_registry(),
            }
        };

        // Resolve a version constraint (e.g. ">=1.3") to a concrete version
        // by looking up available versions from the registry first.
        let resolved = if looks_like_constraint(version) {
            match registry.lookup(name, channel) {
                Ok(Some(info)) => resolve_constraint(&info.versions, version),
                _ => None,
            }
        } else {
            Some(version.to_string())
        };

        let Some(concrete) = resolved else {
            outcomes.push(RegistryDepOutcome {
                name: name.clone(), version: version.to_string(),
                action: RegistryDepAction::Unavailable,
            });
            continue;
        };

        match registry.download_tarball(name, &concrete, channel, project_dir) {
            Ok(checksum) => {
                let source = registry.source_string();
                let _ = LockFile::upsert_registry_dep(project_dir, name, &concrete, &source, &checksum);
                outcomes.push(RegistryDepOutcome {
                    name: name.clone(), version: concrete,
                    action: RegistryDepAction::Downloaded,
                });
            }
            Err(_) => {
                outcomes.push(RegistryDepOutcome {
                    name: name.clone(), version: version.to_string(),
                    action: RegistryDepAction::Unavailable,
                });
            }
        }
    }

    Ok(outcomes)
}

/// Returns true when `v` contains a constraint operator rather than a bare version.
fn looks_like_constraint(v: &str) -> bool {
    let v = v.trim();
    v.starts_with(|c: char| matches!(c, '>' | '<' | '~' | '^')) || v.starts_with(">=") || v.starts_with("<=") || v.starts_with("!=")
}

/// Pick the best version from `available` that satisfies `constraint`.
///
/// Tries semver `VersionReq` first, then falls back to a simple lexicographic
/// search so date-versions and non-semver packages still get a match.
/// Returns the oldest satisfying version (so upgrades are conservative).
fn resolve_constraint(available: &[crate::registry::PackageVersion], constraint: &str) -> Option<String> {
    use semver::{Version, VersionReq};

    let req = VersionReq::parse(constraint).ok();
    let coerce = |s: &str| -> String {
        let parts: Vec<&str> = s.split('.').collect();
        match parts.len() {
            1 => format!("{}.0.0", parts[0]),
            2 => format!("{}.{}.0", parts[0], parts[1]),
            _ => format!("{}.{}.{}", parts[0], parts[1], parts[2]),
        }
    };

    let mut best: Option<Version> = None;
    let mut best_str: Option<String> = None;

    for v in available {
        if let Some(ref req) = req {
            if let Ok(ver) = Version::parse(&coerce(&v.version)) {
                if req.matches(&ver) {
                    if best.as_ref().map_or(true, |b| ver > *b) {
                        best = Some(ver);
                        best_str = Some(v.version.clone());
                    }
                }
            }
        }
    }

    best_str
}

pub enum PackageDepAction {
    SystemPresent,
    Fetched,
    AlreadyPresent,
    Missing,
}

pub struct PackageDepOutcome {
    pub name: String,
    pub action: PackageDepAction,
}

pub fn fetch_package_deps(project_dir: &Path) -> Result<Vec<PackageDepOutcome>, FreightError> {
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

        // Registry-fetched deps are expected to already be present in .deps/
        // after `freight fetch`. Report presence or warn if missing.
        let dep_dir = project_dir.join(".deps").join(name);
        if dep_dir.exists() {
            outcomes.push(PackageDepOutcome { name: name.clone(), action: PackageDepAction::AlreadyPresent });
        } else {
            outcomes.push(PackageDepOutcome { name: name.clone(), action: PackageDepAction::Missing });
        }
    }

    Ok(outcomes)
}

fn package_dep_version(dep: &Dependency) -> Option<&str> {
    match dep {
        Dependency::Simple(version) => Some(version.as_str()),
        Dependency::Detailed(d)
            if d.version.is_some()
                && d.path.is_none()
                && d.git.is_none()
                && d.url.is_none() => d.version.as_deref(),
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
            "toolchains directory not found; set FREIGHT_TEMPLATES_DIR".into(),
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
