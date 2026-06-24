//! pkg-config / pkgconf integration: query cflags/libs and resolve include dirs.
//!
//! Adapted from pkg-config-rs. Key features retained:
//! - `pkgconf` fallback when `pkg-config` is not on PATH
//! - Cross-compilation env var lookup: `PKG_CONFIG_PATH_<target>`,
//!   `PKG_CONFIG_PATH_<target_u>`, `TARGET_PKG_CONFIG_PATH`, `PKG_CONFIG_PATH`
//! - `PKG_CONFIG_LIBDIR` and `PKG_CONFIG_SYSROOT_DIR` cross-compile passthrough
//! - Static mode via `PKG_CONFIG_ALL_STATIC`
use std::ffi::OsString;
use std::path::{Path, PathBuf};
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
    let cflags = run_pkg_config(query, "--cflags", target, is_static, None, None)?;
    let libs = run_pkg_config(query, "--libs", target, is_static, None, None)?;

    let include_dirs = cflags
        .split_ascii_whitespace()
        .filter_map(|f| f.strip_prefix("-I"))
        .map(PathBuf::from)
        .collect();

    let link_flags = libs.split_ascii_whitespace().map(str::to_owned).collect();

    Ok(PkgConfigResult {
        include_dirs,
        link_flags,
    })
}

/// Query pkg-config for a cross-compilation `target`, scoped to a `sysroot`.
///
/// Sets `PKG_CONFIG_SYSROOT_DIR` to `sysroot` and **restricts** the search to the
/// sysroot's pkg-config dirs via `PKG_CONFIG_LIBDIR` (so the host's
/// `/usr/lib/pkgconfig` can't leak host paths into a cross build). Returns
/// sysroot-relative `-I`/`-L` paths. With no `.pc` file in the sysroot the query
/// fails like any other miss, and the caller falls through to a source build.
pub fn pkg_config_query_cross(
    query: &str,
    target: Option<&str>,
    sysroot: &Path,
) -> Result<PkgConfigResult, FreightError> {
    let is_static = std::env::var_os("PKG_CONFIG_ALL_STATIC").is_some();
    let cflags = run_pkg_config(query, "--cflags", target, is_static, None, Some(sysroot))?;
    let libs = run_pkg_config(query, "--libs", target, is_static, None, Some(sysroot))?;

    let include_dirs = cflags
        .split_ascii_whitespace()
        .filter_map(|f| f.strip_prefix("-I"))
        .map(PathBuf::from)
        .collect();
    let link_flags = libs.split_ascii_whitespace().map(str::to_owned).collect();
    Ok(PkgConfigResult {
        include_dirs,
        link_flags,
    })
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
    let mut parts: Vec<String> = extra_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    if !existing.is_empty() {
        parts.push(existing);
    }
    let pkg_config_path = parts.join(":");

    let cflags = run_pkg_config(query, "--cflags", None, false, Some(&pkg_config_path), None)?;
    let libs = run_pkg_config(query, "--libs", None, false, Some(&pkg_config_path), None)?;

    let include_dirs = cflags
        .split_ascii_whitespace()
        .filter_map(|f| f.strip_prefix("-I"))
        .map(PathBuf::from)
        .collect();

    let link_flags = libs.split_ascii_whitespace().map(str::to_owned).collect();

    Ok(PkgConfigResult {
        include_dirs,
        link_flags,
    })
}

/// Run `pkg-config --modversion` for the package name extracted from `query`.
/// Returns an empty string on any failure.
/// Enumerate every pkg-config package installed on the system: `(name,
/// description)` where the description is pkg-config's own `Description` field.
/// Empty when pkg-config/pkgconf isn't available. Order follows `--list-all`.
pub fn pkg_config_list_all() -> Vec<(String, String)> {
    let output = Command::new("pkg-config")
        .arg("--list-all")
        .output()
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Command::new("pkgconf").arg("--list-all").output()
            } else {
                Err(e)
            }
        })
        .ok();
    let Some(output) = output.filter(|o| o.status.success()) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            // "name<spaces>Name - Description"
            let mut it = line.splitn(2, char::is_whitespace);
            let name = it.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let desc = it.next().unwrap_or("").trim().to_string();
            Some((name.to_string(), desc))
        })
        .collect()
}

