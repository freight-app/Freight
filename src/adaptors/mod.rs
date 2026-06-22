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
pub mod scons;

// Dependency resolution moved to `crate::resolve`; these names are used heavily
// here and re-exported for existing `adaptors::` consumers during the migration.
pub use crate::resolve::pkg_config::{
    pkg_config_query, pkg_config_query_cross, pkg_config_query_with_path, pkg_config_version,
    PkgConfigResult, ResolvedPkgConfig,
};
pub use crate::resolve::pkg_config_cache::PkgConfigCache;
use crate::resolve::system_pm;

use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::{Dependency, Manifest};
use crate::toolchain::system_libs::{find_stub, load_system_lib_stubs};

// ── OS-family pseudo-deps ─────────────────────────────────────────────────────

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

/// Build all foreign deps declared in `manifest` and return their link artifacts,
/// the resolved pkg-config results, and any executable bin-dirs accumulated from
/// `[build-dependencies]` entries (prepended to PATH for subsequent build steps).
/// A foreign dep that needs a subprocess build (cmake, make, meson, …).
/// Collected in the sequential pass then dispatched in parallel.
struct BuildJob {
    name: String,
    dep_dir: PathBuf,
    backend: String,
    defines: Vec<String>,
    include: Vec<String>,
    target: Option<String>,
    /// Tool bin dirs accumulated from build-deps built before this job.
    tool_paths: Vec<PathBuf>,
}

/// The effective `[patch]` table for a build. Patches are graph-wide and come
/// from the workspace root. When building the root itself, that's the passed
/// manifest; when building a member, the root manifest is parsed for its
/// `[patch]` table (the root may be `[workspace]`-only, so we read just `[patch]`
/// rather than going through the full manifest loader, which requires `[package]`).
fn effective_patches(
    root_dir: &std::path::Path,
    project_dir: &std::path::Path,
    manifest: &Manifest,
) -> std::collections::HashMap<String, Dependency> {
    if root_dir == project_dir {
        return manifest.patch.clone();
    }
    #[derive(serde::Deserialize)]
    struct PatchOnly {
        #[serde(default)]
        patch: std::collections::HashMap<String, Dependency>,
    }
    std::fs::read_to_string(root_dir.join("freight.toml"))
        .ok()
        .and_then(|t| toml::from_str::<PatchOnly>(&t).ok())
        .map(|p| p.patch)
        .unwrap_or_default()
}

/// If `dir`'s manifest is a *foreign* package (`[package].build` set, the shape
/// `vcpkg-scraper` produces), return `(url, build_system, patches)`. `None` when
/// it's an ordinary native freight package (built by the dep-graph builder).
fn foreign_package_spec(dir: &std::path::Path) -> Option<(Option<String>, String, Vec<String>)> {
    let m = crate::manifest::load_manifest(dir).ok()?;
    let build = m.package.build.clone()?;
    // A package that also declares local sources/a library is native, not foreign.
    if m.lib.is_some() {
        return None;
    }
    Some((m.package.url.clone(), build, m.package.patches.clone()))
}

