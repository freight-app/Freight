//! Freight package registry client.
//!
//! The registry is queried during `freight add` to resolve a package name to a
//! concrete version. Build-time resolution (pkg-config → system stubs → registry)
//! is separate — the registry is only consulted at `freight add` time.

pub mod freight_registry;
pub mod repos;

pub use freight_registry::FreightRegistry;
pub use repos::{repo_by_name, default_repo, registries_in_order};

use crate::error::FreightError;

/// Default registry base URL. Override with `FREIGHT_REGISTRY_URL` env var.
pub const DEFAULT_REGISTRY_URL: &str = "https://freight.dev";

/// Metadata for a single version of a package.
#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub version: String,
    /// SHA-256 of the source tarball (lowercase hex), if the registry provides it.
    pub checksum: Option<String>,
    /// Download URL for the source tarball (registry-hosted or upstream redirect).
    pub download_url: Option<String>,
    /// Upstream source archive URL (set for metadata-only packages that point to
    /// a GitHub release or similar). When present, `download_url` equals this value
    /// and no tarball is stored on the registry server.
    pub upstream_url: Option<String>,
    /// Foreign build system needed to compile this package ("cmake", "make", …).
    /// `None` for packages that ship pre-built headers + libs.
    pub build_system: Option<String>,
    /// Target triples for which prebuilt binary tarballs are available.
    pub prebuilt_triples: Vec<String>,
    /// Dependencies declared in freight.toml: name → version constraint.
    pub dependencies: std::collections::HashMap<String, String>,
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

    /// Look up a package by name in the given channel (`None` = registry default).
    /// Returns `Ok(None)` when not found (404).
    fn lookup(&self, name: &str, channel: Option<&str>) -> Result<Option<PackageInfo>, FreightError>;

    /// Search for packages matching `query`.
    fn search(&self, query: &str) -> Result<Vec<PackageInfo>, FreightError>;

    /// Fetch the README for a package. Returns `None` if not available.
    fn fetch_readme(&self, name: &str) -> Option<String>;
}

/// Backward-compatibility alias. Prefer [`PackageRepo`].
#[deprecated(since = "0.0.0", note = "use PackageRepo instead")]
pub trait Registry: PackageRepo {}

/// Return a normalised target triple for the current host, e.g.
/// `"x86_64-linux-gnu"`, `"aarch64-apple-darwin"`, `"x86_64-windows-msvc"`.
///
/// Used to select prebuilt tarballs during `freight fetch`.
pub fn host_triple() -> String {
    let arch = std::env::consts::ARCH; // "x86_64" | "aarch64" | "arm" | …
    match std::env::consts::OS {
        "linux"   => format!("{arch}-linux-gnu"),
        "macos"   => format!("{arch}-apple-darwin"),
        "windows" => format!("{arch}-windows-msvc"),
        other     => format!("{arch}-{other}"),
    }
}
