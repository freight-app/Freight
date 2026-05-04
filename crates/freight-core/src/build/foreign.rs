//! Foreign build system integration: cmake, make, meson, autotools, scons.
//!
//! When a dependency has a recognised foreign build system (detected
//! automatically or declared via `build_system = "..."`), freight delegates
//! compilation to that tool and links the resulting libraries.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FreightError;
use crate::manifest::types::{Dependency, Manifest};

// ── Public types ──────────────────────────────────────────────────────────────

/// Output of a foreign dep build: library archives to link + include dirs.
pub struct ForeignBuilt {
    pub name: String,
    pub libs: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    /// Raw linker flags (e.g. `-pthread`, `-L/usr/lib`, `-lfoo`) produced by
    /// pkg-config queries. Appended verbatim to the linker command.
    pub raw_link_flags: Vec<String>,
}

/// Resolved pkg-config dep result exposed to `build.freight` as `packages["name"]`.
pub struct ResolvedPkgConfig {
    pub name:         String,
    pub found:        bool,
    pub version:      String,
    /// Resolved include directories from `pkg-config --cflags`.
    pub include_dirs: Vec<PathBuf>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build all foreign deps declared in `manifest` and return their link artifacts
/// alongside the resolved pkg-config results (for `build.freight` `packages` map).
pub fn build_foreign_deps(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
) -> Result<(Vec<ForeignBuilt>, Vec<ResolvedPkgConfig>), FreightError> {
    let mut results = Vec::new();
    let mut pkg_results: Vec<ResolvedPkgConfig> = Vec::new();

    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else { continue };

        // ── pkg-config dep ────────────────────────────────────────────────────
        // Can be standalone (`{ pkg_config = "zlib" }`) or combined with
        // `system` (`{ system = "z", pkg_config = "zlib" }`). When combined,
        // `system` is the bare -l{name} fallback if pkg-config is unavailable.
        if let Some(query) = &d.pkg_config {
            use owo_colors::OwoColorize;
            println!("  {} {} (pkg-config)", "Resolving".dimmed(), name);
            match super::http::pkg_config_query(query) {
                Ok(pc) => {
                    let version = pkg_config_version(query);
                    pkg_results.push(ResolvedPkgConfig {
                        name: name.clone(),
                        found: true,
                        version,
                        include_dirs: pc.include_dirs.clone(),
                    });
                    results.push(ForeignBuilt {
                        name: name.clone(),
                        libs: vec![],
                        include_dirs: pc.include_dirs,
                        raw_link_flags: pc.link_flags,
                    });
                }
                Err(e) => {
                    if let Some(fallback) = &d.system {
                        println!(
                            "  {} pkg-config for '{name}' failed ({e}); \
                             falling back to -l{fallback}",
                            "warning:".yellow()
                        );
                        pkg_results.push(ResolvedPkgConfig { name: name.clone(), found: false, version: String::new(), include_dirs: vec![] });
                        results.push(ForeignBuilt {
                            name: name.clone(),
                            libs: vec![],
                            include_dirs: vec![],
                            raw_link_flags: vec![format!("-l{fallback}")],
                        });
                    } else if d.optional {
                        println!(
                            "  {} pkg-config for '{name}' not found (optional, skipping)",
                            "warning:".yellow()
                        );
                        pkg_results.push(ResolvedPkgConfig { name: name.clone(), found: false, version: String::new(), include_dirs: vec![] });
                    } else {
                        return Err(FreightError::ManifestParse(format!(
                            "pkg-config failed for '{name}' and no system fallback: {e}"
                        )));
                    }
                }
            }
            continue;
        }

        // ── Pure system dep (no pkg_config) ──────────────────────────────────
        // -l{name} is collected by collect_system_lib_flags; nothing to do here.
        if d.system.is_some() {
            continue;
        }

        // ── Determine source directory ─────────────────────────────────────────
        let dep_dir = if let Some(rel) = &d.path {
            project_dir.join(rel)
        } else if d.git.is_some() {
            project_dir.join(".deps").join(name)
        } else if let Some(url) = &d.url {
            super::http::fetch_url_dep(name, url, d.sha256.as_deref(), project_dir)?
        } else {
            // Version dep — not a foreign build.
            continue;
        };

        if !dep_dir.exists() {
            return Err(FreightError::ManifestParse(format!(
                "foreign dep '{name}' not found at '{}' — run `freight fetch` first",
                dep_dir.display()
            )));
        }

        // ── Resolve build system ──────────────────────────────────────────────
        // Explicit > auto-detect > header-only fallback > skip (freight native).
        let bs = match &d.build_system {
            Some(bs) if bs == "none" => {
                let include_dirs = collect_include_dirs(&dep_dir, &d.include, None);
                results.push(ForeignBuilt {
                    name: name.clone(),
                    libs: vec![],
                    include_dirs,
                    raw_link_flags: vec![],
                });
                continue;
            }
            Some(bs) => bs.clone(),
            None => {
                if d.path.is_some() && dep_dir.join("freight.toml").exists() {
                    continue;
                }
                match detect_build_system(&dep_dir) {
                    Some(detected) => detected,
                    None => {
                        // No known build system. If the dep has no compilable
                        // source files it is header-only — collect include dirs
                        // without building. Otherwise skip silently.
                        if !has_source_files(&dep_dir) {
                            let include_dirs =
                                collect_include_dirs(&dep_dir, &d.include, None);
                            if !include_dirs.is_empty() {
                                results.push(ForeignBuilt {
                                    name: name.clone(),
                                    libs: vec![],
                                    include_dirs,
                                    raw_link_flags: vec![],
                                });
                            }
                        }
                        continue;
                    }
                }
            }
        };

        let build_dir = dep_dir.join(".freight-build");
        let libs = invoke_build_system(&dep_dir, &build_dir, name, &bs, profile, &d.cmake_args)?;

        let include_dirs = collect_include_dirs(&dep_dir, &d.include, Some(&build_dir));

        results.push(ForeignBuilt {
            name: name.clone(),
            libs,
            include_dirs,
            raw_link_flags: vec![],
        });
    }

