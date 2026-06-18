//! Fold a sibling `vcpkg.json`'s dependencies into a migrated `freight.toml`.
//!
//! `freight migrate cmake|make|autotools` reconstructs targets, standards and
//! link libraries from the project's build system. Many such projects *also*
//! declare their third-party dependencies in a `vcpkg.json` manifest, which the
//! build files reference only by bare name (no version). When a `vcpkg.json`
//! sits next to the build file, this step merges those declared dependencies —
//! with versions, features and platform conditions — into the emitted manifest,
//! producing a more complete, buildable starting point.
//!
//! Versions resolve in order: an explicit `overrides` pin in the `vcpkg.json`,
//! then the system `pkg-config` version (via [`super::system_dep_item`]), else
//! the `"*"` draft placeholder that `freight build` flags for pinning — the same
//! convention the rest of the migrators use.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, Value};

// ── vcpkg.json subset (only what we fold in) ──────────────────────────────────

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    dependencies: Vec<Dep>,
    #[serde(default)]
    overrides: Vec<Override>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Dep {
    Simple(String),
    Detailed(DetailedDep),
}

#[derive(Deserialize)]
struct DetailedDep {
    name: String,
    #[serde(default)]
    features: Vec<String>,
    #[serde(rename = "default-features", default = "default_true")]
    default_features: bool,
    #[serde(default)]
    platform: Option<String>,
}

#[derive(Deserialize)]
struct Override {
    name: String,
    #[serde(rename = "version", alias = "version-semver", alias = "version-date", alias = "version-string")]
    version: String,
}

fn default_true() -> bool { true }

struct ResolvedDep {
    name: String,
    features: Vec<String>,
    default_features: bool,
    /// `None` = unconditional `[dependencies]`; `Some("windows")` = `[os.windows.dependencies]`.
    os: Option<&'static str>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// If `project_dir/vcpkg.json` exists, merge its dependencies into `toml` and
/// return the augmented document. Otherwise return `toml` unchanged. Parse
/// failures are reported through `warnings` and leave `toml` untouched.
pub(crate) fn apply_vcpkg_manifest(
    toml: String,
    project_dir: &Path,
    warnings: &mut Vec<String>,
) -> String {
    let path = project_dir.join("vcpkg.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return toml; // no manifest — nothing to do
    };
    let manifest: Manifest = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => {
            warnings.push(format!("vcpkg.json present but could not be parsed ({e}); skipping its dependencies"));
            return toml;
        }
    };

    let overrides: HashMap<&str, &str> = manifest
        .overrides
        .iter()
        .map(|o| (o.name.as_str(), o.version.as_str()))
        .collect();

    // Resolve each dependency to a name + version + scope.
    let mut resolved: Vec<ResolvedDep> = Vec::new();
    for dep in &manifest.dependencies {
        let (name, features, default_features, platform) = match dep {
            Dep::Simple(n) => (n.clone(), Vec::new(), true, None),
            Dep::Detailed(d) => (
                d.name.clone(),
                d.features.clone(),
                d.default_features,
                d.platform.clone(),
            ),
        };
        // vcpkg's own tooling packages are not real dependencies.
        if name.starts_with("vcpkg-") {
            continue;
        }
        let os = match platform.as_deref() {
            None | Some("") => None,
            Some(expr) => match map_platform(expr) {
                Some(os) => Some(os),
                None => {
                    warnings.push(format!(
                        "vcpkg dependency `{name}` has platform `{expr}` with no freight mapping; placed in [dependencies]"
                    ));
                    None
                }
            },
        };
        resolved.push(ResolvedDep { name, features, default_features, os });
    }

    if resolved.is_empty() {
        return toml;
    }

    let mut doc: DocumentMut = match toml.parse() {
        Ok(d) => d,
        Err(e) => {
            warnings.push(format!("internal: migrated toml did not round-trip ({e}); vcpkg deps not merged"));
            return toml;
        }
    };

