//! Foreign build system integration: cmake, make, meson.
//!
//! When a dependency declares `build_system = "cmake"` (or similar), crane
//! delegates compilation to that tool and links the resulting libraries instead
//! of trying to compile the dep's sources itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::CraneError;
use crate::manifest::types::{Dependency, Manifest};

// ── Public types ──────────────────────────────────────────────────────────────

/// Output of a foreign dep build: library archives to link + include dirs.
pub struct ForeignBuilt {
    pub name: String,
    pub libs: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build all foreign deps declared in `manifest` and return their link artifacts.
pub fn build_foreign_deps(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
) -> Result<Vec<ForeignBuilt>, CraneError> {
    let mut results = Vec::new();

    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else { continue };

        let dep_dir = if let Some(rel) = &d.path {
            project_dir.join(rel)
        } else if d.git.is_some() {
            project_dir.join(".deps").join(name)
        } else {
            continue;
        };

        // Resolve effective build system: explicit > auto-detect > skip (crane project).
        // For path deps that have a crane.toml, crane owns the build regardless.
        let bs = match &d.build_system {
            Some(bs) => bs.clone(),
            None => {
                if d.path.is_some() && dep_dir.join("crane.toml").exists() {
                    continue;
                }
                match detect_build_system(&dep_dir) {
                    Some(detected) => detected,
                    None => continue,
                }
            }
        };

        if !dep_dir.exists() {
            return Err(CraneError::ManifestParse(format!(
                "foreign dep '{name}' not found at '{}' — run `crane fetch` first",
                dep_dir.display()
            )));
        }

        let libs = invoke_build_system(&dep_dir, name, &bs, profile, &d.cmake_args)?;

        // Explicit `include = [...]` wins; if absent, probe common conventions.
        let include_dirs: Vec<PathBuf> = if !d.include.is_empty() {
            d.include.iter().map(|p| dep_dir.join(p)).collect()
        } else {
            ["include", "inc"]
                .iter()
                .map(|p| dep_dir.join(p))
                .filter(|p| p.is_dir())
                .collect()
        };

        results.push(ForeignBuilt { name: name.clone(), libs, include_dirs });
    }

    Ok(results)
}

// ── Build system dispatch ─────────────────────────────────────────────────────

fn invoke_build_system(
    dep_dir: &Path,
    name: &str,
    build_system: &str,
    profile: &str,
    cmake_args: &[String],
) -> Result<Vec<PathBuf>, CraneError> {
    let resolved = if build_system == "auto" {
        detect_build_system(dep_dir).ok_or_else(|| {
            CraneError::ManifestParse(format!(
                "cannot auto-detect build system for foreign dep '{name}'"
            ))
        })?
    } else {
        build_system.to_string()
    };

    let build_dir = dep_dir.join(".crane-build");
    std::fs::create_dir_all(&build_dir)?;

    use owo_colors::OwoColorize;
    println!("  {} {} ({})", "Building".dimmed(), name, resolved);

    let search_dir = match resolved.as_str() {
        "cmake" => { build_cmake(dep_dir, &build_dir, profile, cmake_args)?; build_dir }
        "make"  => { build_make(dep_dir)?; dep_dir.to_path_buf() }
        "meson" => { build_meson(dep_dir, &build_dir)?; build_dir }
        other => {
            return Err(CraneError::ManifestParse(format!(
                "unknown build_system '{other}' for '{name}'; \
                 expected: cmake, make, meson, auto"
            )));
        }
    };

    find_libs(&search_dir)
}

pub(crate) fn detect_build_system(dep_dir: &Path) -> Option<String> {
    if dep_dir.join("CMakeLists.txt").exists() { return Some("cmake".into()); }
    if dep_dir.join("meson.build").exists()    { return Some("meson".into()); }
    if dep_dir.join("Makefile").exists() || dep_dir.join("GNUmakefile").exists() {
        return Some("make".into());
    }
    None
}

// ── Individual build system runners ──────────────────────────────────────────

fn build_cmake(dep_dir: &Path, build_dir: &Path, profile: &str, extra_args: &[String]) -> Result<(), CraneError> {
    let build_type = if profile == "release" { "Release" } else { "Debug" };

    let src   = dep_dir.to_string_lossy().into_owned();
    let bdir  = build_dir.to_string_lossy().into_owned();
    let btype = format!("-DCMAKE_BUILD_TYPE={build_type}");

    let mut configure_args: Vec<&str> = vec!["-S", &src, "-B", &bdir, &btype];
    for a in extra_args { configure_args.push(a.as_str()); }

    run("cmake", &configure_args, dep_dir, "cmake configure")?;
    run("cmake", &["--build", &bdir], dep_dir, "cmake build")
}

fn build_make(dep_dir: &Path) -> Result<(), CraneError> {
    run("make", &[], dep_dir, "make")
}

fn build_meson(dep_dir: &Path, build_dir: &Path) -> Result<(), CraneError> {
    if !build_dir.join("build.ninja").exists() {
        run("meson", &[
            "setup",
            &build_dir.to_string_lossy(),
            &dep_dir.to_string_lossy(),
        ], dep_dir, "meson setup")?;
    }
    run("ninja", &["-C", &build_dir.to_string_lossy()], dep_dir, "ninja")
}

fn run(prog: &str, args: &[&str], cwd: &Path, label: &str) -> Result<(), CraneError> {
    let status = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| CraneError::CompilerNotFound(format!("{prog} not found: {e}")))?;

    if !status.success() {
        return Err(CraneError::CompileFailed(
            label.to_string(),
            format!("{prog} exited with status {}", status.code().unwrap_or(-1)),
        ));
    }
    Ok(())
}

// ── Output discovery ──────────────────────────────────────────────────────────

fn find_libs(search_dir: &Path) -> Result<Vec<PathBuf>, CraneError> {
    let mut libs = Vec::new();
    for entry in walkdir::WalkDir::new(search_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if matches!(ext, "a" | "so" | "dylib") {
                libs.push(path.to_path_buf());
            }
        }
    }
    libs.sort();
    Ok(libs)
}
