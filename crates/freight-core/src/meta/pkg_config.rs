//! pkg-config integration: query cflags/libs and resolve include dirs.
use std::path::PathBuf;
use std::process::Command;

use crate::error::FreightError;

pub struct PkgConfigResult {
    pub include_dirs: Vec<PathBuf>,
    pub link_flags: Vec<String>,
}

pub struct ResolvedPkgConfig {
    pub name: String,
    pub found: bool,
    pub version: String,
    pub include_dirs: Vec<PathBuf>,
}

/// Run `pkg-config --cflags` and `--libs` for `query` and return the results.
pub fn pkg_config_query(query: &str) -> Result<PkgConfigResult, FreightError> {
    let cflags = run_pkg_config(query, "--cflags")?;
    let libs   = run_pkg_config(query, "--libs")?;

    let include_dirs = cflags
        .split_ascii_whitespace()
        .filter_map(|f| f.strip_prefix("-I"))
        .map(PathBuf::from)
        .collect();

    let link_flags = libs.split_ascii_whitespace().map(str::to_owned).collect();

    Ok(PkgConfigResult { include_dirs, link_flags })
}

/// Like [`pkg_config_query`] but prepends `extra_paths` to `PKG_CONFIG_PATH`.
///
/// Used by the conan resolver to query `.pc` files generated in a project-local
/// output directory without polluting the process environment permanently.
pub fn pkg_config_query_with_path(query: &str, extra_paths: &[std::path::PathBuf]) -> Result<PkgConfigResult, FreightError> {
    let existing = std::env::var("PKG_CONFIG_PATH").unwrap_or_default();
    let mut path_parts: Vec<String> = extra_paths.iter()
        .map(|p| p.display().to_string())
        .collect();
    if !existing.is_empty() {
        path_parts.push(existing);
    }
    let pkg_config_path = path_parts.join(":");

    let cflags = run_pkg_config_with_env(query, "--cflags", &pkg_config_path)?;
    let libs   = run_pkg_config_with_env(query, "--libs",   &pkg_config_path)?;

    let include_dirs = cflags
        .split_ascii_whitespace()
        .filter_map(|f| f.strip_prefix("-I"))
        .map(std::path::PathBuf::from)
        .collect();

    let link_flags = libs.split_ascii_whitespace().map(str::to_owned).collect();

    Ok(PkgConfigResult { include_dirs, link_flags })
}

fn run_pkg_config_with_env(query: &str, flag: &str, pkg_config_path: &str) -> Result<String, FreightError> {
    let parts: Vec<&str> = query.split_whitespace().collect();
    let out = Command::new("pkg-config")
        .arg(flag)
        .args(&parts)
        .env("PKG_CONFIG_PATH", pkg_config_path)
        .output()
        .map_err(|e| FreightError::CompilerNotFound(format!("pkg-config not found: {e}")))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(FreightError::ManifestParse(format!(
            "pkg-config failed for '{query}': {stderr}"
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Run `pkg-config --modversion` and return the version string, or empty on failure.
pub fn pkg_config_version(query: &str) -> String {
    let pkg_name = query.split_whitespace().next().unwrap_or(query);
    let out = Command::new("pkg-config")
        .args(["--modversion", pkg_name])
        .output()
        .ok();
    out.filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn run_pkg_config(query: &str, flag: &str) -> Result<String, FreightError> {
    let parts: Vec<&str> = query.split_whitespace().collect();
    let out = Command::new("pkg-config")
        .arg(flag)
        .args(&parts)
        .output()
        .map_err(|e| FreightError::CompilerNotFound(format!("pkg-config not found: {e}")))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(FreightError::ManifestParse(format!(
            "pkg-config failed for '{query}': {stderr}"
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
