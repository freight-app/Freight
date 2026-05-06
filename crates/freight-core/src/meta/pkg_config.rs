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
