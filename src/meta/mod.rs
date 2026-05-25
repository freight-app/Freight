//! Foreign build system integration: detection, dispatch, and output collection.
//!
//! Each submodule owns one build system. This module holds the orchestrator
//! (`build_foreign_deps`), shared types, detection logic, and helpers that all
//! builders use.

pub mod autotools;
pub mod bazel;
pub mod cmake;
pub mod make;
pub mod meson;
pub mod pkg_config;
pub mod pkg_config_cache;
pub mod scons;
pub mod system_pm;

pub use pkg_config::{PkgConfigResult, ResolvedPkgConfig, pkg_config_query, pkg_config_query_with_path, pkg_config_version};
pub use pkg_config_cache::PkgConfigCache;

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::{Dependency, DetailedDep, Manifest};
use crate::supports::eval_supports;
use crate::toolchain::system_libs::{find_stub, load_system_lib_stubs, SystemLibStub};

/// Hard cap on parallel jobs passed to foreign build systems (cmake, make, ninja, …).
/// Prevents saturating all cores when building dependencies.
pub(crate) const MAX_JOBS: usize = 6;

// ── OS-family pseudo-deps ─────────────────────────────────────────────────────

/// Dep names that are treated as OS-family selectors rather than real packages.
/// `windows = { features = ["ws2_32", "kernel32"] }` → link those libs on Windows.
const OS_FAMILIES: &[&str] = &[
    "windows", "linux", "macos", "osx", "unix", "bsd",
    "freebsd", "openbsd", "netbsd", "dragonfly",
    "android", "ios", "solaris", "illumos",
];

