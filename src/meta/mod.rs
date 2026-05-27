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
use rayon::prelude::*;

use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::{Dependency, DetailedDep, Manifest};
use crate::supports::eval_supports;
use crate::toolchain::system_libs::{find_stub, load_system_lib_stubs, SystemLibStub};

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
/// A foreign dep that needs a subprocess build (cmake, make, meson, …).
/// Collected in the sequential pass then dispatched in parallel.
struct BuildJob {
    name:       String,
    dep_dir:    PathBuf,
    backend:    String,
    cmake_args: Vec<String>,
    include:    Vec<String>,
    target:     Option<String>,
    /// Tool bin dirs accumulated from build-deps built before this job.
    tool_paths: Vec<PathBuf>,
}

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

    // ── Build-dependency pass (sequential, before everything else) ────────────
    // Build-deps are tools (cmake, ninja, protoc, …).  We build them first and
    // collect any executable `bin/` directories they install.  Those paths are
    // then prepended to PATH for every subsequent build step so freight-installed
    // tools take precedence over system ones.
    let mut tool_paths: Vec<PathBuf> = Vec::new();

    for (name, dep) in &manifest.build_dependencies {
        let dep_dir = match build_dep_dir(name, dep, project_dir) {
            Some(d) => d,
            None    => continue,
        };
        if !dep_dir.exists() {
            progress(BuildEvent::Warning(format!(
                "build-dep '{name}' not found at '{}' — run `freight fetch` first",
                dep_dir.display()
            )));
            continue;
        }

        // Determine the backend (same logic as regular deps but build-deps rarely
        // need compilation — most are prebuilt binary tarballs with build = "none").
        let backend = if let Dependency::Detailed(d) = dep {
            match d.backend.as_deref() {
                Some("none") | None if !dep_dir.join("CMakeLists.txt").exists()
                                     && !dep_dir.join("Makefile").exists()
                                     && !dep_dir.join("meson.build").exists() => None,
                Some(bs) => Some(bs.to_string()),
                None     => detect_build_system(&dep_dir),
            }
        } else {
            detect_build_system(&dep_dir)
        };

        if let Some(bs) = backend {
            let build_dir = dep_dir.join(".freight-build");
            invoke_build_system(
                &dep_dir, &build_dir, name, &bs,
                profile, &[], None, progress, &tool_paths,
            )?;
        }

        let new_bins = collect_bin_dirs(&dep_dir);
        if !new_bins.is_empty() {
            progress(BuildEvent::Warning(format!(
                "build-dep '{name}': using local executables from {}",
                new_bins.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            )));
        }
        tool_paths.extend(new_bins);
    }

    // ── Sequential pass: fast deps + build-job collection ────────────────────
    // Slow deps (those that call into cmake/make/ninja) are staged in `jobs`
    // and built concurrently in the parallel pass below.

    // OS-family deps first — deduplicate link flags across families.
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

    let mut jobs: Vec<BuildJob> = Vec::new();

    for (name, dep) in &manifest.dependencies {
        if OS_FAMILIES.contains(&name.as_str()) { continue; }

        if let Some(version) = package_dep_version(dep) {
            // Check for a metadata-only registry dep that was fetched from upstream source.
            // `fetch_registry_deps()` writes `.freight-build-system` when the dep needs
            // compiling from source (e.g. vcpkg packages with `build = "cmake"`).
            let dep_dir = project_dir.join(".deps").join(name);
            let bs_file = dep_dir.join(".freight-build-system");
            if bs_file.exists() {
                let bs = std::fs::read_to_string(&bs_file)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "cmake".to_string());
                jobs.push(BuildJob {
                    name:       name.clone(),
                    dep_dir,
                    backend:    bs,
                    cmake_args: vec![],
                    include:    vec![],
                    target:     manifest.compiler.target.clone(),
                    tool_paths: tool_paths.clone(),
                });
            } else {
                let query    = package_query(name, version);
                let repo     = dep_repo(dep);
                let optional = package_dep_optional(dep);
                match resolve_version_dep(name, &query, version, repo, optional, project_dir, progress, &mut pc_cache)? {
                    Some((built, maybe_pc)) => {
                        if let Some(pc) = maybe_pc { pkg_results.push(pc); }
                        results.push(built);
                    }
                    None => {}
                }
            }
            continue;
        }

        let Dependency::Detailed(d) = dep else { continue };

        if crate::manifest::types::is_platform_dep(name) { continue; }

        let dep_dir = if let Some(rel) = &d.path {
            project_dir.join(rel)
        } else if d.git.is_some() {
            project_dir.join(".deps").join(name)
        } else if let Some(url) = &d.url {
            crate::fetch::http::fetch_url_dep(name, url, d.sha256.as_deref(), project_dir, progress)?
        } else {
            continue;
        };

        if !dep_dir.exists() {
            return Err(FreightError::ManifestParse(format!(
                "foreign dep '{name}' not found at '{}' — run `freight fetch` first",
                dep_dir.display()
            )));
        }

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
                            let include_dirs = collect_include_dirs(&dep_dir, &d.include, None);
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

        jobs.push(BuildJob {
            name:       name.clone(),
            dep_dir,
            backend:    bs,
            cmake_args: d.cmake_args.clone(),
            include:    d.include.clone(),
            target:     manifest.compiler.target.clone(),
            tool_paths: tool_paths.clone(),
        });
    }

    // ── Parallel pass: invoke foreign build systems concurrently ─────────────
    // progress is Arc<dyn Fn + Send + Sync> so it is safe to share across threads.
    let built: Result<Vec<ForeignBuilt>, FreightError> = jobs
        .into_par_iter()
        .map(|job| {
            let build_dir = job.dep_dir.join(".freight-build");
            let libs = invoke_build_system(
                &job.dep_dir, &build_dir, &job.name, &job.backend,
                profile, &job.cmake_args, job.target.as_deref(), progress,
                &job.tool_paths,
            )?;
            let include_dirs = collect_include_dirs(&job.dep_dir, &job.include, Some(&build_dir));
            Ok(ForeignBuilt {
                name: job.name,
                libs,
                include_dirs,
                raw_link_flags: vec![],
            })
        })
        .collect();

    results.extend(built?);

    pc_cache.save(project_dir);
    Ok((results, pkg_results))
}

