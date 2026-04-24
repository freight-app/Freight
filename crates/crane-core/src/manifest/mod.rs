pub mod find;
pub mod types;
pub mod validate;

pub use find::find_manifest_dir;
pub use types::{Manifest, WorkspaceSection};
pub use validate::{validate, validate_dep_compat, ValidationError};

use std::path::Path;

use crate::error::CraneError;

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

/// Try to load a workspace root `crane.toml` from `dir`.
///
/// Returns `Some(WorkspaceSection)` when the file exists and contains a
/// `[workspace]` section. Returns `None` for regular project manifests or
/// when the file is absent.
pub fn load_workspace_manifest(dir: &Path) -> Option<WorkspaceSection> {
    let src = std::fs::read_to_string(dir.join("crane.toml")).ok()?;
    let parsed: types::WorkspaceToml = toml_edit::de::from_str(&src).ok()?;
    Some(parsed.workspace)
}
