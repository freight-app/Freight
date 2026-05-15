//! Freight package registry client.
//!
//! The registry is queried during `freight add` to resolve a package name to a
//! concrete version. Build-time resolution (pkg-config → conan → system stubs)
//! is separate — the registry is only consulted at `freight add` time.

pub mod freight_registry;
pub mod repos;

pub use freight_registry::FreightRegistry;
pub use repos::{repo_by_name, default_repo};

use crate::error::FreightError;

/// Default registry base URL. Override with `FREIGHT_REGISTRY_URL` env var.
pub const DEFAULT_REGISTRY_URL: &str = "https://freight.dev";

/// Metadata for a single version of a package.
#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub version: String,
    /// SHA-256 of the source tarball (lowercase hex), if the registry provides it.
    pub checksum: Option<String>,
    /// Download URL for the source tarball.
    pub download_url: Option<String>,
}

/// Package metadata returned by a registry lookup.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub description: Option<String>,
    /// The latest stable version string.
    pub latest: String,
    /// All available versions, newest first.
    pub versions: Vec<PackageVersion>,
}

/// A package repository that can resolve and search packages by name.
pub trait PackageRepo: Send + Sync {
    /// Identifier used in `repo = "..."` in freight.toml.
    /// Empty string for the freight registry (the default).
    fn repo_key(&self) -> &str;

    /// Look up a package by name. Returns `Ok(None)` when not found (404).
    fn lookup(&self, name: &str) -> Result<Option<PackageInfo>, FreightError>;

    /// Search for packages matching `query`.
    fn search(&self, query: &str) -> Result<Vec<PackageInfo>, FreightError>;
}

/// Backward-compatibility alias. Prefer [`PackageRepo`].
#[deprecated(since = "0.0.0", note = "use PackageRepo instead")]
pub trait Registry: PackageRepo {}