/// Resolve the on-disk directory for a build-dep entry.
/// Same logic as regular deps: path → join project_dir; git/url/version → .deps/<name>.
fn build_dep_dir(name: &str, dep: &Dependency, project_dir: &Path) -> Option<PathBuf> {
    match dep {
        Dependency::Simple(_) => Some(project_dir.join(".deps").join(name)),
        Dependency::Detailed(d) => {
            if let Some(p) = &d.path {
                Some(project_dir.join(p))
            } else {
                // git, url, or version dep — all land in .deps/<name> after `freight fetch`
                Some(project_dir.join(".deps").join(name))
            }
        }
    }
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
    tool_paths: &[PathBuf],
) -> Result<Vec<PathBuf>, FreightError> {
    let resolved = build_system.to_string();

    std::fs::create_dir_all(build_dir)?;

    progress(BuildEvent::BuildingForeignDep { name: name.to_string(), backend: resolved.clone() });

    let search_dir = match resolved.as_str() {
        "cmake"     => { cmake::build_cmake(dep_dir, build_dir, profile, cmake_args, target, tool_paths)?; build_dir.to_path_buf() }
        "make"      => { make::build_make(dep_dir, tool_paths)?; dep_dir.to_path_buf() }
        "meson"     => { meson::build_meson(dep_dir, build_dir, tool_paths)?; build_dir.to_path_buf() }
        "autotools" => { autotools::build_autotools(dep_dir, build_dir, target, tool_paths)?; build_dir.join("install") }
        "scons"     => { scons::build_scons(dep_dir, tool_paths)?; dep_dir.to_path_buf() }
        "bazel"     => { bazel::build_bazel(dep_dir, tool_paths)?; dep_dir.to_path_buf() }
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
///
/// `tool_paths` is prepended to `PATH` so that build-time tool deps (cmake,
/// ninja, protoc, …) installed by freight take precedence over system ones.
pub(crate) fn run(
    prog: &str,
    args: &[&str],
    cwd: &Path,
    label: &str,
    tool_paths: &[PathBuf],
) -> Result<(), FreightError> {
    let mut cmd = Command::new(prog);
    cmd.args(args).current_dir(cwd);

    if !tool_paths.is_empty() {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let mut parts: Vec<PathBuf> = tool_paths.to_vec();
        parts.extend(std::env::split_paths(&current));
        if let Ok(new_path) = std::env::join_paths(parts) {
            cmd.env("PATH", new_path);
        }
    }

    let status = cmd
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

/// Collect `bin/` directories that contain executables from a built or extracted dep.
///
/// Tries, in order:
/// - `<dep_dir>/bin/`              — prebuilt tarballs or installed deps
/// - `<dep_dir>/install/bin/`      — cmake/autotools install prefix
/// - `<dep_dir>/.freight-build/install/bin/` — freight's own build dir
/// - `<dep_dir>/<any-subdir>/bin/` — tarballs with a top-level wrapper dir
///                                   (e.g. `cmake-3.28.6-linux-x86_64/bin/`)
///
/// Only directories that actually contain at least one executable are returned.
pub(crate) fn collect_bin_dirs(dep_dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();

    let candidates = [
        dep_dir.join("bin"),
        dep_dir.join("install").join("bin"),
        dep_dir.join(".freight-build").join("install").join("bin"),
    ];
    for c in &candidates {
        if has_executables(c) { found.push(c.clone()); }
    }

    // One level deep — catches tarballs that unpack to a versioned top directory.
    if let Ok(entries) = std::fs::read_dir(dep_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
            // Skip already-checked names and freight internal dirs.
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(name.as_ref(), "bin" | "install" | ".freight-build" | "include" | "lib" | "src") {
                continue;
            }
            let sub_bin = entry.path().join("bin");
            if has_executables(&sub_bin) { found.push(sub_bin); }
        }
    }

    found
}

fn has_executables(dir: &Path) -> bool {
    if !dir.is_dir() { return false; }
    std::fs::read_dir(dir)
        .ok()
        .map(|rd| rd.flatten().any(|e| is_executable(&e.path())))
        .unwrap_or(false)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("exe") | Some("cmd") | Some("bat")
        )
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
