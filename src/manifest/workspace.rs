//! Workspace inheritance — resolving `workspace = true` markers in a member
//! `freight.toml` against the workspace-root `[workspace.dependencies]` and
//! `[workspace.package]` tables.
//!
//! Resolution happens at the TOML-document level *before* deserialization, so
//! the typed [`Manifest`](super::types::Manifest) structs never need an
//! "inherited" representation: by the time serde sees the document, every
//! `{ workspace = true }` marker has been replaced with a concrete value.
//!
//! Two forms are supported, mirroring Cargo:
//! - **Package fields**: `version.workspace = true` (and `license`, `authors`,
//!   `repository`, …) pull the value from `[workspace.package]`.
//! - **Dependencies**: `foo = { workspace = true }` in any of the three dep
//!   tables pulls the entry from `[workspace.dependencies]`. A member may add
//!   `features` (unioned with the workspace entry's), and override `optional`
//!   and `default-features`.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, InlineTable, Item, Value};

use crate::error::FreightError;

const DEP_TABLES: [&str; 3] = ["dependencies", "build-dependencies", "dev-dependencies"];

/// Resolve any workspace-inheritance markers in `src` (the text of the member
/// manifest at `dir/freight.toml`) and return the rewritten TOML.
///
/// When the manifest contains no `workspace = true` markers this is a cheap
/// no-op that returns `src` unchanged — the common case for a standalone
/// project or a workspace member that doesn't inherit anything.
pub fn resolve_inheritance(src: &str, dir: &Path) -> Result<String, FreightError> {
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| FreightError::ManifestParse(e.to_string()))?;

    if !needs_inheritance(&doc) {
        return Ok(src.to_string());
    }

    let root_dir = find_workspace_root(dir).ok_or_else(|| {
        FreightError::ManifestParse(
            "manifest uses `workspace = true` inheritance but no workspace root \
             (a freight.toml with a [workspace] section) was found in any parent directory"
                .to_string(),
        )
    })?;
    let root_src = std::fs::read_to_string(root_dir.join("freight.toml"))
        .map_err(|e| FreightError::ManifestParse(format!("reading workspace root: {e}")))?;
    let root_doc: DocumentMut = root_src
        .parse()
        .map_err(|e: toml_edit::TomlError| FreightError::ManifestParse(e.to_string()))?;
    let workspace = root_doc
        .get("workspace")
        .and_then(Item::as_table_like)
        .ok_or_else(|| {
            FreightError::ManifestParse("workspace root has no [workspace] section".to_string())
        })?;

    resolve_package(&mut doc, workspace)?;
    for table in DEP_TABLES {
        resolve_deps(&mut doc, table, workspace, dir, &root_dir)?;
    }

    Ok(doc.to_string())
}

/// Walk up from `start` (inclusive) looking for a `freight.toml` that contains a
/// `[workspace]` table; return its directory.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if let Ok(src) = std::fs::read_to_string(d.join("freight.toml")) {
            if let Ok(doc) = src.parse::<DocumentMut>() {
                if doc.get("workspace").and_then(Item::as_table_like).is_some() {
                    return Some(d.to_path_buf());
                }
            }
        }
        dir = d.parent();
    }
    None
}

/// True when any `[package]` field or dependency entry carries a
/// `{ workspace = true }` marker.
fn needs_inheritance(doc: &DocumentMut) -> bool {
    let pkg_inherits = doc
        .get("package")
        .and_then(Item::as_table_like)
        .map(|t| t.iter().any(|(_, v)| is_marker(v)))
        .unwrap_or(false);
    if pkg_inherits {
        return true;
    }
    DEP_TABLES.iter().any(|name| {
        doc.get(name)
            .and_then(Item::as_table_like)
            .map(|t| t.iter().any(|(_, v)| is_marker(v)))
            .unwrap_or(false)
    })
}

/// True when `item` is a table-like value containing `workspace = true`.
fn is_marker(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|t| t.get("workspace"))
        .and_then(Item::as_bool)
        == Some(true)
}

