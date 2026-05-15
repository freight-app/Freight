//! Foreign build system integration: detection, dispatch, and output collection.
//!
//! Each submodule owns one build system. This module holds the orchestrator
//! (`build_foreign_deps`), shared types, detection logic, and helpers that all
//! builders use.

pub mod autotools;
pub mod bazel;
pub mod cmake;
pub mod conan;
pub mod make;
pub mod meson;
pub mod pkg_config;
pub mod scons;
pub mod system_pm;

pub use pkg_config::{PkgConfigResult, ResolvedPkgConfig, pkg_config_query, pkg_config_version};

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::{Dependency, Manifest};
use crate::toolchain::system_libs::{find_stub, load_system_lib_stubs};

// ── Public types ──────────────────────────────────────────────────────────────

/// Output of a foreign dep build: library archives to link + include dirs.
pub struct ForeignBuilt {
    pub name: String,
    pub libs: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    /// Raw linker flags (e.g. `-pthread`, `-L/usr/lib`, `-lfoo`) from pkg-config.
    pub raw_link_flags: Vec<String>,
}

// ── Orchestrator ──────────────────────────────────────────────────────────────

/// Build all foreign deps declared in `manifest` and return their link artifacts
/// alongside the resolved pkg-config results.
pub fn build_foreign_deps(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    progress: &Progress,
) -> Result<(Vec<ForeignBuilt>, Vec<ResolvedPkgConfig>), FreightError> {
    let mut results: Vec<ForeignBuilt> = Vec::new();
    let mut pkg_results: Vec<ResolvedPkgConfig> = Vec::new();

    for (name, dep) in &manifest.dependencies {
        if let Some(version) = package_dep_version(dep) {
            let query    = package_query(name, version);
            let repo     = dep_repo(dep);
            let optional = package_dep_optional(dep);
            match resolve_version_dep(name, &query, version, repo, optional, project_dir, progress)? {
                Some((built, maybe_pc)) => {
                    if let Some(pc) = maybe_pc { pkg_results.push(pc); }
                    results.push(built);
                }
                None => {} // optional, not found
            }
            continue;
        }

        let Dependency::Detailed(d) = dep else { continue };

        // ── Pure system dep — -l{name} handled by linker, nothing to build ───
        if d.system.is_some() {
            continue;
        }

        // ── Determine source directory ────────────────────────────────────────
        let dep_dir = if let Some(rel) = &d.path {
            project_dir.join(rel)
        } else if d.git.is_some() {
            project_dir.join(".deps").join(name)
        } else if let Some(url) = &d.url {
            crate::fetch::http::fetch_url_dep(name, url, d.sha256.as_deref(), project_dir, progress)?
        } else {
            continue; // version dep — not a foreign build
        };

        if !dep_dir.exists() {
            return Err(FreightError::ManifestParse(format!(
                "foreign dep '{name}' not found at '{}' — run `freight fetch` first",
                dep_dir.display()
            )));
        }

        // ── Resolve build system ──────────────────────────────────────────────
        let bs = match &d.backend {
            Some(bs) if bs == "none" => {
                let include_dirs = collect_include_dirs(&dep_dir, &d.include, None);
                results.push(ForeignBuilt {
                    name: name.clone(), libs: vec![],
                    include_dirs, raw_link_flags: vec![],
                });
                continue;
            }
            Some(bs) => {
                validate_backend(name, bs, &dep_dir)?;
                bs.clone()
            }
            None => {
                if d.path.is_some() && dep_dir.join("freight.toml").exists() {
                    continue;
                }
                match detect_build_system(&dep_dir) {
                    Some(detected) => detected,
                    None => {
                        if !has_source_files(&dep_dir) {
                            let include_dirs =
                                collect_include_dirs(&dep_dir, &d.include, None);
                            if !include_dirs.is_empty() {
                                results.push(ForeignBuilt {
                                    name: name.clone(), libs: vec![],
                                    include_dirs, raw_link_flags: vec![],
                                });
                            }
                        }
                        continue;
                    }
                }
            }
        };

        let build_dir = dep_dir.join(".freight-build");
        let libs = invoke_build_system(&dep_dir, &build_dir, name, &bs, profile, &d.cmake_args, manifest.compiler.target.as_deref(), progress)?;
        let include_dirs = collect_include_dirs(&dep_dir, &d.include, Some(&build_dir));

        results.push(ForeignBuilt {
            name: name.clone(), libs, include_dirs, raw_link_flags: vec![],
        });
    }

    Ok((results, pkg_results))
}