pub fn pkg_config_version(query: &str) -> String {
    let pkg_name = query.split_whitespace().next().unwrap_or(query);
    let result = Command::new("pkg-config")
        .args(["--modversion", pkg_name])
        .output()
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Command::new("pkgconf")
                    .args(["--modversion", pkg_name])
                    .output()
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
    std::env::var_os(format!("{var_base}_{target}"))
        .or_else(|| std::env::var_os(format!("{var_base}_{target_u}")))
        .or_else(|| std::env::var_os(format!("TARGET_{var_base}")))
        .or_else(|| std::env::var_os(var_base))
}

fn run_pkg_config(
    query: &str,
    flag: &str,
    target: Option<&str>,
    is_static: bool,
    override_path: Option<&str>,
    sysroot: Option<&Path>,
) -> Result<String, FreightError> {
    let parts: Vec<&str> = query.split_whitespace().collect();

    let exe =
        targeted_env_var("PKG_CONFIG", target).unwrap_or_else(|| OsString::from("pkg-config"));

    let out = build_command(
        &exe,
        flag,
        &parts,
        is_static,
        target,
        override_path,
        sysroot,
    )
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
                sysroot,
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
    sysroot: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(exe);
    if is_static {
        cmd.arg("--static");
    }
    cmd.arg(flag).args(parts);

    // An explicit sysroot (cross build) wins: point pkg-config entirely inside
    // it — `PKG_CONFIG_LIBDIR` *restricts* the search to the sysroot so the host
    // `/usr/lib/pkgconfig` can't leak host `-I`/`-L` paths, and
    // `PKG_CONFIG_SYSROOT_DIR` rewrites `-I`/`-L` prefixes to the sysroot. This
    // mirrors a Yocto/Petalinux SDK `environment-setup` script.
    if let Some(root) = sysroot {
        let libdir = sysroot_pkgconfig_path(root);
        cmd.env("PKG_CONFIG_LIBDIR", &libdir);
        cmd.env("PKG_CONFIG_PATH", &libdir);
        cmd.env("PKG_CONFIG_SYSROOT_DIR", root);
        return cmd;
    }

    if let Some(path) = override_path {
        cmd.env("PKG_CONFIG_PATH", path);
    } else if let Some(path) = targeted_env_var("PKG_CONFIG_PATH", target) {
        cmd.env("PKG_CONFIG_PATH", path);
    }
    if let Some(libdir) = targeted_env_var("PKG_CONFIG_LIBDIR", target) {
        cmd.env("PKG_CONFIG_LIBDIR", libdir);
    }
    if let Some(sr) = targeted_env_var("PKG_CONFIG_SYSROOT_DIR", target) {
        cmd.env("PKG_CONFIG_SYSROOT_DIR", sr);
    }

    cmd
}

/// The `.pc` search path inside a cross sysroot, joined for `PKG_CONFIG_LIBDIR`
/// / `PKG_CONFIG_PATH`. Covers the standard `usr/lib`, `usr/share`, and the
/// common Debian multiarch dirs (harmless when absent).
fn sysroot_pkgconfig_path(sysroot: &Path) -> std::ffi::OsString {
    let candidates = [
        sysroot.join("usr/lib/pkgconfig"),
        sysroot.join("usr/lib/x86_64-linux-gnu/pkgconfig"),
        sysroot.join("usr/lib/aarch64-linux-gnu/pkgconfig"),
        sysroot.join("usr/lib/arm-linux-gnueabihf/pkgconfig"),
        sysroot.join("usr/share/pkgconfig"),
        sysroot.join("usr/local/lib/pkgconfig"),
    ];
    let joined = candidates
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");
    std::ffi::OsString::from(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysroot_pkgconfig_path_lists_standard_dirs() {
        let p = sysroot_pkgconfig_path(Path::new("/opt/root"));
        let s = p.to_string_lossy();
        assert!(s.contains("/opt/root/usr/lib/pkgconfig"), "{s}");
        assert!(s.contains("/opt/root/usr/share/pkgconfig"), "{s}");
        // Restricting to the sysroot means the host root must not appear.
        assert!(!s.split(':').any(|d| d == "/usr/lib/pkgconfig"), "{s}");
    }
}