    let mut added = 0usize;
    for dep in &resolved {
        let version = overrides
            .get(dep.name.as_str())
            .map(|v| v.to_string())
            .unwrap_or_else(|| dep_version_string(&dep.name));
        if insert_dep(&mut doc, dep, &version) {
            added += 1;
        }
    }

    if added > 0 {
        warnings.push(format!("merged {added} dependenc(ies) from vcpkg.json"));
    }
    doc.to_string()
}

// ── TOML insertion ────────────────────────────────────────────────────────────

/// Insert `dep` into the right table. Returns `false` when an entry already
/// exists and isn't an upgradeable `"*"` placeholder (i.e. nothing was changed).
fn insert_dep(doc: &mut DocumentMut, dep: &ResolvedDep, version: &str) -> bool {
    let tbl = dep_table(doc, dep.os);

    // Don't clobber a real entry the build-system migration already produced,
    // but DO upgrade a bare `"*"` placeholder when vcpkg gives a real version.
    if let Some(existing) = tbl.get(&dep.name) {
        let existing_is_placeholder = existing
            .as_str()
            .map(|s| s == "*")
            .or_else(|| existing.as_value().and_then(|v| v.as_str()).map(|s| s == "*"))
            .unwrap_or(false);
        if !(existing_is_placeholder && version != "*") {
            return false;
        }
    }

    tbl.insert(&dep.name, dep_item(dep, version));
    true
}

/// Build the TOML value for a dependency: a bare version string, or an inline
/// table when features / `default-features = false` need to be expressed.
fn dep_item(dep: &ResolvedDep, version: &str) -> Item {
    if dep.features.is_empty() && dep.default_features {
        return toml_edit::value(version);
    }
    let mut t = toml_edit::InlineTable::new();
    t.insert("version", version.into());
    if !dep.features.is_empty() {
        let mut arr = toml_edit::Array::new();
        for f in &dep.features {
            arr.push(f.as_str());
        }
        t.insert("features", Value::Array(arr));
    }
    if !dep.default_features {
        t.insert("default-features", false.into());
    }
    toml_edit::value(t)
}

/// Get (creating if needed) the dependency table for a scope: `[dependencies]`
/// for `None`, or `[os.<os>.dependencies]` for a platform scope.
fn dep_table<'a>(doc: &'a mut DocumentMut, os: Option<&str>) -> &'a mut Table {
    match os {
        None => doc
            .entry("dependencies")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .expect("dependencies is a table"),
        Some(os_name) => {
            let os_tbl = doc
                .entry("os")
                .or_insert(Item::Table(implicit_table()))
                .as_table_mut()
                .expect("os is a table");
            os_tbl.set_implicit(true);
            let one = os_tbl
                .entry(os_name)
                .or_insert(Item::Table(implicit_table()))
                .as_table_mut()
                .expect("os.<name> is a table");
            one.set_implicit(true);
            one.entry("dependencies")
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .expect("os.<name>.dependencies is a table")
        }
    }
}

fn implicit_table() -> Table {
    let mut t = Table::new();
    t.set_implicit(true);
    t
}

/// Version for a vcpkg dep without an explicit override: the installed
/// pkg-config version, else the `"*"` draft placeholder.
fn dep_version_string(name: &str) -> String {
    // Reuse the shared resolver so vcpkg deps and find_package deps pin identically.
    super::system_dep_item(name)
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "*".to_string())
}

// ── Platform mapping ──────────────────────────────────────────────────────────

