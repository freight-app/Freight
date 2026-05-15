//! Factory helpers for obtaining a [`PackageRepo`] by name.

use crate::error::FreightError;
use super::{PackageRepo, FreightRegistry};
use super::vcpkg::VcpkgRepo;
use super::conan::ConanRepo;

/// Return the repo implementation for the given name.
///
/// Known names: `""` / `"freight"` (default), `"vcpkg"`, `"conan"`.
pub fn repo_by_name(name: &str) -> Result<Box<dyn PackageRepo>, FreightError> {
    match name {
        "" | "freight" => Ok(Box::new(FreightRegistry::new())),
        "vcpkg"        => Ok(Box::new(VcpkgRepo)),
        "conan"        => Ok(Box::new(ConanRepo)),
        other => Err(FreightError::RegistryError(format!(
            "unknown repo '{other}'; known: freight, vcpkg, conan"
        ))),
    }
}

/// The default repo used when no `--repo` flag is given.
pub fn default_repo() -> Box<dyn PackageRepo> {
    Box::new(FreightRegistry::new())
}