fn expand_os_family_dep(name: &str, d: &DetailedDep, all_stubs: &[SystemLibStub]) -> Vec<ForeignBuilt> {
    if !eval_supports(name) {
        return vec![];
    }
    d.features.iter().map(|feat| {
        let link_flag = if let Some(stub) = find_stub(feat, all_stubs) {
            format!("-l{}", stub.link_name)
        } else {
            format!("-l{feat}")
        };
        ForeignBuilt {
            name: feat.clone(),
            libs: vec![],
            include_dirs: vec![],
            raw_link_flags: vec![link_flag],
        }
    }).collect()
}

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
    let mut pc_cache = PkgConfigCache::load(project_dir);

    let all_stubs = load_system_lib_stubs();

    // Collect OS-family deps first so we can deduplicate across families.
    // e.g. `unix = { features = ["pthread"] }` + `linux = { features = ["pthread", "rt"] }`
    // should only produce one -lpthread on Linux.
    let mut os_link_flags: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (name, dep) in &manifest.dependencies {
        if OS_FAMILIES.contains(&name.as_str()) {
            if let Dependency::Detailed(d) = dep {
                for built in expand_os_family_dep(name, d, &all_stubs) {
                    for flag in built.raw_link_flags {
                        if os_link_flags.insert(flag.clone()) {
                            results.push(ForeignBuilt {
                                name: built.name.clone(),
                                libs: vec![],
                                include_dirs: vec![],
                                raw_link_flags: vec![flag],
                            });
                        }
                    }
                }
            }
        }
    }

    for (name, dep) in &manifest.dependencies {
        if OS_FAMILIES.contains(&name.as_str()) { continue; }

        if let Some(version) = package_dep_version(dep) {
            let query    = package_query(name, version);
            let repo     = dep_repo(dep);
            let optional = package_dep_optional(dep);
            match resolve_version_dep(name, &query, version, repo, optional, project_dir, progress, &mut pc_cache)? {
                Some((built, maybe_pc)) => {
                    if let Some(pc) = maybe_pc { pkg_results.push(pc); }
                    results.push(built);
                }
                None => {} // optional, not found
            }
            continue;
        }

        let Dependency::Detailed(d) = dep else { continue };

        // ── Platform dep — link flags handled by linker, nothing to build ───
        if crate::manifest::types::is_platform_dep(name) {
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

    pc_cache.save(project_dir);
    Ok((results, pkg_results))
}

fn package_dep_version(dep: &Dependency) -> Option<&str> {
    match dep {
        Dependency::Simple(version) => Some(version.as_str()),
        Dependency::Detailed(d)
            if d.version.is_some()
                && d.path.is_none()
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
/// Default chain (no explicit `repo`): pkg-config → system-lib stubs → .deps cache.
fn resolve_version_dep(
    name: &str,
    query: &str,
    version: &str,
    repo: Option<&str>,
    optional: bool,
    project_dir: &Path,
    progress: &Progress,
    pc_cache: &mut PkgConfigCache,
) -> Result<Option<(ForeignBuilt, Option<ResolvedPkgConfig>)>, FreightError> {
    // Suppress unused warning; version may be used by future resolvers.
    let _ = version;

    match repo {
        Some("system") => {
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
        Some(_registry_name) => {
            // Named registry dep (`repo = "myregistry"` or `@registry/name` shorthand):
            // fetched by `freight fetch`; resolve at build time the same as a plain
            // version dep — pkg-config first, then system-lib stubs.
            resolve_version_dep(name, query, version, None, optional, project_dir, progress, pc_cache)
        }
        None => {
            // Default chain: pkg-config (cached) → system-lib stubs → .deps/ cache.
            progress(BuildEvent::ResolvingDep { name: name.to_string(), via: query.to_string() });
            if let Ok((pc, ver)) = pc_cache.query(query) {
                return Ok(Some((
                    ForeignBuilt { name: name.to_string(), libs: vec![], include_dirs: pc.include_dirs.clone(), raw_link_flags: pc.link_flags },
                    Some(ResolvedPkgConfig { name: name.to_string(), found: true, version: ver, include_dirs: pc.include_dirs }),
                )));
            }
            // Built-in system-lib stubs (e.g. pthread, ws2_32).
            let stubs = load_system_lib_stubs();
            if let Some(stub) = find_stub(name, &stubs) {
                let link_flag = format!("-l{}", stub.link_name);
                progress(BuildEvent::ResolvingDep { name: name.to_string(), via: "system-lib stub".to_string() });
                return Ok(Some((
                    ForeignBuilt { name: name.to_string(), libs: vec![], include_dirs: vec![], raw_link_flags: vec![link_flag] },
                    None,
                )));
            }
            // .deps/ cache — populated by `freight fetch` from the registry.
            // Check for a .pc file first; fall back to bare include/ + lib/ layout.
            let dep_dir = project_dir.join(".deps").join(name);
            if dep_dir.join(".freight-fetched").exists() {
                progress(BuildEvent::ResolvingDep { name: name.to_string(), via: ".deps (registry fetch)".to_string() });

                // Try pkg-config if the dep ships a .pc file.
                let pc_dir = dep_dir.join("lib").join("pkgconfig");
                if pc_dir.is_dir() {
                    if let Ok((pc, ver)) = pc_cache.query_with_path(query, &[pc_dir]) {
                        return Ok(Some((
                            ForeignBuilt { name: name.to_string(), libs: vec![], include_dirs: pc.include_dirs.clone(), raw_link_flags: pc.link_flags },
                            Some(ResolvedPkgConfig { name: name.to_string(), found: true, version: ver, include_dirs: pc.include_dirs }),
                        )));
                    }
                }

                // No .pc file — collect include/ and lib/ dirs directly.
                let mut include_dirs: Vec<PathBuf> = Vec::new();
                for candidate in &["include", "inc"] {
                    let d = dep_dir.join(candidate);
                    if d.is_dir() { include_dirs.push(d); }
                }
                let mut link_flags: Vec<String> = Vec::new();
                let lib_dir = dep_dir.join("lib");
                if lib_dir.is_dir() {
                    link_flags.push(format!("-L{}", lib_dir.display()));
                    // Link any static archives or shared libs found there.
                    if let Ok(entries) = std::fs::read_dir(&lib_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                            let ext  = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                            if matches!(ext, "a" | "so" | "dylib" | "lib") {
                                let lname = stem.strip_prefix("lib").unwrap_or(stem);
                                link_flags.push(format!("-l{lname}"));
                            }
                        }
                    }
                }

                return Ok(Some((
                    ForeignBuilt { name: name.to_string(), libs: vec![], include_dirs, raw_link_flags: link_flags },
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
                    "dep '{name}' not found via pkg-config or system stubs; \
                     run `freight fetch` to download it from the registry first"
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