fn resolve_package(doc: &mut DocumentMut, workspace: &dyn toml_edit::TableLike) -> Result<(), FreightError> {
    let Some(pkg) = doc.get_mut("package").and_then(Item::as_table_like_mut) else {
        return Ok(());
    };
    let ws_pkg = workspace.get("package").and_then(Item::as_table_like);

    let keys: Vec<String> = pkg
        .iter()
        .filter(|(_, v)| is_marker(v))
        .map(|(k, _)| k.to_string())
        .collect();

    for key in keys {
        let src_val = ws_pkg.and_then(|t| t.get(&key)).ok_or_else(|| {
            FreightError::ManifestParse(format!(
                "package.{key} = {{ workspace = true }} but [workspace.package] has no '{key}'"
            ))
        })?;
        pkg.insert(&key, src_val.clone());
    }
    Ok(())
}

fn resolve_deps(
    doc: &mut DocumentMut,
    table: &str,
    workspace: &dyn toml_edit::TableLike,
    member_dir: &Path,
    root_dir: &Path,
) -> Result<(), FreightError> {
    let ws_deps = workspace.get("dependencies").and_then(Item::as_table_like);

    let Some(deps) = doc.get_mut(table).and_then(Item::as_table_like_mut) else {
        return Ok(());
    };

    let keys: Vec<String> = deps
        .iter()
        .filter(|(_, v)| is_marker(v))
        .map(|(k, _)| k.to_string())
        .collect();

    for key in keys {
        let ws_entry = ws_deps.and_then(|t| t.get(&key)).ok_or_else(|| {
            FreightError::ManifestParse(format!(
                "{table}.{key} = {{ workspace = true }} but [workspace.dependencies] has no '{key}'"
            ))
        })?;
        let member = deps.get(&key).expect("key came from this table");
        let mut merged = merge_dep(ws_entry, member)?;
        // A path in [workspace.dependencies] is relative to the workspace root;
        // once inherited it must point at the same place from the *member* dir.
        rewrite_inherited_path(&mut merged, member_dir, root_dir);
        deps.insert(&key, merged);
    }
    Ok(())
}

/// If `item` is a path dependency, rewrite its `path` (which was written
/// relative to the workspace root) to be relative to `member_dir` instead.
fn rewrite_inherited_path(item: &mut Item, member_dir: &Path, root_dir: &Path) {
    let Some(path) = item
        .as_table_like()
        .and_then(|t| t.get("path"))
        .and_then(Item::as_str)
    else {
        return;
    };
    let abs = root_dir.join(path);
    let rel = crate::lock::relative_path(member_dir, &abs);
    let rel_str = rel.to_string_lossy().into_owned();
    if let Some(t) = item.as_table_like_mut() {
        t.insert("path", toml_edit::value(rel_str));
    }
}

/// Combine a workspace dependency definition with a member's `{ workspace = true,
/// features = [...], optional = .., default-features = .. }` overrides.
fn merge_dep(ws_entry: &Item, member: &Item) -> Result<Item, FreightError> {
    let member_tbl = member.as_table_like().expect("marker is table-like");

    // If the member only writes `workspace = true`, inherit verbatim.
    let has_overrides = member_tbl.iter().any(|(k, _)| k != "workspace");
    if !has_overrides {
        return Ok(ws_entry.clone());
    }

    let mut inline = to_inline_table(ws_entry)?;

    // features: union (workspace first, then member's not already present).
    if let Some(member_features) = member_tbl.get("features").and_then(Item::as_array) {
        let mut arr = inline
            .get("features")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for v in member_features.iter() {
            if let Some(s) = v.as_str() {
                if !arr.iter().any(|e| e.as_str() == Some(s)) {
                    arr.push(s);
                }
            }
        }
        inline.insert("features", Value::Array(arr));
    }

    // optional / default-features: member overrides outright when present.
    for flag in ["optional", "default-features"] {
        if let Some(b) = member_tbl.get(flag).and_then(Item::as_bool) {
            inline.insert(flag, b.into());
        }
    }

    Ok(Item::Value(Value::InlineTable(inline)))
}

