//! Factory helpers for obtaining a [`PackageRepo`] by name.

use crate::error::FreightError;
use crate::toolchain::cache::GlobalConfig;
use super::{PackageRepo, FreightRegistry};

/// Return the repo implementation for the given name, searching the configured
/// registries. Falls back to the default freight.dev registry for `""` / `"freight"`.
pub fn repo_by_name(name: &str, config: &GlobalConfig) -> Result<Box<dyn PackageRepo>, FreightError> {
    match name {
        "" | "freight" => return Ok(Box::new(FreightRegistry::default_registry())),
        _ => {}
    }

    if let Some(cfg) = config.registries.iter().find(|r| r.name == name) {
        return Ok(Box::new(FreightRegistry::from_config(cfg)));
    }

    Err(FreightError::RegistryError(format!(
        "unknown repo '{name}'; add a [[registry]] entry with name = \"{name}\" to your config"
    )))
}

/// The default repo used when no `--repo` flag is given and config has no registries.
pub fn default_repo() -> Box<dyn PackageRepo> {
    Box::new(FreightRegistry::default_registry())
}

/// Return all configured registries in priority order (declaration order),
/// with the default freight.dev registry appended last unless an entry named
/// `"freight"` is already present.
pub fn registries_in_order(config: &GlobalConfig) -> Vec<Box<dyn PackageRepo>> {
    let mut repos: Vec<Box<dyn PackageRepo>> = config.registries
        .iter()
        .map(|cfg| -> Box<dyn PackageRepo> { Box::new(FreightRegistry::from_config(cfg)) })
        .collect();

    let has_freight = config.registries.iter().any(|r| r.name == "freight");
    if !has_freight {
        repos.push(Box::new(FreightRegistry::default_registry()));
    }

    repos
}