/// Apply a foreign member's `[package].patches` to its (fetched) source, if the
/// patch files are present in the member directory. Best-effort: a missing patch
/// file is warned about, not fatal.
fn apply_member_patches(
    member_dir: &std::path::Path,
    source_dir: &std::path::Path,
    patches: &[String],
    progress: &Progress,
) {
    for p in patches {
        let patch_file = member_dir.join(p);
        if !patch_file.is_file() {
            progress(BuildEvent::Warning(format!(
                "patch '{p}' not found in {} — skipping",
                member_dir.display()
            )));
            continue;
        }
        let applied = std::process::Command::new("patch")
            .arg("-p1")
            .arg("-i")
            .arg(&patch_file)
            .current_dir(source_dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !applied {
            progress(BuildEvent::Warning(format!("failed to apply patch '{p}'")));
        }
    }
}

pub fn build_foreign_deps(
    project_dir: &std::path::Path,
    root_dir: &std::path::Path,
    manifest: &Manifest,
    profile: &str,
    dep_defines: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    progress: &Progress,
) -> Result<(Vec<ForeignBuilt>, Vec<ResolvedPkgConfig>, Vec<PathBuf>), FreightError> {
    let pkgs_root = root_dir;
    let mut results: Vec<ForeignBuilt> = Vec::new();
    let mut pkg_results: Vec<ResolvedPkgConfig> = Vec::new();
    let mut pc_cache = PkgConfigCache::load(project_dir);
    // Cross build? Then host pkg-config must not feed this build (see CrossBuild).
    let cross = cross_build(manifest);

    // ── Build-dependency pass (sequential, before everything else) ────────────
    // Build-deps are tools (cmake, ninja, protoc, …).  We build them first and
    // collect any executable `bin/` directories they install.  Those paths are
    // then prepended to PATH for every subsequent build step so freight-installed
    // tools take precedence over system ones.
    let mut tool_paths: Vec<PathBuf> = Vec::new();

    // Build-deps are host tools — gate them on the host platform, not the target.
    let tool_env = crate::resolve::build_deps::HostToolEnv {
        pkgs_dir: pkgs_root.join(".pkgs"),
    };
    for (name, dep) in &manifest.effective_build_dependencies() {
        // Resolver decision (no prebuilt index yet): a system tool on PATH
        // satisfies the build-dep with no fetch/build — unless `source = true`
        // forces a from-source build.
        let source_forced = matches!(dep, Dependency::Detailed(d) if d.source);
        if !source_forced && crate::resolve::build_deps::ToolEnv::system(&tool_env, name, "*") {
            continue;
        }
        let dep_dir = match build_dep_dir(name, dep, project_dir, pkgs_root) {
            Some(d) => d,
            None => continue,
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
            match d.dep_type.as_deref() {
                Some("none") | None
                    if !dep_dir.join("CMakeLists.txt").exists()
                        && !dep_dir.join("Makefile").exists()
                        && !dep_dir.join("meson.build").exists() =>
                {
                    None
                }
                Some(bs) => Some(bs.to_string()),
                None => detect_build_system(&dep_dir),
            }
        } else {
            detect_build_system(&dep_dir)
        };

        if let Some(bs) = backend {
            let build_dir = dep_dir.join(".freight-build");
            invoke_build_system(
                &dep_dir,
                &build_dir,
                name,
                &bs,
                profile,
                &[],
                None,
                progress,
                &tool_paths,
                &[],
            )?;
        }

        let new_bins = collect_bin_dirs(&dep_dir);
        if !new_bins.is_empty() {
            progress(BuildEvent::Warning(format!(
                "build-dep '{name}': using local executables from {}",
                new_bins
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        tool_paths.extend(new_bins);
    }

    // ── cmake version check ───────────────────────────────────────────────────
    // If the project declares `cmake = "<constraint>"` in [build-dependencies],
    // verify that the cmake binary we will actually use satisfies the constraint
    // before any cmake-based dep build starts.
    if let Some(cmake_constraint) = cmake_build_dep_constraint(manifest) {
        check_cmake_version(&cmake_constraint, &tool_paths)?;
    }

    // ── Sequential pass: fast deps + build-job collection ────────────────────
    // Slow deps (those that call into cmake/make/ninja) are staged in `jobs`
    // and built concurrently in the parallel pass below.

    // Note: system-library link features from `[os.*]/[arch.*] features = [...]`
    // are resolved at link time by `collect_system_lib_flags` (which also handles
    // macOS `-framework` and MSVC `.lib` formatting), not here.

    let mut jobs: Vec<BuildJob> = Vec::new();
    // Vendored foreign packages (path / [patch] members with `[package]` url+build).
    // Collected here, then built with their transitive foreign deps after the
    // parallel pass (see build_foreign_member_closure).
    let mut foreign_roots: Vec<(String, PathBuf)> = Vec::new();

    // `[patch]` overrides apply graph-wide and come from the workspace root.
    // Honour them here too (the dep-graph builder already does), so a dep that is
    // patched to a local path resolves to that member instead of falling through
    // to pkg-config / the registry.
    let patches = effective_patches(root_dir, project_dir, manifest);

    for (name, dep) in &manifest.dependencies {
        // Apply a `[patch]` redirect, if any. A patched dep's path resolves
        // relative to the root manifest, not the dep-declaring manifest.
        let patched = patches.get(name);
        let dep = patched.unwrap_or(dep);
        let dep_base: &std::path::Path = if patched.is_some() {
            root_dir
        } else {
            project_dir
        };

        if let Some(version) = package_dep_version(dep) {
            // Check for a metadata-only registry dep that was fetched from upstream source.
            // `fetch_registry_deps()` writes `.freight-build-system` when the dep needs
            // compiling from source (e.g. vcpkg packages with `build = "cmake"`).
            let dep_dir = pkgs_root.join(".pkgs").join(name);
            let bs_file = dep_dir.join(".freight-build-system");
            if bs_file.exists() {
                let bs = std::fs::read_to_string(&bs_file)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "cmake".to_string());
                jobs.push(BuildJob {
                    name: name.clone(),
                    dep_dir,
                    backend: bs,
                    defines: vec![],
                    include: vec![],
                    target: manifest.compiler.target.clone(),
                    tool_paths: tool_paths.clone(),
                });
            } else {
                let query = package_query(name, version);
                let repo = dep_repo(dep);
                let optional = package_dep_optional(dep);
                if let Some((built, maybe_pc)) = resolve_version_dep(
                    name,
                    &query,
                    version,
                    repo,
                    optional,
                    project_dir,
                    profile,
                    pkgs_root,
                    progress,
                    &mut pc_cache,
                    cross.as_ref(),
                )? {
                    if let Some(pc) = maybe_pc {
                        pkg_results.push(pc);
                    }
                    results.push(built);
                }
            }
            continue;
        }

        let Dependency::Detailed(d) = dep else {
            continue;
        };

        if crate::manifest::types::is_platform_dep(name) {
            continue;
        }

        // `external = true` deps are built by a plugin (e.g. one handling
        // `[cmake]`), not by freight's core — skip the auto build/detect here.
        if d.external {
            continue;
        }

        let dep_dir = if let Some(rel) = &d.path {
            dep_base.join(rel)
        } else if d.is_git() {
            pkgs_root.join(".pkgs").join(name)
        } else if let Some(url) = &d.url {
            crate::fetch::http::fetch_url_dep(
                name,
                url,
                d.sha256.as_deref(),
                project_dir,
                progress,
            )?
        } else {
            continue;
        };

        if !dep_dir.exists() {
            return Err(FreightError::ManifestParse(format!(
                "foreign dep '{name}' not found at '{}' — run `freight fetch` first",
                dep_dir.display()
            )));
        }

        let bs = match &d.dep_type {
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
            Some(bs) => {
                validate_backend(name, bs, &dep_dir)?;
                bs.clone()
            }
            None => {
                if d.path.is_some() && dep_dir.join("freight.toml").exists() {
                    // The member may itself be a *foreign* package (`[package]` with
                    // `url`/`build`, the vcpkg-scraper shape). Collect it as a root of
                    // the foreign-member sub-graph, built — with its transitive
                    // foreign deps, in topological order — after the parallel pass.
                    // Otherwise it's a native freight package handled by the
                    // dep-graph builder.
                    if foreign_package_spec(&dep_dir).is_some() {
                        foreign_roots.push((name.clone(), dep_dir.clone()));
                    }
                    continue;
                }
                match detect_build_system(&dep_dir) {
                    Some(detected) => detected,
                    None => {
                        if !has_source_files(&dep_dir) {
                            let include_dirs = collect_include_dirs(&dep_dir, &d.include, None);
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

        // Per-dep defines: the dep's own `defines = [...]` plus any forwarded from
        // the root manifest via `<dep>/define:NAME` feature entries.
        let mut defines = d.defines.clone();
        if let Some(fwd) = dep_defines.get(name) {
            defines.extend(fwd.iter().cloned());
        }
        jobs.push(BuildJob {
            name: name.clone(),
            dep_dir,
            backend: bs,
            defines,
            include: d.include.clone(),
            target: manifest.compiler.target.clone(),
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
                &job.dep_dir,
                &build_dir,
                &job.name,
                &job.backend,
                profile,
                &job.defines,
                job.target.as_deref(),
                progress,
                &job.tool_paths,
                &[], // leaf foreign deps: no transitive prefixes
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

    // Foreign-member sub-graph (vendored vcpkg-style packages reached via path /
    // [patch]) is built in topological order so each member sees its already-built
    // dependencies' install prefixes (CMAKE_PREFIX_PATH).
    results.extend(build_foreign_member_closure(
        &foreign_roots,
        &patches,
        root_dir,
        profile,
        progress,
        &tool_paths,
    )?);

    pc_cache.save(project_dir);
    Ok((results, pkg_results, tool_paths))
}

/// Build the closure of vendored foreign packages (path / `[patch]` members whose
/// `[package]` declares `url`/`build`) in **topological order**, so each member's
/// build sees the install prefixes of its already-built foreign dependencies
/// (via `CMAKE_PREFIX_PATH`). `roots` are the directly-depended foreign members;
/// their transitive foreign deps are discovered here.
fn build_foreign_member_closure(
    roots: &[(String, PathBuf)],
    patches: &std::collections::HashMap<String, Dependency>,
    root_dir: &Path,
    profile: &str,
    progress: &Progress,
    tool_paths: &[PathBuf],
) -> Result<Vec<ForeignBuilt>, FreightError> {
    use std::collections::{BTreeMap, VecDeque};
    if roots.is_empty() {
        return Ok(vec![]);
    }

    struct Node {
        dir: PathBuf,
        url: Option<String>,
        build: String,
        patches: Vec<String>,
        deps: Vec<String>,
    }

    // 1. Discover the closure.
    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    let mut queue: VecDeque<(String, PathBuf)> = roots.iter().cloned().collect();
    while let Some((name, dir)) = queue.pop_front() {
        if nodes.contains_key(&name) {
            continue;
        }
        let Some((url, build, mpatches)) = foreign_package_spec(&dir) else {
            continue;
        };
        let mut fdeps = Vec::new();
        if let Ok(m) = crate::manifest::load_manifest(&dir) {
            for (dep_name, dep) in &m.dependencies {
                if let Some(td) = foreign_dep_dir(patches, root_dir, &dir, dep_name, dep) {
                    fdeps.push(dep_name.clone());
                    if !nodes.contains_key(dep_name) {
                        queue.push_back((dep_name.clone(), td));
                    }
                }
            }
        }
        nodes.insert(
            name,
            Node {
                dir,
                url,
                build,
                patches: mpatches,
                deps: fdeps,
            },
        );
    }

    // 2. Topological order (deps first).
    let graph: BTreeMap<String, Vec<String>> = nodes
        .iter()
        .map(|(n, node)| (n.clone(), node.deps.clone()))
        .collect();
    let order = topo_order(&graph);

    // 3. Build each, accumulating install prefixes for downstream members.
    let mut prefixes: Vec<PathBuf> = Vec::new();
    let mut built = Vec::new();
    for name in order {
        let Some(node) = nodes.get(&name) else {
            continue;
        };
        let source_dir = match &node.url {
            Some(url) => crate::fetch::http::fetch_url_dep(&name, url, None, &node.dir, progress)?,
            None => node.dir.clone(),
        };
        apply_member_patches(&node.dir, &source_dir, &node.patches, progress);
        let build_dir = source_dir.join(".freight-build");
        let libs = invoke_build_system(
            &source_dir,
            &build_dir,
            &name,
            &node.build,
            profile,
            &[],
            None,
            progress,
            tool_paths,
            &prefixes,
        )?;
        let mut include_dirs = collect_include_dirs(&source_dir, &[], Some(&build_dir));
        // Header-only / copy-only ports often keep their header(s) at the archive
        // root (e.g. plf_colony.h, stb_image.h) rather than in include/. Expose the
        // source root so dependents can find them — no build tool is involved.
        if matches!(node.build.as_str(), "none" | "header") && !include_dirs.contains(&source_dir) {
            include_dirs.push(source_dir.clone());
        }
        if let Some(p) = install_prefix(&source_dir, &node.build) {
            if p.is_dir() {
                prefixes.push(p);
            }
        }
        built.push(ForeignBuilt {
            name,
            libs,
            include_dirs,
            raw_link_flags: vec![],
        });
    }
    // Built deps-first (topological); the linker needs dependents before their
    // dependencies for static archives, so return in reverse order.
    built.reverse();
    Ok(built)
}

/// Build a *foreign package itself* (`[package]` with `url`/`build`, no native
/// targets — the vcpkg-scraper shape) as a standalone `freight build`. Fetches
/// the source (if `url`), applies `[package].patches`, runs the foreign build
/// with `prefix_paths` on `CMAKE_PREFIX_PATH`, and copies the produced libraries
/// into `target_dir/<profile>/`. Returns the placed library paths.
pub fn build_foreign_self(
    project_dir: &Path,
    target_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    prefix_paths: &[PathBuf],
    tool_paths: &[PathBuf],
    progress: &Progress,
) -> Result<Vec<PathBuf>, FreightError> {
    let pkg = &manifest.package;
    let build = pkg.build.clone().ok_or_else(|| {
        FreightError::ManifestParse("foreign self-build requires [package].build".into())
    })?;
    let source_dir = match &pkg.url {
        Some(url) => crate::fetch::http::fetch_url_dep(
            &pkg.name,
            url,
            pkg.sha256.as_deref(),
            project_dir,
            progress,
        )?,
        None => project_dir.to_path_buf(),
    };
    apply_member_patches(project_dir, &source_dir, &pkg.patches, progress);
    let build_dir = source_dir.join(".freight-build");
    let libs = invoke_build_system(
        &source_dir,
        &build_dir,
        &pkg.name,
        &build,
        profile,
        &[],
        manifest.compiler.target.as_deref(),
        progress,
        tool_paths,
        prefix_paths,
    )?;

    // Place the built libraries in the package's own target/<profile>/.
    let out_dir = target_dir.join(profile);
    std::fs::create_dir_all(&out_dir).ok();
    let mut placed = Vec::new();
    for lib in &libs {
        if let Some(fname) = lib.file_name() {
            let dest = out_dir.join(fname);
            if std::fs::copy(lib, &dest).is_ok() {
                placed.push(dest);
            }
        }
    }
    Ok(placed)
}

/// The directory a member's dependency resolves to *if* it's itself a foreign
/// member — via a `[patch]` path (relative to the workspace root) or the dep's
/// own `path` (relative to the member). `None` when it isn't a foreign package.
fn foreign_dep_dir(
    patches: &std::collections::HashMap<String, Dependency>,
    root_dir: &Path,
    member_dir: &Path,
    dep_name: &str,
    dep: &Dependency,
) -> Option<PathBuf> {
    let dir = if let Some(Dependency::Detailed(pd)) = patches.get(dep_name) {
        root_dir.join(pd.path.as_ref()?)
    } else if let Dependency::Detailed(d) = dep {
        member_dir.join(d.path.as_ref()?)
    } else {
        return None;
    };
    (dir.join("freight.toml").exists() && foreign_package_spec(&dir).is_some()).then_some(dir)
}

/// The install prefix a foreign build produces (contains `include/` + `lib/`),
/// fed to dependents' `CMAKE_PREFIX_PATH`.
fn install_prefix(source_dir: &Path, build: &str) -> Option<PathBuf> {
    match build {
        "cmake" | "meson" | "autotools" => Some(source_dir.join(".freight-build").join("install")),
        _ => Some(source_dir.to_path_buf()),
    }
}

/// Topologically order `graph` (name → dep names), dependencies first. Cycle
/// back-edges are ignored; every node appears exactly once.
fn topo_order(graph: &std::collections::BTreeMap<String, Vec<String>>) -> Vec<String> {
    use std::collections::HashSet;
    fn visit(
        n: &str,
        graph: &std::collections::BTreeMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        on_stack: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if visited.contains(n) || !on_stack.insert(n.to_string()) {
            return;
        }
        if let Some(deps) = graph.get(n) {
            for d in deps {
                visit(d, graph, visited, on_stack, out);
            }
        }
        on_stack.remove(n);
        if visited.insert(n.to_string()) {
            out.push(n.to_string());
        }
    }
    let mut visited = HashSet::new();
    let mut on_stack = HashSet::new();
    let mut out = Vec::new();
    for n in graph.keys() {
        visit(n, graph, &mut visited, &mut on_stack, &mut out);
    }
    out
}

/// Extract the cmake version constraint from `[build-dependencies]`, if any.
/// Returns `Some(">=3.20")` when the user wrote `cmake = ">=3.20"` (or any
/// detailed form with a `version` field).
fn cmake_build_dep_constraint(manifest: &Manifest) -> Option<String> {
    let dep = manifest.build_dependencies.get("cmake")?;
    match dep {
        Dependency::Simple(v) => {
            if crate::manifest::types::is_unpinned_version(v) {
                None
            } else {
                Some(v.trim().to_string())
            }
        }
        Dependency::Detailed(d) => {
            let v = d.version.as_deref()?;
            if crate::manifest::types::is_unpinned_version(v) {
                None
            } else {
                Some(v.trim().to_string())
            }
        }
    }
}

/// Check that the cmake binary reachable via `tool_paths` (or system PATH)
/// satisfies `constraint`.  Returns a descriptive error if it does not.
fn check_cmake_version(constraint: &str, tool_paths: &[PathBuf]) -> Result<(), FreightError> {
    use semver::{Version, VersionReq};

    let (major, minor, patch) = match cmake::cmake_version(tool_paths) {
        Some((maj, min, pat)) => (maj, min, pat),
        None => {
            return Err(FreightError::CompilerNotFound(
                "cmake not found — install cmake or add it to [build-dependencies]".to_string(),
            ))
        }
    };

    let req = match VersionReq::parse(constraint) {
        Ok(r) => r,
        Err(_) => return Ok(()), // unparseable constraint — skip check
    };

    let ver_str = format!("{major}.{minor}.{patch}");
    let ver = match Version::parse(&ver_str) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    if req.matches(&ver) {
        Ok(())
    } else {
        Err(FreightError::ManifestParse(format!(
            "cmake {constraint} required by [build-dependencies] but found {ver_str}; \
             install a compatible cmake or change the version constraint"
        )))
    }
}

/// Resolve the on-disk directory for a build-dep entry.
/// Same logic as regular deps: path → join project_dir; git/url/version → .deps/<name>.
fn build_dep_dir(
    name: &str,
    dep: &Dependency,
    project_dir: &Path,
    pkgs_root: &Path,
) -> Option<PathBuf> {
    match dep {
        Dependency::Simple(_) => Some(pkgs_root.join(".pkgs").join(name)),
        Dependency::Detailed(d) => {
            if let Some(p) = &d.path {
                Some(project_dir.join(p))
            } else {
                Some(pkgs_root.join(".pkgs").join(name))
            }
        }
    }
}

fn package_dep_version(dep: &Dependency) -> Option<&str> {
    match dep {
        Dependency::Simple(version) => Some(version.as_str()),
        Dependency::Detailed(d)
            if d.version.is_some() && d.path.is_none() && !d.is_git() && d.url.is_none() =>
        {
            d.version.as_deref()
        }
        _ => None,
    }
}

fn package_dep_optional(dep: &Dependency) -> bool {
    matches!(dep, Dependency::Detailed(d) if d.optional)
}

fn dep_repo(dep: &Dependency) -> Option<&str> {
    if let Dependency::Detailed(d) = dep {
        d.registry.as_deref()
    } else {
        None
    }
}

/// Build a pkg-config query string from a dep name + version constraint:
/// unconstrained → bare name; `<`/`>`/`=`/`!`-prefixed → passed through; bare
/// number → `>=`. Shared by the build resolver and the fetch path.
pub(crate) fn package_query(name: &str, version: &str) -> String {
    if crate::manifest::types::is_unpinned_version(version) {
        return name.to_string();
    }
    let version = version.trim();
    if matches!(version.as_bytes().first(), Some(b'<' | b'>' | b'=' | b'!')) {
        format!("{name} {version}")
    } else {
        format!("{name} >= {version}")
    }
}

// ── Cross-compilation context ────────────────────────────────────────────────

/// Cross-compilation inputs for dependency resolution, derived from the
/// manifest's `[compiler]` section. `None` for a native (host) build.
///
/// Present when the effective target triple differs from the host **or** a
/// `[compiler].sysroot` is set. When cross-compiling, host pkg-config and the
/// host include/lib paths must never feed the build — system libs come from the
/// sysroot (if any) and everything else from a freight-fetched source package.
pub(crate) struct CrossBuild {
    pub target: Option<String>,
    pub sysroot: Option<PathBuf>,
}

/// Derive the [`CrossBuild`] context from a manifest, or `None` for a host build.
pub(crate) fn cross_build(manifest: &Manifest) -> Option<CrossBuild> {
    let target = manifest.compiler.target.clone();
    let sysroot = manifest
        .compiler
        .sysroot
        .clone()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let cross_triple = target.as_deref().map(is_cross_triple).unwrap_or(false);
    if cross_triple || sysroot.is_some() {
        Some(CrossBuild { target, sysroot })
    } else {
        None
    }
}

/// Whether a target triple/arch string denotes a different platform than the
/// host (a different CPU arch or OS). Coarse but sufficient to decide between
/// host pkg-config and a cross/source resolution path.
fn is_cross_triple(target: &str) -> bool {
    fn norm(a: &str) -> &str {
        match a {
            "amd64" => "x86_64",
            "arm64" => "aarch64",
            x => x,
        }
    }
    let t = target.to_ascii_lowercase();
    let host_arch = norm(std::env::consts::ARCH);
    let target_arch = norm(t.split('-').next().unwrap_or(""));
    if target_arch != host_arch {
        return true;
    }
    // Same arch, different OS (e.g. x86_64 host building x86_64-pc-windows).
    let os_token = |s: &str| {
        if s.contains("windows") || s.contains("mingw") || s.contains("msvc") {
            "windows"
        } else if s.contains("darwin") || s.contains("macos") || s.contains("apple") {
            "macos"
        } else if s.contains("linux") {
            "linux"
        } else {
            "other"
        }
    };
    let host_os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    };
    let to = os_token(&t);
    to != "other" && to != host_os
}

// ── Version dep resolution chain ─────────────────────────────────────────────

/// Resolve a version dep (`name = "1.3"` or `{ version = "1.3", repo = "..." }`)
/// through the configured resolver chain.
///
/// Returns `Ok(Some((built, maybe_pc)))` on success, `Ok(None)` when the dep
/// is optional and not found, or `Err` when the dep is required and all
/// resolvers fail.
///
/// Default chain (no explicit `repo`): pkg-config → system-lib stubs → target/deps cache.
#[allow(clippy::too_many_arguments)]
fn resolve_version_dep(
    name: &str,
    query: &str,
    version: &str,
    repo: Option<&str>,
    optional: bool,
    project_dir: &Path,
    profile: &str,
    root_dir: &Path,
    progress: &Progress,
    pc_cache: &mut PkgConfigCache,
    cross: Option<&CrossBuild>,
) -> Result<Option<(ForeignBuilt, Option<ResolvedPkgConfig>)>, FreightError> {
    let pkgs_root = root_dir;
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
            Ok(Some((
                ForeignBuilt {
                    name: name.to_string(),
                    libs: vec![],
                    include_dirs: vec![],
                    raw_link_flags: vec![link_flag],
                },
                None,
            )))
        }
        Some(_registry_name) => {
            // Named registry dep (`repo = "myregistry"` or `@registry/name` shorthand):
            // fetched by `freight fetch`; resolve at build time the same as a plain
            // version dep — pkg-config first, then system-lib stubs.
            resolve_version_dep(
                name,
                query,
                version,
                None,
                optional,
                project_dir,
                profile,
                pkgs_root,
                progress,
                pc_cache,
                cross,
            )
        }
        None if cross.is_some() => {
            // Cross build: host pkg-config would inject host `-I`/`-L` paths, so
            // it is never consulted. Resolve system libs from the sysroot's
            // pkg-config (if a sysroot is configured); libc stubs (`-lpthread`,
            // `-lm`, …) are fine — the cross linker resolves them against its
            // sysroot. Anything else must be a freight-fetched source package
            // (handled by the shared `.pkgs/` path below).
            let cb = cross.expect("cross.is_some()");
            if let Some(sysroot) = &cb.sysroot {
                if let Ok(pc) = pkg_config_query_cross(query, cb.target.as_deref(), sysroot) {
                    return Ok(Some((
                        ForeignBuilt {
                            name: name.to_string(),
                            libs: vec![],
                            include_dirs: pc.include_dirs.clone(),
                            raw_link_flags: pc.link_flags,
                        },
                        Some(ResolvedPkgConfig {
                            name: name.to_string(),
                            found: true,
                            version: String::new(),
                            include_dirs: pc.include_dirs,
                        }),
                    )));
                }
            }
            let stubs = load_system_lib_stubs();
            if let Some(stub) = find_stub(name, &stubs) {
                return Ok(Some((
                    ForeignBuilt {
                        name: name.to_string(),
                        libs: vec![],
                        include_dirs: vec![],
                        raw_link_flags: vec![format!("-l{}", stub.link_name)],
                    },
                    None,
                )));
            }
            // Fall through to the shared `.pkgs/` source-package resolution.
            resolve_fetched_dep(
                name, query, version, optional, profile, pkgs_root, progress, pc_cache,
            )
        }
        None => {
            // Default chain: pkg-config (cached) → system-lib stubs → target/deps/ cache.
            if let Ok((pc, ver)) = pc_cache.query(query) {
                return Ok(Some((
                    ForeignBuilt {
                        name: name.to_string(),
                        libs: vec![],
                        include_dirs: pc.include_dirs.clone(),
                        raw_link_flags: pc.link_flags,
                    },
                    Some(ResolvedPkgConfig {
                        name: name.to_string(),
                        found: true,
                        version: ver,
                        include_dirs: pc.include_dirs,
                    }),
                )));
            }
            // Built-in system-lib stubs (e.g. pthread, ws2_32).
            let stubs = load_system_lib_stubs();
            if let Some(stub) = find_stub(name, &stubs) {
                let link_flag = format!("-l{}", stub.link_name);
                return Ok(Some((
                    ForeignBuilt {
                        name: name.to_string(),
                        libs: vec![],
                        include_dirs: vec![],
                        raw_link_flags: vec![link_flag],
                    },
                    None,
                )));
            }
            // All freight-fetched deps (source, prebuilt, git, url) live in
            // `.pkgs/<name>/` — shared with the cross-build path.
            resolve_fetched_dep(
                name, query, version, optional, profile, pkgs_root, progress, pc_cache,
            )
        }
    }
}

/// Resolve a dependency from the freight-fetched `.pkgs/<name>/` cache (source,
/// prebuilt, git, or url). Tries a shipped `.pc` file, then bare `include/`+`lib/`
/// dirs, then a source build from a bundled `freight.toml`. Returns the
/// not-found error (or `Ok(None)` when optional) if the package was never
/// fetched. Used by both the native and cross resolution paths.
#[allow(clippy::too_many_arguments)]
fn resolve_fetched_dep(
    name: &str,
    query: &str,
    version: &str,
    optional: bool,
    profile: &str,
    pkgs_root: &Path,
    progress: &Progress,
    pc_cache: &mut PkgConfigCache,
) -> Result<Option<(ForeignBuilt, Option<ResolvedPkgConfig>)>, FreightError> {
    let dep_dir = pkgs_root.join(".pkgs").join(name);
    let cached = dep_dir.join(".freight-fetched").exists();

    if cached {
        // Try pkg-config if the dep ships a .pc file.
        let pc_dir = dep_dir.join("lib").join("pkgconfig");
        if pc_dir.is_dir() {
            if let Ok((pc, ver)) = pc_cache.query_with_path(query, &[pc_dir]) {
                return Ok(Some((
                    ForeignBuilt {
                        name: name.to_string(),
                        libs: vec![],
                        include_dirs: pc.include_dirs.clone(),
                        raw_link_flags: pc.link_flags,
                    },
                    Some(ResolvedPkgConfig {
                        name: name.to_string(),
                        found: true,
                        version: ver,
                        include_dirs: pc.include_dirs,
                    }),
                )));
            }
        }

        // No .pc file — collect include/ and lib/ dirs directly.
        let mut include_dirs: Vec<PathBuf> = Vec::new();
        for candidate in &["include", "inc"] {
            let d = dep_dir.join(candidate);
            if d.is_dir() {
                include_dirs.push(d);
            }
        }
        let mut link_flags: Vec<String> = Vec::new();
        let lib_dir = dep_dir.join("lib");
        if lib_dir.is_dir() {
            link_flags.push(format!("-L{}", lib_dir.display()));
            if let Ok(entries) = std::fs::read_dir(&lib_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if matches!(ext, "a" | "so" | "dylib" | "lib") {
                        let lname = stem.strip_prefix("lib").unwrap_or(stem);
                        link_flags.push(format!("-l{lname}"));
                    }
                }
            }
        }

        // No prebuilt libs found — if the dep ships a freight.toml (source
        // tarball downloaded from the registry), build it in-place and point
        // at the resulting static lib in target/{profile}/.
        if link_flags.is_empty() && dep_dir.join("freight.toml").exists() {
            progress(BuildEvent::DepBuildStarted {
                name: format!("{name}@{version}"),
            });
            // Suppress BuildStarted/Compiling from the inner build; translate Compiling
            // to DepCompiling so the CLI can show an inline dot bar instead.
            let outer = std::sync::Arc::clone(progress);
            let inner_progress: Progress = std::sync::Arc::new(move |ev| match ev {
                BuildEvent::BuildStarted { .. }
                | BuildEvent::DepBuildStarted { .. }
                | BuildEvent::DepBuildDone => {}
                BuildEvent::Compiling { .. } => outer(BuildEvent::DepCompiling),
                BuildEvent::DepCompiling => outer(BuildEvent::DepCompiling),
                other => outer(other),
            });
            if let Err(e) = crate::build::build_project_at(
                &dep_dir,
                profile,
                &[],
                true,
                None,
                &[],
                &inner_progress,
                Some(pkgs_root),
            ) {
                progress(BuildEvent::Warning(format!(
                    "source-build of {name} failed: {e}"
                )));
            }
            progress(BuildEvent::DepBuildDone);
            let built_lib = pkgs_root
                .join("target")
                .join("deps")
                .join(name)
                .join(profile)
                .join(format!("lib{name}.a"));
            if built_lib.exists() {
                let out_dir = built_lib.parent().unwrap().to_path_buf();
                link_flags.push(format!("-L{}", out_dir.display()));
                link_flags.push(format!("-l{name}"));
                // Also pick up include dirs from the dep itself.
                for candidate in &["include", "inc"] {
                    let d = dep_dir.join(candidate);
                    if d.is_dir() && !include_dirs.contains(&d) {
                        include_dirs.push(d);
                    }
                }
            }
        }

        return Ok(Some((
            ForeignBuilt {
                name: name.to_string(),
                libs: vec![],
                include_dirs,
                raw_link_flags: link_flags,
            },
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
        progress(BuildEvent::Warning(format!(
            "'{name}' not found at build time (optional, skipping)"
        )));
        Ok(None)
    } else {
        Err(FreightError::ManifestParse(format!(
            "dep '{name}' not found via pkg-config or system stubs; \
             run `freight fetch` to download it from the registry first"
        )))
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_backend(name: &str, dep_type: &str, dep_dir: &Path) -> Result<(), FreightError> {
    let (present, marker) = match dep_type {
        "cmake" => (dep_dir.join("CMakeLists.txt").exists(), "CMakeLists.txt"),
        "meson" => (dep_dir.join("meson.build").exists(), "meson.build"),
        "autotools" => (
            dep_dir.join("configure.ac").exists()
                || dep_dir.join("configure.in").exists()
                || dep_dir.join("autogen.sh").exists()
                || dep_dir.join("configure").exists(),
            "configure.ac / configure",
        ),
        "make" => (
            dep_dir.join("Makefile").exists() || dep_dir.join("GNUmakefile").exists(),
            "Makefile",
        ),
        "scons" => (dep_dir.join("SConstruct").exists(), "SConstruct"),
        "bazel" => (
            dep_dir.join("WORKSPACE").exists() || dep_dir.join("WORKSPACE.bazel").exists(),
            "WORKSPACE",
        ),
        "auto" | "none" => return Ok(()),
        other => {
            return Err(FreightError::ManifestParse(format!(
                "unknown type '{other}' for dep '{name}'; \
             expected: cmake, make, meson, autotools, scons, bazel, none"
            )))
        }
    };
    if !present {
        return Err(FreightError::ManifestParse(format!(
            "type '{dep_type}' specified for dep '{name}' \
             but '{marker}' not found in '{}'",
            dep_dir.display()
        )));
    }
    Ok(())
}

// ── Detection ─────────────────────────────────────────────────────────────────

pub fn detect_build_system(dep_dir: &Path) -> Option<String> {
    if dep_dir.join("CMakeLists.txt").exists() {
        return Some("cmake".into());
    }
    if dep_dir.join("meson.build").exists() {
        return Some("meson".into());
    }
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
    if dep_dir.join("SConstruct").exists() {
        return Some("scons".into());
    }
    if dep_dir.join("Makefile").exists() || dep_dir.join("GNUmakefile").exists() {
        return Some("make".into());
    }
    None
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn invoke_build_system(
    dep_dir: &Path,
    build_dir: &Path,
    name: &str,
    build_system: &str,
    profile: &str,
    defines: &[String],
    target: Option<&str>,
    progress: &Progress,
    tool_paths: &[PathBuf],
    // Install prefixes of already-built dependencies (for transitive foreign
    // deps). Passed to cmake as CMAKE_PREFIX_PATH so `find_package` finds them.
    prefix_paths: &[PathBuf],
) -> Result<Vec<PathBuf>, FreightError> {
    let resolved = build_system.to_string();

    std::fs::create_dir_all(build_dir)?;

    progress(BuildEvent::BuildingForeignDep {
        name: name.to_string(),
        backend: resolved.clone(),
    });

    // `KEY=VALUE` bodies, with any leading `-D` stripped — each builder applies
    // them in its native form below.
    let bodies: Vec<&str> = defines
        .iter()
        .map(|d| d.strip_prefix("-D").unwrap_or(d))
        .collect();

    let search_dir = match resolved.as_str() {
        "cmake" => {
            // cmake cache variables: `-DKEY=VALUE`.
            let mut args: Vec<String> = bodies.iter().map(|b| format!("-D{b}")).collect();
            if !prefix_paths.is_empty() {
                let joined = prefix_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(";");
                args.push(format!("-DCMAKE_PREFIX_PATH={joined}"));
            }
            cmake::build_cmake(dep_dir, build_dir, profile, &args, target, tool_paths)?;
            build_dir.to_path_buf()
        }
        "make" => {
            // make variable assignments: `KEY=VALUE`.
            let vars: Vec<String> = bodies.iter().map(|b| b.to_string()).collect();
            make::build_make(dep_dir, &vars, tool_paths)?;
            dep_dir.to_path_buf()
        }
        "meson" => {
            // meson project options: `-DKEY=VALUE`.
            let args: Vec<String> = bodies.iter().map(|b| format!("-D{b}")).collect();
            meson::build_meson(dep_dir, build_dir, &args, tool_paths)?;
            build_dir.to_path_buf()
        }
        "autotools" => {
            autotools::build_autotools(dep_dir, build_dir, target, tool_paths)?;
            build_dir.join("install")
        }
        "scons" => {
            scons::build_scons(dep_dir, tool_paths)?;
            dep_dir.to_path_buf()
        }
        "bazel" => {
            bazel::build_bazel(dep_dir, tool_paths)?;
            dep_dir.to_path_buf()
        }
        // Header-only / no build system: nothing to compile — the source was
        // fetched and its headers are exposed via collect_include_dirs by the
        // caller. No cmake (or any build tool) is invoked.
        "none" | "header" => dep_dir.to_path_buf(),
        other => {
            return Err(FreightError::ManifestParse(format!(
                "unknown backend '{other}' for dep '{name}'; \
                 expected: cmake, make, meson, autotools, scons, bazel, none"
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
        "c", "cpp", "cc", "cxx", "c++", "cppm", "f", "f90", "f95", "f03", "f08", "s", "asm",
        "nasm", "cu", "hip", "cl",
    ];
    fn walk(dir: &Path, depth: u8) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if depth > 0 && walk(&p, depth - 1) {
                    return true;
                }
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
        if has_executables(c) {
            found.push(c.clone());
        }
    }

    // One level deep — catches tarballs that unpack to a versioned top directory.
    if let Ok(entries) = std::fs::read_dir(dep_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            // Skip already-checked names and freight internal dirs.
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                "bin" | "install" | ".freight-build" | "include" | "lib" | "src"
            ) {
                continue;
            }
            let sub_bin = entry.path().join("bin");
            if has_executables(&sub_bin) {
                found.push(sub_bin);
            }
        }
    }

    found
}

fn has_executables(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
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

#[cfg(test)]
mod foreign_pkg_tests {
    use super::foreign_package_spec;

    fn write(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fpkg-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("freight.toml"), body).unwrap();
        dir
    }

    #[test]
    fn detects_foreign_url_build_package() {
        let dir = write("foreign",
            "[package]\nname=\"curl\"\nversion=\"8.0\"\nurl=\"https://x/c.tar.gz\"\nbuild=\"cmake\"\n");
        let spec = foreign_package_spec(&dir).expect("foreign");
        assert_eq!(spec.0.as_deref(), Some("https://x/c.tar.gz"));
        assert_eq!(spec.1, "cmake");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn topo_order_puts_deps_before_dependents() {
        use super::topo_order;
        use std::collections::BTreeMap;
        let mut g: BTreeMap<String, Vec<String>> = BTreeMap::new();
        g.insert("app".into(), vec!["mid".into()]);
        g.insert("mid".into(), vec!["base".into()]);
        g.insert("base".into(), vec![]);
        let order = topo_order(&g);
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("base") < pos("mid"));
        assert!(pos("mid") < pos("app"));
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn topo_order_tolerates_cycles() {
        use super::topo_order;
        use std::collections::BTreeMap;
        let mut g: BTreeMap<String, Vec<String>> = BTreeMap::new();
        g.insert("a".into(), vec!["b".into()]);
        g.insert("b".into(), vec!["a".into()]); // cycle
        let order = topo_order(&g);
        assert_eq!(order.len(), 2); // both present, no infinite loop
    }

    #[test]
    fn native_package_is_not_foreign() {
        // build set but a local [lib] present → native, not foreign.
        let dir = write(
            "native",
            "[package]\nname=\"x\"\nversion=\"1.0\"\nbuild=\"make\"\n[lib]\nsrcs=[\"x.c\"]\n",
        );
        assert!(foreign_package_spec(&dir).is_none());
        // no build at all → native.
        let dir2 = write("plain", "[package]\nname=\"x\"\nversion=\"1.0\"\n");
        assert!(foreign_package_spec(&dir2).is_none());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dir2).ok();
    }
}

#[cfg(test)]
mod cross_tests {
    use super::{cross_build, is_cross_triple};
    use std::path::Path;

    #[test]
    fn is_cross_triple_detects_foreign_arch_and_os() {
        let host = std::env::consts::ARCH;
        let foreign_arch = if host == "aarch64" {
            "x86_64-unknown-linux-gnu"
        } else {
            "aarch64-unknown-linux-gnu"
        };
        assert!(is_cross_triple(foreign_arch));

        // Host arch + host OS family is not cross.
        let native_os = if cfg!(target_os = "windows") {
            "pc-windows-msvc"
        } else if cfg!(target_os = "macos") {
            "apple-darwin"
        } else {
            "unknown-linux-gnu"
        };
        assert!(!is_cross_triple(&format!("{host}-{native_os}")));

        // Same arch, foreign OS is cross.
        let foreign_os = if cfg!(target_os = "linux") {
            format!("{host}-pc-windows-msvc")
        } else {
            format!("{host}-unknown-linux-gnu")
        };
        assert!(is_cross_triple(&foreign_os));
    }

    #[test]
    fn cross_build_from_sysroot_or_foreign_target() {
        let mut m =
            crate::manifest::load_manifest_str("[package]\nname=\"a\"\nversion=\"0.1.0\"\n")
                .unwrap();
        // Native: no target, no sysroot.
        assert!(cross_build(&m).is_none());

        // A sysroot alone marks a cross build.
        m.compiler.sysroot = Some("/opt/sysroot".to_string());
        let cb = cross_build(&m).expect("sysroot ⇒ cross");
        assert_eq!(cb.sysroot.as_deref(), Some(Path::new("/opt/sysroot")));

        // A foreign target triple alone marks a cross build.
        m.compiler.sysroot = None;
        let host = std::env::consts::ARCH;
        m.compiler.target = Some(if host == "aarch64" {
            "x86_64-unknown-linux-gnu".into()
        } else {
            "aarch64-unknown-linux-gnu".into()
        });
        assert!(cross_build(&m).is_some());
    }
}
