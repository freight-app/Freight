//! C++23 standard-library module (`import std;`) support.
//!
//! `import std;` / `import std.compat;` need the standard library compiled as a
//! module: a BMI (`.pcm`) built from the toolchain's std module source, passed to
//! every translation unit that imports it via `-fmodule-file=std=<bmi>`. The
//! compiler does **not** do this automatically, so neither a plain `freight
//! build` nor clangd can resolve `import std;` without help. This module:
//!
//! 1. Locates the toolchain's module manifest (`libstdc++.modules.json` /
//!    `libc++.modules.json`) via `<compiler> -print-file-name=`.
//! 2. Precompiles the requested std modules into cached BMIs.
//! 3. Returns the `-fmodule-file=<name>=<bmi>` flags to inject.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A built standard-library module BMI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdModuleBmi {
    pub logical_name: String,
    pub bmi: PathBuf,
}

/// One module entry from a `*.modules.json` manifest.
struct ManifestModule {
    logical_name: String,
    source: PathBuf,
}

/// Locate the toolchain's std-module manifest for `compiler`, returning its
/// parsed modules (with absolute, existing source paths). Tries libstdc++ then
/// libc++. Returns `None` when the toolchain ships no std module.
fn manifest_modules(compiler: &Path) -> Option<Vec<ManifestModule>> {
    for name in ["libstdc++.modules.json", "libc++.modules.json"] {
        let out = Command::new(compiler)
            .arg(format!("-print-file-name={name}"))
            .output()
            .ok()?;
        let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let manifest = PathBuf::from(&printed);
        // `-print-file-name` echoes the bare name when the file is not found.
        if !manifest.is_absolute() || !manifest.is_file() {
            continue;
        }
        let dir = manifest.parent().unwrap_or(Path::new("."));
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let mut modules = Vec::new();
        for m in json
            .get("modules")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let (Some(logical), Some(src)) = (
                m.get("logical-name").and_then(|v| v.as_str()),
                m.get("source-path").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let source = dir.join(src);
            if let Ok(canon) = source.canonicalize() {
                modules.push(ManifestModule {
                    logical_name: logical.to_string(),
                    source: canon,
                });
            }
        }
        if !modules.is_empty() {
            return Some(modules);
        }
    }
    None
}

/// Ensure the requested std modules are built and return `-fmodule-file=` flags
/// for them.
///
/// * `wanted` — logical names the sources import (e.g. `["std"]`,
///   `["std", "std.compat"]`).
/// * `std_flag` — the `-std=` value (e.g. `"c++23"`).
/// * `cache_dir` — where BMIs are cached (created if absent).
///
/// Returns an empty vector when the toolchain has no std module or a build
/// fails — callers then behave as before (clangd reports the module as missing,
/// which is the honest state).
pub fn module_file_flags(
    compiler: &Path,
    std_flag: &str,
    cache_dir: &Path,
    wanted: &[&str],
) -> Vec<String> {
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(manifest) = manifest_modules(compiler) else {
        return Vec::new();
    };
    let available: HashMap<&str, &Path> = manifest
        .iter()
        .map(|m| (m.logical_name.as_str(), m.source.as_path()))
        .collect();

    // `std.compat` imports `std`, so build `std` first when both are requested.
    let mut order: Vec<&str> = Vec::new();
    if wanted.contains(&"std") {
        order.push("std");
    }
    for w in wanted {
        if *w != "std" {
            order.push(w);
        }
    }

    let _ = std::fs::create_dir_all(cache_dir);
    let mut built: Vec<StdModuleBmi> = Vec::new();
    for name in order {
        let Some(source) = available.get(name) else {
            continue;
        };
        let bmi = cache_dir.join(format!("{name}.pcm"));
        if needs_rebuild(&bmi, source) {
            if !precompile(compiler, std_flag, name, source, &bmi, &built) {
                continue;
            }
        }
        built.push(StdModuleBmi {
            logical_name: name.to_string(),
            bmi,
        });
    }

    built
        .iter()
        .map(|m| format!("-fmodule-file={}={}", m.logical_name, m.bmi.display()))
        .collect()
}

/// True when `bmi` is missing or older than its `source`.
fn needs_rebuild(bmi: &Path, source: &Path) -> bool {
    let (Ok(bm), Ok(sm)) = (
        std::fs::metadata(bmi).and_then(|m| m.modified()),
        std::fs::metadata(source).and_then(|m| m.modified()),
    ) else {
        return true;
    };
    bm < sm
}

/// Precompile a single std module BMI. Returns false on failure.
fn precompile(
    compiler: &Path,
    std_flag: &str,
    _name: &str,
    source: &Path,
    bmi: &Path,
    deps: &[StdModuleBmi],
) -> bool {
    let mut cmd = Command::new(compiler);
    cmd.arg(format!("-std={std_flag}"))
        .arg("-Wno-reserved-module-identifier")
        .arg("--precompile")
        .arg("-x")
        .arg("c++-module")
        .arg(source)
        .arg("-o")
        .arg(bmi);
    // A module that imports an already-built one (std.compat -> std) needs its BMI.
    for d in deps {
        cmd.arg(format!(
            "-fmodule-file={}={}",
            d.logical_name,
            d.bmi.display()
        ));
    }
    matches!(cmd.status(), Ok(s) if s.success()) && bmi.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_modules_requested_yields_no_flags() {
        let flags = module_file_flags(
            Path::new("c++"),
            "c++23",
            std::env::temp_dir().as_path(),
            &[],
        );
        assert!(flags.is_empty());
    }

    // A real end-to-end build is exercised by the LSP/build integration; this
    // unit test only guards the empty-request fast path so it stays toolchain-
    // independent.
}
