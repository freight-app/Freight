pub mod find;
pub mod types;
pub mod validate;

pub use find::find_manifest_dir;
pub use types::Manifest;
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