fn package_dep_version(dep: &Dependency) -> Option<&str> {
    match dep {
        Dependency::Simple(version) => Some(version.as_str()),
        Dependency::Detailed(d)
            if d.version.is_some()
                && d.path.is_none()
                && d.system.is_none()
                && d.git.is_none()
                && d.url.is_none() => d.version.as_deref(),
        _ => None,
    }
}

fn package_dep_optional(dep: &Dependency) -> bool {
    matches!(dep, Dependency::Detailed(d) if d.optional)
}

fn dep_repo(dep: &Dependency) -> Option<&str> {
    if let Dependency::Detailed(d) = dep { d.repo.as_deref() } else { None }
}

fn package_query(name: &str, version: &str) -> String {
    let version = version.trim();
    if version.is_empty() || version == "*" {
        return name.to_string();
    }
    if matches!(version.as_bytes().first(), Some(b'<' | b'>' | b'=' | b'!')) {
        format!("{name} {version}")
    } else {
        format!("{name} >= {version}")
    }
}

// ── Version dep resolution chain ─────────────────────────────────────────────

/// Resolve a version dep (`name = "1.3"` or `{ version = "1.3", repo = "..." }`)
/// through the configured resolver chain.
///
/// Returns `Ok(Some((built, maybe_pc)))` on success, `Ok(None)` when the dep
/// is optional and not found, or `Err` when the dep is required and all
/// resolvers fail.
///
/// Default chain (no explicit `repo`): pkg-config → conan → vcpkg
/// If all fail and a system PM is detectable, a helpful install hint is emitted
/// as a `BuildEvent::Warning` before returning the error.
fn resolve_version_dep(
    name: &str,
    query: &str,
    version: &str,
    repo: Option<&str>,
    optional: bool,
    project_dir: &Path,
    progress: &Progress,
) -> Result<Option<(ForeignBuilt, Option<ResolvedPkgConfig>)>, FreightError> {
    match repo {
        Some("conan") => {
            match conan::resolve_conan_dep(name, name, version, project_dir, progress) {
                Ok(built) => Ok(Some((built, None))),
                Err(e) if optional => {
                    progress(BuildEvent::Warning(format!("'{name}' not found via conan (optional, skipping): {e}")));
                    Ok(None)
                }
                Err(e) => Err(e),
            }
        }
        Some("vcpkg") => {
            match crate::fetch::vcpkg::resolve_vcpkg_dep(name, name, None, project_dir, progress) {
                Ok(v) => Ok(Some((
                    ForeignBuilt { name: name.to_string(), libs: v.libs, include_dirs: v.include_dirs, raw_link_flags: v.raw_link_flags },
                    None,
                ))),
                Err(e) if optional => {
                    progress(BuildEvent::Warning(format!("'{name}' not found via vcpkg (optional, skipping): {e}")));
                    Ok(None)
                }
                Err(e) => Err(e),
            }
        }
        Some("system") => {
            // Bypass all resolvers and link the library directly.
            let stubs = load_system_lib_stubs();
            let link_flag = if let Some(stub) = find_stub(name, &stubs) {
                format!("-l{}", stub.link_name)
            } else {
                format!("-l{name}")
            };
            progress(BuildEvent::ResolvingDep { name: name.to_string(), via: "system".to_string() });
            Ok(Some((
                ForeignBuilt { name: name.to_string(), libs: vec![], include_dirs: vec![], raw_link_flags: vec![link_flag] },
                None,
            )))
        }
        Some(other) => Err(FreightError::ManifestParse(format!(
            "unknown repo '{other}' for dep '{name}'; accepted: conan, vcpkg, system"
        ))),
        None => {
            // Default build-time chain: pkg-config → conan → system-lib stubs.
            // vcpkg is only used when repo = "vcpkg" is explicit; it is not
            // tried automatically so that the freight registry remains the
            // canonical source of truth for package resolution.
            progress(BuildEvent::ResolvingDep { name: name.to_string(), via: query.to_string() });
            if let Ok(pc) = pkg_config_query(query) {
                let ver = pkg_config_version(query);
                return Ok(Some((
                    ForeignBuilt { name: name.to_string(), libs: vec![], include_dirs: pc.include_dirs.clone(), raw_link_flags: pc.link_flags },
                    Some(ResolvedPkgConfig { name: name.to_string(), found: true, version: ver, include_dirs: pc.include_dirs }),
                )));
            }
            if conan::is_conan_available() {
                if let Ok(built) = conan::resolve_conan_dep(name, name, version, project_dir, progress) {
                    return Ok(Some((built, None)));
                }
            }
            // Final fallback: built-in system-lib stubs (e.g. pthread, ws2_32).
            let stubs = load_system_lib_stubs();
            if let Some(stub) = find_stub(name, &stubs) {
                let link_flag = format!("-l{}", stub.link_name);
                progress(BuildEvent::ResolvingDep { name: name.to_string(), via: "system-lib stub".to_string() });
                return Ok(Some((
                    ForeignBuilt { name: name.to_string(), libs: vec![], include_dirs: vec![], raw_link_flags: vec![link_flag] },
                    None,
                )));
            }
            if let Some(pm) = system_pm::detect() {
                progress(BuildEvent::Warning(format!(
                    "hint: try `{}` to install the system package",
                    pm.install_hint(name),
                )));
            }
            if optional {
                progress(BuildEvent::Warning(format!("'{name}' not found at build time (optional, skipping)")));
                Ok(None)
            } else {
                Err(FreightError::ManifestParse(format!(
                    "dep '{name}' not found via pkg-config, conan, or system stubs; \
                     run `freight fetch` to download it first"
                )))
            }
        }
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_backend(name: &str, backend: &str, dep_dir: &Path) -> Result<(), FreightError> {
    let (present, marker) = match backend {
        "cmake"     => (dep_dir.join("CMakeLists.txt").exists(),     "CMakeLists.txt"),
        "meson"     => (dep_dir.join("meson.build").exists(),        "meson.build"),
        "autotools" => (dep_dir.join("configure.ac").exists()
                     || dep_dir.join("configure.in").exists()
                     || dep_dir.join("autogen.sh").exists()
                     || dep_dir.join("configure").exists(),          "configure.ac / configure"),
        "make"      => (dep_dir.join("Makefile").exists()
                     || dep_dir.join("GNUmakefile").exists(),        "Makefile"),
        "scons"     => (dep_dir.join("SConstruct").exists(),         "SConstruct"),
        "bazel"     => (dep_dir.join("WORKSPACE").exists()
                     || dep_dir.join("WORKSPACE.bazel").exists(),    "WORKSPACE"),
        "auto" | "none" => return Ok(()),
        other => return Err(FreightError::ManifestParse(format!(
            "unknown backend '{other}' for dep '{name}'; \
             expected: cmake, make, meson, autotools, scons, bazel"
        ))),
    };
    if !present {
        return Err(FreightError::ManifestParse(format!(
            "backend '{backend}' specified for dep '{name}' \
             but '{marker}' not found in '{}'",
            dep_dir.display()
        )));
    }
    Ok(())
}

// ── Detection ─────────────────────────────────────────────────────────────────

pub fn detect_build_system(dep_dir: &Path) -> Option<String> {
    if dep_dir.join("CMakeLists.txt").exists() { return Some("cmake".into()); }
    if dep_dir.join("meson.build").exists()    { return Some("meson".into()); }
    // configure.ac / configure.in → autotools (before Makefile: autotools projects
    // may ship a generated Makefile from a prior configure run)
    if dep_dir.join("configure.ac").exists() || dep_dir.join("configure.in").exists() {
        return Some("autotools".into());
    }
    if dep_dir.join("autogen.sh").exists() || dep_dir.join("configure").exists() {
        return Some("autotools".into());
    }
    if dep_dir.join("WORKSPACE").exists() || dep_dir.join("WORKSPACE.bazel").exists() {
        return Some("bazel".into());
    }
    if dep_dir.join("SConstruct").exists() { return Some("scons".into()); }
    if dep_dir.join("Makefile").exists() || dep_dir.join("GNUmakefile").exists() {
        return Some("make".into());
    }
    None
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

fn invoke_build_system(
    dep_dir: &Path,
    build_dir: &Path,
    name: &str,
    build_system: &str,
    profile: &str,
    cmake_args: &[String],
    target: Option<&str>,
    progress: &Progress,
) -> Result<Vec<PathBuf>, FreightError> {
    let resolved = build_system.to_string();

    std::fs::create_dir_all(build_dir)?;

    progress(BuildEvent::BuildingForeignDep { name: name.to_string(), backend: resolved.clone() });

    let search_dir = match resolved.as_str() {
        "cmake"     => { cmake::build_cmake(dep_dir, build_dir, profile, cmake_args, target)?; build_dir.to_path_buf() }
        "make"      => { make::build_make(dep_dir)?; dep_dir.to_path_buf() }
        "meson"     => { meson::build_meson(dep_dir, build_dir)?; build_dir.to_path_buf() }
        "autotools" => { autotools::build_autotools(dep_dir, build_dir, target)?; build_dir.join("install") }
        "scons"     => { scons::build_scons(dep_dir)?; dep_dir.to_path_buf() }
        "bazel"     => { bazel::build_bazel(dep_dir)?; dep_dir.to_path_buf() }
        other => {
            return Err(FreightError::ManifestParse(format!(
                "unknown backend '{other}' for dep '{name}'; \
                 expected: cmake, make, meson, autotools, scons, bazel"
            )));
        }
    };

    find_libs(&search_dir)
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Shared helper used by all builder submodules.
pub(crate) fn run(prog: &str, args: &[&str], cwd: &Path, label: &str) -> Result<(), FreightError> {
    let status = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| FreightError::CompilerNotFound(format!("{prog} not found: {e}")))?;

    if !status.success() {
        return Err(FreightError::CompileFailed(
            label.to_string(),
            format!("{prog} exited with status {}", status.code().unwrap_or(-1)),
        ));
    }
    Ok(())
}

/// Resolve include directories for a dep.
pub(crate) fn collect_include_dirs(
    dep_dir: &Path,
    explicit: &[String],
    build_dir: Option<&Path>,
) -> Vec<PathBuf> {
    if !explicit.is_empty() {
        return explicit.iter().map(|p| dep_dir.join(p)).collect();
    }
    let mut candidates = vec![dep_dir.join("include"), dep_dir.join("inc")];
    if let Some(bd) = build_dir {
        candidates.push(bd.join("install").join("include"));
    }
    candidates.into_iter().filter(|p| p.is_dir()).collect()
}

/// Return `true` if `dir` contains at least one compilable source file.
pub(crate) fn has_source_files(dir: &Path) -> bool {
    const SOURCE_EXTS: &[&str] = &[
        "c", "cpp", "cc", "cxx", "c++", "cppm",
        "f", "f90", "f95", "f03", "f08",
        "s", "asm", "nasm",
        "cu", "hip", "cl",
    ];
    fn walk(dir: &Path, depth: u8) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else { return false };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if depth > 0 && walk(&p, depth - 1) { return true; }
            } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if SOURCE_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
                    return true;
                }
            }
        }
        false
    }
    walk(dir, 4)
}

fn find_libs(search_dir: &Path) -> Result<Vec<PathBuf>, FreightError> {
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