/// Normalise a workspace dependency entry (a bare version string or a table) to
/// an inline table so member overrides can be merged into it.
fn to_inline_table(entry: &Item) -> Result<InlineTable, FreightError> {
    if let Some(s) = entry.as_str() {
        let mut t = InlineTable::new();
        t.insert("version", s.into());
        return Ok(t);
    }
    if let Some(tbl) = entry.as_table_like() {
        let mut t = InlineTable::new();
        for (k, v) in tbl.iter() {
            if let Some(val) = v.as_value() {
                t.insert(k, val.clone());
            }
        }
        return Ok(t);
    }
    Err(FreightError::ManifestParse(
        "a [workspace.dependencies] entry must be a version string or a table".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn no_markers_is_passthrough() {
        let tmp = tempfile::tempdir().unwrap();
        let src = "[package]\nname=\"a\"\nversion=\"1.0.0\"\n";
        let out = resolve_inheritance(src, tmp.path()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn inherits_dependency_and_package_version() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "freight.toml",
            "[workspace]\nmembers=[\"app\"]\n\
             [workspace.package]\nversion=\"2.1.0\"\nlicense=\"MIT\"\n\
             [workspace.dependencies]\nzlib=\"1.3\"\n",
        );
        let member = "[package]\nname=\"app\"\nversion.workspace=true\nlicense.workspace=true\n\
                      [dependencies]\nzlib={ workspace = true }\n";
        let out = resolve_inheritance(member, &tmp.path().join("app")).unwrap();
        let m = crate::manifest::load_manifest_str(&out).unwrap();
        assert_eq!(m.package.version, "2.1.0");
        assert_eq!(m.package.license, "MIT");
        match m.dependencies.get("zlib").unwrap() {
            crate::manifest::types::Dependency::Simple(v) => assert_eq!(v, "1.3"),
            other => panic!("expected simple version, got {other:?}"),
        }
    }

    #[test]
    fn member_features_union_with_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "freight.toml",
            "[workspace]\nmembers=[\"app\"]\n\
             [workspace.dependencies]\nfoo={ version = \"1.0\", features = [\"a\"] }\n",
        );
        let member = "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
                      [dependencies]\nfoo={ workspace = true, features = [\"b\"], optional = true }\n";
        let out = resolve_inheritance(member, &tmp.path().join("app")).unwrap();
        let m = crate::manifest::load_manifest_str(&out).unwrap();
        match m.dependencies.get("foo").unwrap() {
            crate::manifest::types::Dependency::Detailed(d) => {
                assert_eq!(d.version.as_deref(), Some("1.0"));
                assert_eq!(d.features, vec!["a".to_string(), "b".to_string()]);
                assert!(d.optional);
            }
            other => panic!("expected detailed dep, got {other:?}"),
        }
    }

    #[test]
    fn inherited_path_dep_is_rewritten_relative_to_member() {
        let tmp = tempfile::tempdir().unwrap();
        // [workspace.dependencies].greeter path is relative to the root; the
        // member lives in app/, so the inherited path must become ../greeter.
        write(
            tmp.path(),
            "freight.toml",
            "[workspace]\nmembers=[\"app\",\"greeter\"]\n\
             [workspace.dependencies]\ngreeter = { path = \"greeter\" }\n",
        );
        let member = "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
                      [dependencies]\ngreeter = { workspace = true }\n";
        let out = resolve_inheritance(member, &tmp.path().join("app")).unwrap();
        let m = crate::manifest::load_manifest_str(&out).unwrap();
        match m.dependencies.get("greeter").unwrap() {
            crate::manifest::types::Dependency::Detailed(d) => {
                assert_eq!(d.path.as_deref(), Some("../greeter"));
            }
            other => panic!("expected detailed path dep, got {other:?}"),
        }
    }

    #[test]
    fn missing_workspace_dep_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "freight.toml",
            "[workspace]\nmembers=[\"app\"]\n[workspace.dependencies]\nother=\"1.0\"\n",
        );
        let member = "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
                      [dependencies]\nzlib={ workspace = true }\n";
        let err = resolve_inheritance(member, &tmp.path().join("app")).unwrap_err();
        assert!(format!("{err}").contains("has no 'zlib'"));
    }

    #[test]
    fn marker_without_workspace_root_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let member = "[package]\nname=\"app\"\nversion.workspace=true\n";
        let err = resolve_inheritance(member, tmp.path()).unwrap_err();
        assert!(format!("{err}").contains("no workspace root"));
    }
}
