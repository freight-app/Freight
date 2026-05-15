//! Factory helpers for obtaining a [`PackageRepo`] by name.

use crate::error::FreightError;
use super::{PackageRepo, FreightRegistry};

/// Return the repo implementation for the given name.
///
/// Currently only `""` / `"freight"` is supported; the freight registry is the
/// single source of truth. Additional repos may be added in the future.
pub fn repo_by_name(name: &str) -> Result<Box<dyn PackageRepo>, FreightError> {
    match name {
        "" | "freight" => Ok(Box::new(FreightRegistry::new())),
        other => Err(FreightError::RegistryError(format!(
            "unknown repo '{other}'; only 'freight' is currently supported"
        ))),
    }
}

/// The default repo used when no `--repo` flag is given.
pub fn default_repo() -> Box<dyn PackageRepo> {
    Box::new(FreightRegistry::new())
}
