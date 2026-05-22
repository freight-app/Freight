//! pkg-config / pkgconf integration: query cflags/libs and resolve include dirs.
//!
//! Adapted from pkg-config-rs. Key features retained:
//! - `pkgconf` fallback when `pkg-config` is not on PATH
//! - Cross-compilation env var lookup: `PKG_CONFIG_PATH_<target>`,
//!   `PKG_CONFIG_PATH_<target_u>`, `TARGET_PKG_CONFIG_PATH`, `PKG_CONFIG_PATH`
//! - `PKG_CONFIG_LIBDIR` and `PKG_CONFIG_SYSROOT_DIR` cross-compile passthrough
//! - Static mode via `PKG_CONFIG_ALL_STATIC`
use std::ffi::OsString;
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
    pkg_config_query_for_target(query, None)
}

/// Like [`pkg_config_query`] but applies cross-compilation env var lookup for `target`.
///
/// When `target` is set, env vars are resolved with the priority order:
/// `<VAR>_<target>`, `<VAR>_<target_underscored>`, `TARGET_<VAR>`, `<VAR>`.
pub fn pkg_config_query_for_target(
    query: &str,
    target: Option<&str>,
) -> Result<PkgConfigResult, FreightError> {
    let is_static = std::env::var_os("PKG_CONFIG_ALL_STATIC").is_some();
    let cflags = run_pkg_config(query, "--cflags", target, is_static, None)?;
    let libs   = run_pkg_config(query, "--libs",   target, is_static, None)?;

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
/// Queries `.pc` files in a project-local output directory without polluting
/// the process environment permanently.
pub fn pkg_config_query_with_path(
    query: &str,
    extra_paths: &[PathBuf],
) -> Result<PkgConfigResult, FreightError> {
    let existing = std::env::var("PKG_CONFIG_PATH").unwrap_or_default();
    let mut parts: Vec<String> = extra_paths.iter()
        .map(|p| p.display().to_string())
        .collect();
    if !existing.is_empty() {
        parts.push(existing);
    }
    let pkg_config_path = parts.join(":");

    let cflags = run_pkg_config(query, "--cflags", None, false, Some(&pkg_config_path))?;
    let libs   = run_pkg_config(query, "--libs",   None, false, Some(&pkg_config_path))?;

    let include_dirs = cflags
        .split_ascii_whitespace()
        .filter_map(|f| f.strip_prefix("-I"))
        .map(PathBuf::from)
        .collect();

    let link_flags = libs.split_ascii_whitespace().map(str::to_owned).collect();

    Ok(PkgConfigResult { include_dirs, link_flags })
}

/// Run `pkg-config --modversion` for the package name extracted from `query`.
/// Returns an empty string on any failure.
pub fn pkg_config_version(query: &str) -> String {
    let pkg_name = query.split_whitespace().next().unwrap_or(query);
    let result = Command::new("pkg-config")
        .args(["--modversion", pkg_name])
        .output()
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Command::new("pkgconf").args(["--modversion", pkg_name]).output()
            } else {
                Err(e)
            }
        })
        .ok();
    result
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Resolve a pkg-config env variable with cross-compilation target priority.
///
/// Search order (first non-empty wins):
/// 1. `{var_base}_{target}`       e.g. `PKG_CONFIG_PATH_aarch64-unknown-linux-gnu`
/// 2. `{var_base}_{target_u}`     e.g. `PKG_CONFIG_PATH_aarch64_unknown_linux_gnu`
/// 3. `TARGET_{var_base}`         e.g. `TARGET_PKG_CONFIG_PATH`
/// 4. `{var_base}`                e.g. `PKG_CONFIG_PATH`
fn targeted_env_var(var_base: &str, target: Option<&str>) -> Option<OsString> {
    let Some(target) = target else {
        return std::env::var_os(var_base);
    };
    let target_u = target.replace('-', "_");
    std::env::var_os(&format!("{var_base}_{target}"))
        .or_else(|| std::env::var_os(&format!("{var_base}_{target_u}")))
        .or_else(|| std::env::var_os(&format!("TARGET_{var_base}")))
        .or_else(|| std::env::var_os(var_base))
}

fn run_pkg_config(
    query: &str,
    flag: &str,
    target: Option<&str>,
    is_static: bool,
    override_path: Option<&str>,
) -> Result<String, FreightError> {
    let parts: Vec<&str> = query.split_whitespace().collect();

    let exe = targeted_env_var("PKG_CONFIG", target)
        .unwrap_or_else(|| OsString::from("pkg-config"));

    let out = build_command(&exe, flag, &parts, is_static, target, override_path)
        .output()
        .or_else(|e| {
            // Fallback to pkgconf when pkg-config binary is not found and no
            // explicit PKG_CONFIG override is in the environment.
            if e.kind() == std::io::ErrorKind::NotFound
                && targeted_env_var("PKG_CONFIG", target).is_none()
            {
                build_command(
                    &OsString::from("pkgconf"),
                    flag,
                    &parts,
                    is_static,
                    target,
                    override_path,
                )
                .output()
            } else {
                Err(e)
            }
        })
        .map_err(|e| FreightError::CompilerNotFound(format!("pkg-config not found: {e}")))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(FreightError::ManifestParse(format!(
            "pkg-config failed for '{query}': {stderr}"
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn build_command(
    exe: &OsString,
    flag: &str,
    parts: &[&str],
    is_static: bool,
    target: Option<&str>,
    override_path: Option<&str>,
) -> Command {
    let mut cmd = Command::new(exe);
    if is_static {
        cmd.arg("--static");
    }
    cmd.arg(flag).args(parts);

    if let Some(path) = override_path {
        cmd.env("PKG_CONFIG_PATH", path);
    } else if let Some(path) = targeted_env_var("PKG_CONFIG_PATH", target) {
        cmd.env("PKG_CONFIG_PATH", path);
    }
    if let Some(libdir) = targeted_env_var("PKG_CONFIG_LIBDIR", target) {
        cmd.env("PKG_CONFIG_LIBDIR", libdir);
    }
    if let Some(sysroot) = targeted_env_var("PKG_CONFIG_SYSROOT_DIR", target) {
        cmd.env("PKG_CONFIG_SYSROOT_DIR", sysroot);
    }

    cmd
}