/// Map a vcpkg platform expression to a freight `[os.<os>]` key. Only simple,
/// unambiguous single-identifier expressions (and the common `!windows`) are
/// mapped; anything compound returns `None` (caller places it unconditionally).
fn map_platform(expr: &str) -> Option<&'static str> {
    match expr.trim() {
        "windows" | "windows & !uwp" | "windows | uwp" => Some("windows"),
        "!windows" => Some("unix"),
        "linux" => Some("linux"),
        "osx" | "mac" => Some("macos"),
        "freebsd" => Some("freebsd"),
        "openbsd" => Some("openbsd"),
        "android" => Some("android"),
        "ios" => Some("ios"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merge(toml: &str, vcpkg: &str) -> (String, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("freight-vcpkg-{}", uid()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vcpkg.json"), vcpkg).unwrap();
        let mut warnings = Vec::new();
        let out = apply_vcpkg_manifest(toml.to_string(), &dir, &mut warnings);
        std::fs::remove_dir_all(&dir).ok();
        (out, warnings)
    }

    fn uid() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        std::process::id() as u64 * 100_000 + C.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn merges_simple_and_override_versions() {
        let vcpkg = r#"{ "name": "p", "dependencies": ["zlib", "fmt"],
            "overrides": [{ "name": "fmt", "version": "10.2.1" }] }"#;
        let (out, _) = merge("[package]\nname = \"p\"\n", vcpkg);
        let doc: DocumentMut = out.parse().unwrap();
        let deps = doc["dependencies"].as_table().unwrap();
        assert_eq!(deps["fmt"].as_str(), Some("10.2.1")); // override wins
        assert!(deps.contains_key("zlib")); // resolved to pkg-config version or "*"
    }

    #[test]
    fn detailed_dep_with_features_emits_inline_table() {
        let vcpkg = r#"{ "name": "p", "dependencies": [
            { "name": "sdl2", "features": ["x11"], "default-features": false } ] }"#;
        let (out, _) = merge("[package]\nname = \"p\"\n", vcpkg);
        let doc: DocumentMut = out.parse().unwrap();
        let sdl = doc["dependencies"]["sdl2"].as_inline_table().unwrap();
        assert!(sdl.contains_key("version"));
        assert_eq!(sdl.get("features").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(sdl.get("default-features").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn platform_dep_goes_to_os_section() {
        let vcpkg = r#"{ "name": "p", "dependencies": [
            { "name": "dirent", "platform": "windows" } ] }"#;
        let (out, _) = merge("[package]\nname = \"p\"\n", vcpkg);
        assert!(out.contains("[os.windows.dependencies]"), "got:\n{out}");
        let doc: DocumentMut = out.parse().unwrap();
        assert!(doc["os"]["windows"]["dependencies"].as_table().unwrap().contains_key("dirent"));
    }

    #[test]
    fn upgrades_placeholder_but_keeps_real_existing() {
        // existing zlib="*" should upgrade; existing curl="8.0" should be kept.
        let toml = "[package]\nname = \"p\"\n\n[dependencies]\nzlib = \"*\"\ncurl = \"8.0\"\n";
        let vcpkg = r#"{ "name": "p", "dependencies": [],
            "overrides": [{ "name": "zlib", "version": "1.3.1" },
                          { "name": "curl", "version": "9.9" }] }"#;
        // overrides alone don't add deps; declare them as dependencies too.
        let vcpkg = vcpkg.replace("\"dependencies\": []", "\"dependencies\": [\"zlib\", \"curl\"]");
        let (out, _) = merge(toml, &vcpkg);
        let doc: DocumentMut = out.parse().unwrap();
        let deps = doc["dependencies"].as_table().unwrap();
        assert_eq!(deps["zlib"].as_str(), Some("1.3.1")); // upgraded from "*"
        assert_eq!(deps["curl"].as_str(), Some("8.0"));   // real value kept
    }

    #[test]
    fn no_manifest_is_noop() {
        let dir = std::env::temp_dir().join(format!("freight-vcpkg-none-{}", uid()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = Vec::new();
        let out = apply_vcpkg_manifest("[package]\nname=\"p\"\n".into(), &dir, &mut w);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(out, "[package]\nname=\"p\"\n");
        assert!(w.is_empty());
    }

    #[test]
    fn skips_vcpkg_tooling_packages() {
        let vcpkg = r#"{ "name": "p", "dependencies": ["vcpkg-cmake", "zlib"] }"#;
        let (out, _) = merge("[package]\nname = \"p\"\n", vcpkg);
        let doc: DocumentMut = out.parse().unwrap();
        let deps = doc["dependencies"].as_table().unwrap();
        assert!(!deps.contains_key("vcpkg-cmake"));
        assert!(deps.contains_key("zlib"));
    }
}
