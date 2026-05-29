//! CMake foreign build system integration.
//!
//! Adapted from cmake-rs. Key features retained:
//! - Generator selection: Ninja preferred when on PATH, else CMake default
//! - Cross-compilation: `CMAKE_SYSTEM_NAME` / `CMAKE_SYSTEM_PROCESSOR` derived
//!   from the target triple using the same OS×arch table as cmake-rs
//! - `cmake --build --parallel N` on CMake ≥ 3.12
//! - `cmake --install` step (falls back to `--target install` on CMake < 3.15)
use std::path::{Path, PathBuf};
use std::process::Command;

use super::run;
use crate::error::FreightError;

pub fn build_cmake(
    dep_dir: &Path,
    build_dir: &Path,
    profile: &str,
    extra_args: &[String],
    target: Option<&str>,
    tool_paths: &[PathBuf],
) -> Result<(), FreightError> {
    let build_type = if profile == "release" {
        "Release"
    } else {
        "Debug"
    };
    let install_prefix = build_dir.join("install");
    std::fs::create_dir_all(&install_prefix)?;

    let src = dep_dir.to_string_lossy().into_owned();
    let bdir = build_dir.to_string_lossy().into_owned();
    let btype = format!("-DCMAKE_BUILD_TYPE={build_type}");
    let prefix = format!("-DCMAKE_INSTALL_PREFIX={}", install_prefix.display());

    let mut configure_args: Vec<String> =
        vec!["-S".into(), src, "-B".into(), bdir.clone(), btype, prefix];

    if let Some(gen) = select_generator(tool_paths) {
        configure_args.push("-G".into());
        configure_args.push(gen);
    }

    if let Some(triple) = target {
        if let Some((system_name, processor)) = cmake_system_from_triple(triple) {
            configure_args.push(format!("-DCMAKE_SYSTEM_NAME={system_name}"));
            configure_args.push(format!("-DCMAKE_SYSTEM_PROCESSOR={processor}"));
        }
    }

    for a in extra_args {
        configure_args.push(a.clone());
    }

    let refs: Vec<&str> = configure_args.iter().map(String::as_str).collect();
    run("cmake", &refs, dep_dir, "cmake configure", tool_paths)?;

    // Build step.
    let jobs = rayon::current_num_threads();
    let jobs_str = jobs.to_string();
    let cmake_ver = cmake_version(tool_paths);
    let parallel_ok = cmake_ver
        .map(|(maj, min, _)| (maj, min) >= (3, 12))
        .unwrap_or(false);

    if parallel_ok {
        run(
            "cmake",
            &["--build", &bdir, "--parallel", &jobs_str],
            dep_dir,
            "cmake build",
            tool_paths,
        )?;
    } else {
        run(
            "cmake",
            &["--build", &bdir],
            dep_dir,
            "cmake build",
            tool_paths,
        )?;
    }

    // Install step.
    let prefix_str = install_prefix.to_string_lossy().into_owned();
    let install_ok = cmake_ver
        .map(|(maj, min, _)| (maj, min) >= (3, 15))
        .unwrap_or(false);
    if install_ok {
        run(
            "cmake",
            &["--install", &bdir, "--prefix", &prefix_str],
            dep_dir,
            "cmake install",
            tool_paths,
        )?;
    } else {
        run(
            "cmake",
            &["--build", &bdir, "--target", "install"],
            dep_dir,
            "cmake install",
            tool_paths,
        )?;
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `(major, minor, patch)` of the cmake binary reachable via
/// `tool_paths` (tried first) or system PATH.
pub(super) fn cmake_version(tool_paths: &[PathBuf]) -> Option<(u32, u32, u32)> {
    // Try tool_paths first (locally installed cmake), then fall back to PATH.
    let cmake_bin = tool_paths
        .iter()
        .map(|p| p.join("cmake"))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cmake".to_string());

    let out = Command::new(&cmake_bin).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?;
    let ver_str = line.strip_prefix("cmake version ")?.trim();
    let mut parts = ver_str.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

fn select_generator(tool_paths: &[PathBuf]) -> Option<String> {
    // Prefer Ninja for faster incremental builds.
    // Check tool_paths first (locally installed ninja), then system PATH.
    let ninja_bin = tool_paths
        .iter()
        .map(|p| p.join("ninja"))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ninja".to_string());

    if Command::new(&ninja_bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some("Ninja".into());
    }
    None
}

/// Map a compiler target triple to (CMAKE_SYSTEM_NAME, CMAKE_SYSTEM_PROCESSOR).
/// Mirrors the mapping table used in cmake-rs.
fn cmake_system_from_triple(triple: &str) -> Option<(&'static str, &'static str)> {
    if triple.contains("linux") {
        let proc = if triple.starts_with("aarch64") {
            "aarch64"
        } else if triple.starts_with("arm") {
            "arm"
        } else if triple.starts_with("riscv64") {
            "riscv64"
        } else if triple.starts_with("i686") || triple.starts_with("i386") {
            "x86"
        } else {
            "x86_64"
        };
        return Some(("Linux", proc));
    }
    if triple.contains("darwin") || triple.contains("apple") {
        let proc = if triple.starts_with("aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        return Some(("Darwin", proc));
    }
    if triple.contains("windows") {
        let proc = if triple.starts_with("aarch64") {
            "ARM64"
        } else if triple.starts_with("i686") || triple.starts_with("i386") {
            "X86"
        } else {
            "AMD64"
        };
        return Some(("Windows", proc));
    }
    if triple.contains("wasm") || triple.contains("emscripten") {
        return Some(("Emscripten", "asm.js"));
    }
    if triple.contains("android") {
        let proc = if triple.starts_with("aarch64") {
            "aarch64"
        } else if triple.starts_with("arm") {
            "arm"
        } else {
            "x86_64"
        };
        return Some(("Android", proc));
    }
    None
}