    Ok((results, pkg_results))
}

/// Query `pkg-config --modversion` and return the version string, or empty on failure.
fn pkg_config_version(query: &str) -> String {
    // Use only the first token (the package name) for --modversion.
    let pkg_name = query.split_whitespace().next().unwrap_or(query);
    let out = std::process::Command::new("pkg-config")
        .args(["--modversion", pkg_name])
        .output()
        .ok();
    out.filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Resolve include directories for a dep.
///
/// Explicit `include = [...]` in the manifest wins. When absent, probe common
/// conventions: `include/`, `inc/`, and (if `build_dir` is provided) the
/// autotools/cmake install tree at `<build_dir>/install/include/`.
fn collect_include_dirs(
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

// ── Build system dispatch ─────────────────────────────────────────────────────

fn invoke_build_system(
    dep_dir: &Path,
    build_dir: &Path,
    name: &str,
    build_system: &str,
    profile: &str,
    cmake_args: &[String],
) -> Result<Vec<PathBuf>, FreightError> {
    let resolved = if build_system == "auto" {
        detect_build_system(dep_dir).ok_or_else(|| {
            FreightError::ManifestParse(format!(
                "cannot auto-detect build system for foreign dep '{name}'"
            ))
        })?
    } else {
        build_system.to_string()
    };

    std::fs::create_dir_all(build_dir)?;

    use owo_colors::OwoColorize;
    println!("  {} {} ({})", "Building".dimmed(), name, resolved);

    let search_dir = match resolved.as_str() {
        "cmake"     => { build_cmake(dep_dir, build_dir, profile, cmake_args)?; build_dir.to_path_buf() }
        "make"      => { build_make(dep_dir)?; dep_dir.to_path_buf() }
        "meson"     => { build_meson(dep_dir, build_dir)?; build_dir.to_path_buf() }
        "autotools" => { build_autotools(dep_dir, build_dir)?; build_dir.join("install") }
        "scons"     => { build_scons(dep_dir)?; dep_dir.to_path_buf() }
        other => {
            return Err(FreightError::ManifestParse(format!(
                "unknown build_system '{other}' for '{name}'; \
                 expected: cmake, make, meson, autotools, scons, auto, none"
            )));
        }
    };

    find_libs(&search_dir)
}

pub(crate) fn detect_build_system(dep_dir: &Path) -> Option<String> {
    if dep_dir.join("CMakeLists.txt").exists() { return Some("cmake".into()); }
    if dep_dir.join("meson.build").exists()    { return Some("meson".into()); }
    // configure.ac / configure.in → autotools (check before Makefile: autotools projects
    // may have a generated Makefile from a prior run, but the canonical source is configure.ac)
    if dep_dir.join("configure.ac").exists() || dep_dir.join("configure.in").exists() {
        return Some("autotools".into());
    }
    if dep_dir.join("autogen.sh").exists() || dep_dir.join("configure").exists() {
        return Some("autotools".into());
    }
    if dep_dir.join("SConstruct").exists() { return Some("scons".into()); }
    if dep_dir.join("Makefile").exists() || dep_dir.join("GNUmakefile").exists() {
        return Some("make".into());
    }
    None
}

/// Return `true` if `dir` contains at least one compilable source file
/// (checked recursively, depth-limited to avoid scanning huge trees).
fn has_source_files(dir: &Path) -> bool {
    const SOURCE_EXTS: &[&str] = &[
        "c", "cpp", "cc", "cxx", "c++", "cppm",
        "f", "f90", "f95", "f03", "f08",
        "s", "asm", "nasm",
        "cu", "hip", "cl",
        "d", "ada", "adb",
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

// ── Individual build system runners ──────────────────────────────────────────

fn build_cmake(dep_dir: &Path, build_dir: &Path, profile: &str, extra_args: &[String]) -> Result<(), FreightError> {
    let build_type = if profile == "release" { "Release" } else { "Debug" };

    let src   = dep_dir.to_string_lossy().into_owned();
    let bdir  = build_dir.to_string_lossy().into_owned();
    let btype = format!("-DCMAKE_BUILD_TYPE={build_type}");

    let mut configure_args: Vec<&str> = vec!["-S", &src, "-B", &bdir, &btype];
    for a in extra_args { configure_args.push(a.as_str()); }

    run("cmake", &configure_args, dep_dir, "cmake configure")?;
    run("cmake", &["--build", &bdir], dep_dir, "cmake build")
}

fn build_make(dep_dir: &Path) -> Result<(), FreightError> {
    run("make", &[], dep_dir, "make")
}

fn build_meson(dep_dir: &Path, build_dir: &Path) -> Result<(), FreightError> {
    if !build_dir.join("build.ninja").exists() {
        run("meson", &[
            "setup",
            &build_dir.to_string_lossy(),
            &dep_dir.to_string_lossy(),
        ], dep_dir, "meson setup")?;
    }
    run("ninja", &["-C", &build_dir.to_string_lossy()], dep_dir, "ninja")
}

fn build_autotools(dep_dir: &Path, build_dir: &Path) -> Result<(), FreightError> {
    // Generate the configure script if it doesn't exist yet.
    if !dep_dir.join("configure").exists() {
        if dep_dir.join("autogen.sh").exists() {
            run("sh", &["autogen.sh"], dep_dir, "autogen.sh")?;
        } else {
            run("autoreconf", &["-fi"], dep_dir, "autoreconf")?;
        }
    }

    // Install into .freight-build/install/ so libs and headers land in known locations.
    let install_dir = build_dir.join("install");
    std::fs::create_dir_all(&install_dir)?;
    let configure = dep_dir.join("configure").to_string_lossy().into_owned();
    let prefix    = format!("--prefix={}", install_dir.display());

    run(&configure, &[&prefix], dep_dir, "configure")?;
    run("make", &[], dep_dir, "make")?;
    run("make", &["install"], dep_dir, "make install")
}

fn build_scons(dep_dir: &Path) -> Result<(), FreightError> {
    run("scons", &[], dep_dir, "scons")
}

fn run(prog: &str, args: &[&str], cwd: &Path, label: &str) -> Result<(), FreightError> {
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

// ── Output discovery ──────────────────────────────────────────────────────────

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
