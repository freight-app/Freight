pub mod find;
pub mod supports;
pub mod types;
pub mod validate;
pub mod workspace;

pub use find::find_manifest_dir;
pub use types::{LintLevel, LintsConfig, Manifest, WorkspaceSection};
pub use validate::{validate, validate_dep_compat, ValidationError};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::error::FreightError;

/// Parse a `Manifest` from a TOML string (used in tests and `freight check`).
pub fn load_manifest_str(src: &str) -> Result<Manifest, FreightError> {
    toml_edit::de::from_str(src)
        .map_err(|e: toml_edit::de::Error| FreightError::ManifestParse(e.to_string()))
}

/// Parse a workspace-root `freight.toml` from a TOML string.
pub fn load_workspace_manifest_str(src: &str) -> Result<WorkspaceSection, FreightError> {
    let parsed: types::WorkspaceToml = toml_edit::de::from_str(src)
        .map_err(|e: toml_edit::de::Error| FreightError::ManifestParse(e.to_string()))?;
    Ok(parsed.workspace)
}

/// Load `freight.toml` from `dir`.
///
/// Resolves any `workspace = true` inheritance markers against the
/// workspace-root manifest before parsing (see [`workspace::resolve_inheritance`]).
pub fn load_manifest(dir: &Path) -> Result<Manifest, FreightError> {
    let path = dir.join("freight.toml");
    let src = std::fs::read_to_string(&path)
        .map_err(|_| FreightError::ManifestNotFound(dir.to_string_lossy().into_owned()))?;
    let resolved = workspace::resolve_inheritance(&src, dir)?;
    load_manifest_str(&resolved)
}

/// Like [`load_manifest`] but memoised by file mtime — for read-heavy callers
/// (the LSP loads the same manifests many times per index refresh and once per
/// inlay-hint hover). Re-parses whenever `freight.toml`'s mtime changes, so an
/// edit is always picked up. The build/compile path should use the uncached
/// [`load_manifest`] to avoid any mtime-granularity staleness after a write.
pub fn load_manifest_cached(dir: &Path) -> Result<Manifest, FreightError> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (SystemTime, Manifest)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let path = dir.join("freight.toml");
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

    if let Some(mt) = mtime {
        if let Ok(guard) = cache.lock() {
            if let Some((cached_mt, manifest)) = guard.get(dir) {
                if *cached_mt == mt {
                    return Ok(manifest.clone());
                }
            }
        }
    }
    let manifest = load_manifest(dir)?;
    if let (Some(mt), Ok(mut guard)) = (mtime, cache.lock()) {
        guard.insert(dir.to_path_buf(), (mt, manifest.clone()));
    }
    Ok(manifest)
}

/// Try to load a workspace root `freight.toml` from `dir`.
///
/// Returns `Some(WorkspaceSection)` when the file exists and contains a
/// `[workspace]` section. Returns `None` for regular project manifests or
/// when the file is absent.
pub fn load_workspace_manifest(dir: &Path) -> Option<WorkspaceSection> {
    let src = std::fs::read_to_string(dir.join("freight.toml")).ok()?;
    load_workspace_manifest_str(&src).ok()
}

#[cfg(test)]
mod cache_tests {
    use super::load_manifest_cached;
    use std::fs;
    use std::time::{Duration, SystemTime};

    #[test]
    fn cached_loader_reloads_on_mtime_change() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let toml = dir.join("freight.toml");
        fs::write(&toml, "[package]\nname=\"a\"\nversion=\"1.0.0\"\n").unwrap();

        // First load populates the cache; second returns the same.
        assert_eq!(load_manifest_cached(dir).unwrap().package.version, "1.0.0");
        assert_eq!(load_manifest_cached(dir).unwrap().package.version, "1.0.0");

        // Rewrite with a distinct mtime → the cache must re-read.
        fs::write(&toml, "[package]\nname=\"a\"\nversion=\"2.0.0\"\n").unwrap();
        let f = fs::OpenOptions::new().write(true).open(&toml).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(10))
            .unwrap();

        assert_eq!(
            load_manifest_cached(dir).unwrap().package.version,
            "2.0.0",
            "cache should re-read after the freight.toml mtime changes"
        );
    }
}
