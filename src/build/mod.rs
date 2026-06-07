pub mod compile;
pub mod compile_commands;
pub mod deps;
pub(crate) mod diagnostics;
pub mod discover;
pub mod features;
pub mod header_units;
pub mod link;
pub mod modules;
pub mod pch;
pub mod pipeline;
pub mod project;
pub mod proto;

pub use compile::{
    compile_sources, compile_sources_unity, dep_file_path, emit_sources, object_path,
    primary_family, select_compiler, settings_for_lang, CompileResult, EmitTarget,
    UNITY_SUPPORTED_LANGS,
};
pub use deps::{check_slot_conflicts, resolve_dep_graph, ResolvedDep};
pub use discover::{discover, DiscoveredSources, SourceFile};
pub use link::{link_static_lib, link_targets, link_test_binary, select_linker, LinkResult};
pub use modules::{
    bmi_path, compile_module_sources, has_modules, plan_module_build, scan_sources,
    ModuleBuildPlan, ModuleRole, ScannedSource,
};
pub use pipeline::{run_pipeline_at, PipelineConfig, PipelineGoal};
pub use project::Project;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use crate::error::FreightError;
use crate::event::{silent, BuildEvent, Progress};
use crate::fetch::git;
use crate::lock::LockFile;
use crate::manifest::types::Backend;
use crate::manifest::types::{Dependency, Manifest};
use crate::manifest::validate::{validate, validate_dep_compat};
use crate::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};
use crate::toolchain::{
    backend_matches, check_manifest_version_bounds, detect_all_cached, load_all_templates,
    CompilerTemplate, DetectedCompiler, GlobalConfig,
};

// ── Shared lang helper ────────────────────────────────────────────────────────

/// Return true if `lang_key` is active in this manifest: either explicitly declared
/// via `[language.X]`, or implicitly present because at least one source file has an
/// extension handled by that language key (checked against `detected` compilers).
pub(super) fn has_lang(manifest: &Manifest, lang_key: &str, detected: &[DetectedCompiler]) -> bool {
    if manifest.language.contains_key(lang_key) {
        return true;
    }
    let exts: Vec<&str> = detected
        .iter()
        .filter_map(|d| d.template.linking.get(lang_key))
        .flat_map(|l| l.extensions.iter().map(String::as_str))
        .collect();
    if exts.is_empty() {
        return false;
    }
    let has = |src: &str| exts.iter().any(|e| src.ends_with(*e));
    manifest.bins.iter().any(|b| has(&b.src))
        || manifest
            .lib
            .as_ref()
            .map_or(false, |l| l.srcs.iter().any(|s| has(s)))
}

// ── Public results ────────────────────────────────────────────────────────────

pub struct BuildOutput {
    pub package_name: String,
    pub binaries: Vec<PathBuf>,
    pub compiled: usize,
    pub skipped: usize,
}

pub struct TestSummary {
    pub passed: usize,
    pub failed: usize,
    pub total: usize,
}

/// Wall-clock timing for one benchmark binary.
pub struct BenchResult {
    pub name: String,
    /// Mean execution time in nanoseconds across all runs.
    pub mean_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub runs: usize,
}

pub struct BenchSummary {
    pub results: Vec<BenchResult>,
}

pub enum PipelineOutput {
    Build(BuildOutput),
    Test(TestSummary),
    Bench(BenchSummary),
}

struct ProjectContext {
    project_dir: PathBuf,
    manifest: Manifest,
    effective_backend: Backend,
    templates: Vec<CompilerTemplate>,
    detected: Vec<DetectedCompiler>,
    found: DiscoveredSources,
}

/// Pre-built dep output: static lib path + include dirs to expose to the root project.
struct BuiltDeps {
    libs: Vec<PathBuf>,
    include_dirs: Vec<PathBuf>,
}

// ── Build pipeline ────────────────────────────────────────────────────────────

/// Build every member of a workspace rooted at the current working directory.
///
/// Members are built in the order declared in `[workspace].members`. If any
/// member fails, the build stops and the error is returned.
///
/// After all members are built a merged `compile_commands.json` is written to
/// the workspace root so that clangd (and other LSP clients) can serve the
/// entire workspace from a single database.
pub fn build_workspace(profile: &str) -> Result<Vec<BuildOutput>, FreightError> {
    build_workspace_with(profile, None, &[], true, &silent())
}

/// Like [`build_workspace`] but routes all progress through `progress`.
///
/// `package` — when `Some`, build only the workspace member whose directory name matches.
/// `features` / `use_defaults` — forwarded to `build_project_at` for the selected member;
/// ignored when building the entire workspace (all members use their own defaults).
pub fn build_workspace_with(
    profile: &str,
    package: Option<&str>,
    features: &[String],
    use_defaults: bool,
    progress: &Progress,
) -> Result<Vec<BuildOutput>, FreightError> {
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| FreightError::ManifestParse("not a workspace root".into()))?;

    if let Some(pkg) = package {
        let found = ws.members.iter().any(|m| {
            ws_dir
                .join(m.trim_end_matches('/'))
                .file_name()
                .and_then(|n| n.to_str())
                == Some(pkg)
        });
        if !found {
            return Err(FreightError::ManifestParse(format!(
                "package `{pkg}` not found in workspace"
            )));
        }
    }

    let mut outputs = Vec::new();
    let mut member_dirs: Vec<PathBuf> = Vec::new();
    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        let member_name = member_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if let Some(pkg) = package {
            if member_name != pkg {
                continue;
            }
        }
        let (mem_features, mem_defaults) = if package.is_some() {
            (features, use_defaults)
        } else {
            (&[][..], true)
        };
        outputs.push(build_project_at(
            &member_dir,
            profile,
            mem_features,
            mem_defaults,
            None,
            &[],
            progress,
            None,
        )?);
        member_dirs.push(member_dir);
    }

    // Merge every member's compile_commands.json into a single workspace-root
    // file so clangd / other LSP clients see the full project in one database.
    let mut all_commands: Vec<compile_commands::CompileCommand> = Vec::new();
    for dir in &member_dirs {
        all_commands.extend(compile_commands::load(dir));
    }
    all_commands.sort_by(|a, b| a.file.cmp(&b.file));
    if let Err(e) = compile_commands::write(&ws_dir, &all_commands) {
        progress(BuildEvent::Warning(format!(
            "could not write workspace compile_commands.json: {e}"
        )));
    }

    Ok(outputs)
}

/// Clean every member of a workspace rooted at the current working directory.
pub fn clean_workspace() -> Result<(), FreightError> {
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| FreightError::ManifestParse("not a workspace root".into()))?;

    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        clean_project_at(&member_dir)?;
    }
    Ok(())
}


/// Test every member of a workspace rooted at the current working directory.
pub fn test_workspace(profile: &str, filter: Option<&str>) -> Result<TestSummary, FreightError> {
    test_workspace_with(profile, filter, None, &[], true, &silent())
}

/// Like [`test_workspace`] but routes all progress through `progress`.
///
/// `package` — when `Some`, test only the member whose directory name matches.
pub fn test_workspace_with(
    profile: &str,
    filter: Option<&str>,
    package: Option<&str>,
    features: &[String],
    use_defaults: bool,
    progress: &Progress,
) -> Result<TestSummary, FreightError> {
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| FreightError::ManifestParse("not a workspace root".into()))?;

    if let Some(pkg) = package {
        let found = ws.members.iter().any(|m| {
            ws_dir
                .join(m.trim_end_matches('/'))
                .file_name()
                .and_then(|n| n.to_str())
                == Some(pkg)
        });
        if !found {
            return Err(FreightError::ManifestParse(format!(
                "package `{pkg}` not found in workspace"
            )));
        }
    }

    let mut total_passed = 0;
    let mut total_failed = 0;
    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        let member_name = member_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if let Some(pkg) = package {
            if member_name != pkg {
                continue;
            }
        }
        let (mem_features, mem_defaults) = if package.is_some() {
            (features, use_defaults)
        } else {
            (&[][..], true)
        };
        let s = test_project_at(
            &member_dir,
            profile,
            filter,
            mem_features,
            mem_defaults,
            &[],
            progress,
        )?;
        total_passed += s.passed;
        total_failed += s.failed;
    }
    Ok(TestSummary {
        passed: total_passed,
        failed: total_failed,
        total: total_passed + total_failed,
    })
}

/// Build the project rooted at the current working directory.
pub fn build_project(
    profile: &str,
    features: &[String],
    use_defaults: bool,
    sanitize_override: &[String],
) -> Result<BuildOutput, FreightError> {
    build_project_with(
        profile,
        features,
        use_defaults,
        sanitize_override,
        &silent(),
    )
}

/// Like [`build_project`] but routes all progress through `progress`.
pub fn build_project_with(
    profile: &str,
    features: &[String],
    use_defaults: bool,
    sanitize_override: &[String],
    progress: &Progress,
) -> Result<BuildOutput, FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    build_project_at(
        &project_dir,
        profile,
        features,
        use_defaults,
        None,
        sanitize_override,
        progress,
        None,
    )
}

/// Build the project at a specific `project_dir`.
pub fn build_project_at(
    project_dir: &Path,
    profile: &str,
    features: &[String],
    use_defaults: bool,
    target_override: Option<&str>,
    sanitize_override: &[String],
    progress: &Progress,
    parent_root: Option<&Path>,
) -> Result<BuildOutput, FreightError> {
    let config = PipelineConfig {
        profile: profile.to_string(),
        features: features.to_vec(),
        use_defaults,
        target_override: target_override.map(str::to_string),
        sanitize_override: sanitize_override.to_vec(),
        goal: PipelineGoal::Build,
    };
    match run_pipeline_at(project_dir, &config, parent_root, progress)? {
        PipelineOutput::Build(out) => Ok(out),
        _ => unreachable!(),
    }
}

/// Generate `compile_commands.json` without running a full build.
///
/// Resolves the dep graph to collect dep include dirs (no compilation), so
/// entries include the same `-I` flags that a real build would use. Returns
/// the number of entries written.
pub fn generate_compile_commands_at(
    project_dir: &Path,
    profile: &str,
) -> Result<usize, FreightError> {
    let ctx = load_project_at(project_dir, profile)?;
    let ProjectContext {
        project_dir,
        manifest,
        effective_backend,
        templates: _,
        detected,
        found,
    } = &ctx;

    let resolution = features::resolve_features(&manifest.features, &[], true)?;
    let feature_defines = features::to_defines(&resolution.active);

    // Collect dep include dirs without triggering compilation.  Resolution
    // failures are non-fatal — we fall back to project-local dirs only.
    let dep_includes = collect_dep_include_dirs(project_dir, manifest);
    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(dep_includes);

    let target_dir = project_dir.join("target");
    let commands = compile_commands::generate_incremental(
        project_dir,
        &target_dir,
        manifest,
        effective_backend,
        detected,
        profile,
        &found.sources,
        &include_dirs,
        &feature_defines,
        &[],
        None,
    );
    // Merge compile_commands from source-built deps.
    let commands = {
        let mut merged = commands;
        let pkgs_dir = project_dir.join(".pkgs");
        let lsp_sub = std::path::Path::new(".freight")
            .join("lsp")
            .join(safe_lsp_profile_dir(profile));
        if let Ok(entries) = std::fs::read_dir(&pkgs_dir) {
            for entry in entries.flatten() {
                let dep_cc = entry.path().join(&lsp_sub).join("compile_commands.json");
                if dep_cc.exists() {
                    merged = compile_commands::merge(merged, compile_commands::load_from(&dep_cc));
                }
            }
        }
        merged
    };

    let count = commands.len();
    let lsp_dir = lsp_compile_commands_dir(project_dir, profile);
    compile_commands::write_to(&lsp_dir.join("compile_commands.json"), &commands).and_then(
        |_| {
            compile_commands::write_incremental_cache(
                project_dir,
                manifest,
                effective_backend,
                detected,
                profile,
                &found.sources,
                &include_dirs,
                &feature_defines,
                &[],
            )
        },
    )?;
    Ok(count)
}

/// Generate the compile database used internally by `freight lsp`.
///
/// Unlike [`generate_compile_commands_at`], this writes to `.freight/lsp/<profile>/`
/// so editor integrations can point source language servers at Freight's
/// manifest-scoped view without adding `compile_commands.json` to the project
/// root or the explorer.
pub fn generate_lsp_compile_commands_at(
    project_dir: &Path,
    profile: &str,
) -> Result<PathBuf, FreightError> {
    if load_workspace_manifest(project_dir).is_some() {
        return generate_lsp_workspace_compile_commands_at(project_dir, profile);
    }
    let commands = generate_lsp_compile_commands_for_project(project_dir, profile)?;
    let dir = lsp_compile_commands_dir(project_dir, profile);
    compile_commands::write_to(&dir.join("compile_commands.json"), &commands)?;
    Ok(dir)
}

fn generate_lsp_workspace_compile_commands_at(
    workspace_dir: &Path,
    profile: &str,
) -> Result<PathBuf, FreightError> {
    let ws = load_workspace_manifest(workspace_dir)
        .ok_or_else(|| FreightError::ManifestParse("not a workspace root".into()))?;
    let mut commands = Vec::new();
    for member in &ws.members {
        let member_dir = workspace_dir.join(member.trim_end_matches('/'));
        commands.extend(generate_lsp_compile_commands_for_project(
            &member_dir,
            profile,
        )?);
    }
    commands.sort_by(|a, b| a.file.cmp(&b.file));
    let dir = lsp_compile_commands_dir(workspace_dir, profile);
    compile_commands::write_to(&dir.join("compile_commands.json"), &commands)?;
    Ok(dir)
}

/// Return compile flags for every source file in the project at `project_dir`,
/// extracted directly from the build context — no filesystem write.
///
/// Keys are absolute source paths; values are `(compiler_binary, working_dir, flags)` where
/// - `compiler_binary` is `arguments[0]` before stripping
/// - `working_dir` is the project root from which relative include paths resolve
/// - `flags` contains no compiler binary, no `-c`, and no `-o <path>`
pub fn lsp_source_flags(
    project_dir: &Path,
    profile: &str,
) -> Result<HashMap<PathBuf, (String, String, Vec<String>)>, FreightError> {
    let commands = generate_lsp_compile_commands_for_project(project_dir, profile)?;
    let mut map = HashMap::new();
    for cmd in commands {
        let dir = cmd.directory.to_string_lossy().into_owned();
        let mut args = cmd.arguments.into_iter();
        let compiler = args.next().unwrap_or_default();
        let file_str = cmd.file.to_string_lossy().into_owned();
        let flags: Vec<String> = args
            .scan(false, |skip_next, arg| {
                if *skip_next {
                    *skip_next = false;
                    return Some(None);
                }
                if arg == "-c" {
                    return Some(None);
                }
                if arg == "-o" {
                    *skip_next = true;
                    return Some(None);
                }
                Some(Some(arg))
            })
            .flatten()
            .filter(|f| f != &file_str)
            .collect();
        map.insert(cmd.file, (compiler, dir, flags));
    }
    Ok(map)
}

fn generate_lsp_compile_commands_for_project(
    project_dir: &Path,
    profile: &str,
) -> Result<Vec<compile_commands::CompileCommand>, FreightError> {
    let ctx = load_project_at(project_dir, profile)?;
    let ProjectContext {
        project_dir,
        manifest,
        effective_backend,
        templates: _,
        detected,
        found,
    } = &ctx;

    let resolution = features::resolve_features(&manifest.features, &[], true)?;
    let feature_defines = features::to_defines(&resolution.active);

    let (dep_includes, dep_roots) = collect_dep_include_dirs_and_roots(project_dir, manifest);
    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(dep_includes);
    let include_dirs = lsp_visible_include_dirs(project_dir, &dep_roots, include_dirs);

    Ok(compile_commands::generate(
        project_dir,
        &project_dir.join("target"),
        manifest,
        effective_backend,
        detected,
        profile,
        &found.sources,
        &include_dirs,
        &feature_defines,
        &[],
    ))
}

fn lsp_compile_commands_dir(project_dir: &Path, profile: &str) -> PathBuf {
    project_dir
        .join(".freight")
        .join("lsp")
        .join(safe_lsp_profile_dir(profile))
}

fn safe_lsp_profile_dir(profile: &str) -> String {
    let safe: String = profile
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if safe.is_empty() {
        "dev".to_string()
    } else {
        safe
    }
}

fn lsp_visible_include_dirs(
    project_dir: &Path,
    dep_roots: &[PathBuf],
    include_dirs: Vec<PathBuf>,
) -> Vec<PathBuf> {
    let project_root = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let dep_roots: Vec<PathBuf> = dep_roots
        .iter()
        .map(|dir| dir.canonicalize().unwrap_or_else(|_| dir.clone()))
        .collect();
    include_dirs
        .into_iter()
        .filter(|dir| {
            if dir.is_relative() {
                return true;
            }
            let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            canonical.starts_with(&project_root)
                || is_standard_c_family_include_dir(&canonical)
                || dep_roots.iter().any(|root| canonical.starts_with(root))
        })
        .collect()
}

fn is_standard_c_family_include_dir(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.contains("/lib/clang/") && text.ends_with("/include") {
        return true;
    }
    if text.contains("/include/c++/") {
        return true;
    }
    if text.ends_with("/include/c++") || text.ends_with("/include/c++/v1") {
        return true;
    }
    false
}

/// Collect every dep's exported include dirs without compiling anything.
///
/// Used by `generate_compile_commands_at` so the standalone
/// `freight compile-commands` command produces complete `-I` flags even when
/// the project has not been built yet.  Resolution errors are silently ignored.
fn collect_dep_include_dirs(project_dir: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    collect_dep_include_dirs_and_roots(project_dir, manifest).0
}

fn collect_dep_include_dirs_and_roots(
    project_dir: &Path,
    manifest: &Manifest,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let empty = BTreeSet::new();
    let mut roots = Vec::new();
    let mut includes = Vec::new();
    let mut seen_roots = std::collections::HashSet::new();
    let mut seen_includes = std::collections::HashSet::new();

    if let Ok(resolved) = resolve_dep_graph(project_dir, manifest, false, &empty) {
        for dep in resolved {
            push_unique_existing_dir(&mut roots, &mut seen_roots, dep.dir.clone());
            for include in deps::dep_include_dirs(&dep.dir, &dep.manifest) {
                push_unique_existing_dir(&mut includes, &mut seen_includes, include);
            }
        }
    }

    collect_cached_package_dep_includes(
        project_dir,
        manifest,
        &mut includes,
        &mut roots,
        &mut seen_includes,
        &mut seen_roots,
    );

    (includes, roots)
}

fn collect_cached_package_dep_includes(
    root_dir: &Path,
    manifest: &Manifest,
    includes: &mut Vec<PathBuf>,
    roots: &mut Vec<PathBuf>,
    seen_includes: &mut std::collections::HashSet<PathBuf>,
    seen_roots: &mut std::collections::HashSet<PathBuf>,
) {
    for (name, dep) in manifest.effective_dependencies() {
        if crate::manifest::types::is_platform_dep(&name) {
            continue;
        }
        if matches!(&dep, Dependency::Detailed(d) if d.optional) {
            continue;
        }
        let Some(dep_dir) = cached_package_dep_dir(root_dir, &name, &dep) else {
            continue;
        };
        if !dep_dir.is_dir() {
            continue;
        }
        let first_seen = push_unique_existing_dir(roots, seen_roots, dep_dir.clone());

        let explicit_includes = match &dep {
            Dependency::Detailed(d) => d.include.as_slice(),
            Dependency::Simple(_) => &[],
        };
        let build_dir = dep_dir.join(".freight-build");
        for include in
            crate::adaptors::collect_include_dirs(&dep_dir, explicit_includes, Some(&build_dir))
        {
            push_unique_existing_dir(includes, seen_includes, include);
        }

        if first_seen {
            if let Ok(dep_manifest) = load_manifest(&dep_dir) {
                collect_cached_package_dep_includes(
                    root_dir,
                    &dep_manifest,
                    includes,
                    roots,
                    seen_includes,
                    seen_roots,
                );
            }
        }
    }
}

fn cached_package_dep_dir(root_dir: &Path, name: &str, dep: &Dependency) -> Option<PathBuf> {
    match dep {
        Dependency::Simple(_) => Some(root_dir.join(".pkgs").join(name)),
        Dependency::Detailed(d)
            if d.path.is_none()
                && d.registry.as_deref() != Some("system")
                && (d.version.is_some() || d.url.is_some() || d.is_git()) =>
        {
            Some(root_dir.join(".pkgs").join(name))
        }
        _ => None,
    }
}

fn push_unique_existing_dir(
    out: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<PathBuf>,
    dir: PathBuf,
) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    if seen.insert(canonical) {
        out.push(dir);
        true
    } else {
        false
    }
}

/// Wipe the project's `target/` directory (finds project by walking up from cwd).
pub fn clean_project() -> Result<(), FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    clean_project_at(&project_dir)
}

/// Wipe the `target/` directory of the project at `project_dir`.
pub fn clean_project_at(project_dir: &Path) -> Result<(), FreightError> {
    let target = project_dir.join("target");
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    Ok(())
}

/// Build and run the tests of the project rooted at the current working directory.
pub fn test_project(
    profile: &str,
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
    sanitize_override: &[String],
) -> Result<TestSummary, FreightError> {
    test_project_with(
        profile,
        filter,
        features,
        use_defaults,
        sanitize_override,
        &silent(),
    )
}

/// Like [`test_project`] but routes all progress through `progress`.
pub fn test_project_with(
    profile: &str,
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
    sanitize_override: &[String],
    progress: &Progress,
) -> Result<TestSummary, FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    test_project_at(
        &project_dir,
        profile,
        filter,
        features,
        use_defaults,
        sanitize_override,
        progress,
    )
}

/// Build and execute the project's test binaries.
pub fn test_project_at(
    project_dir: &Path,
    profile: &str,
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
    sanitize_override: &[String],
    progress: &Progress,
) -> Result<TestSummary, FreightError> {
    let config = PipelineConfig {
        profile: profile.to_string(),
        features: features.to_vec(),
        use_defaults,
        target_override: None,
        sanitize_override: sanitize_override.to_vec(),
        goal: PipelineGoal::Test {
            filter: filter.map(str::to_string),
        },
    };
    match run_pipeline_at(project_dir, &config, None, progress)? {
        PipelineOutput::Test(out) => Ok(out),
        _ => unreachable!(),
    }
}

// ── Bench pipeline ────────────────────────────────────────────────────────────

/// Benchmark every member of a workspace (or a specific member when `package` is given).
pub fn bench_workspace_with(
    filter: Option<&str>,
    package: Option<&str>,
    features: &[String],
    use_defaults: bool,
    progress: &Progress,
) -> Result<BenchSummary, FreightError> {
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| FreightError::ManifestParse("not a workspace root".into()))?;

    if let Some(pkg) = package {
        let found = ws.members.iter().any(|m| {
            ws_dir
                .join(m.trim_end_matches('/'))
                .file_name()
                .and_then(|n| n.to_str())
                == Some(pkg)
        });
        if !found {
            return Err(FreightError::ManifestParse(format!(
                "package `{pkg}` not found in workspace"
            )));
        }
    }

    let mut all: Vec<BenchResult> = Vec::new();
    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        let member_name = member_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if let Some(pkg) = package {
            if member_name != pkg {
                continue;
            }
        }
        let (mem_features, mem_defaults) = if package.is_some() {
            (features, use_defaults)
        } else {
            (&[][..], true)
        };
        let s = bench_project_at(
            &member_dir,
            "bench",
            filter,
            mem_features,
            mem_defaults,
            progress,
        )?;
        all.extend(s.results);
    }
    Ok(BenchSummary { results: all })
}

/// Build and run benchmark binaries found in `benches/`.
pub fn bench_project(
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
) -> Result<BenchSummary, FreightError> {
    bench_project_with(filter, features, use_defaults, &silent())
}

/// Like [`bench_project`] but routes progress through `progress`.
pub fn bench_project_with(
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
    progress: &Progress,
) -> Result<BenchSummary, FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    bench_project_at(
        &project_dir,
        "bench",
        filter,
        features,
        use_defaults,
        progress,
    )
}

/// Build and run benchmark binaries for the project at `project_dir`.
pub fn bench_project_at(
    project_dir: &Path,
    profile: &str,
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
    progress: &Progress,
) -> Result<BenchSummary, FreightError> {
    let config = PipelineConfig {
        profile: profile.to_string(),
        features: features.to_vec(),
        use_defaults,
        target_override: None,
        sanitize_override: vec![],
        goal: PipelineGoal::Bench {
            filter: filter.map(str::to_string),
        },
    };
    match run_pipeline_at(project_dir, &config, None, progress)? {
        PipelineOutput::Bench(out) => Ok(out),
        _ => unreachable!(),
    }
}

// ── Git dep helpers ───────────────────────────────────────────────────────────

/// Auto-clone any git deps whose `.deps/<name>/` directory doesn't exist yet.
/// Runs silently when all deps are present.
fn ensure_git_deps_fetched(
    project_dir: &Path,
    manifest: &Manifest,
    progress: &Progress,
) -> Result<(), FreightError> {
    let deps_dir = project_dir.join(".pkgs");

    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else {
            continue;
        };
        let Some(url) = &d.url else { continue };

        let dest = deps_dir.join(name);
        if dest.exists() {
            continue;
        }

        progress(BuildEvent::FetchingDep {
            name: name.clone(),
            source: format!("git+{url}"),
        });
        std::fs::create_dir_all(&deps_dir)?;
        git::clone_dep(
            &dest,
            url,
            d.branch.as_deref(),
            d.tag.as_deref(),
            d.rev.as_deref(),
        )?;
    }

    Ok(())
}

/// After the dep graph is resolved, check each git dep's current commit against
/// the lock file. If a dep was pinned with `rev =` in the manifest, silently
/// enforce the pin by checking out that exact SHA. For branch-tracked deps,
/// print a warning when the repo has drifted from the locked SHA so the user
/// knows to run `freight update`.
fn verify_git_dep_shas(
    project_dir: &Path,
    manifest: &Manifest,
    lock: &LockFile,
    progress: &Progress,
) {
    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else {
            continue;
        };
        if !d.is_git() {
            continue;
        };

        let dep_dir = project_dir.join(".pkgs").join(name);
        if !dep_dir.exists() {
            continue;
        }

        let current = match git::current_rev(&dep_dir) {
            Some(sha) => sha,
            None => continue,
        };

        // Find the SHA the lock file recorded for this dep.
        let locked_sha = lock
            .packages
            .iter()
            .find(|p| &p.name == name)
            .and_then(|p| p.source.as_deref())
            .and_then(|src| src.split('#').nth(1))
            .map(str::to_string);

        let Some(locked) = locked_sha else { continue };

        // Rev-pinned: enforce the exact SHA.
        if let Some(pinned) = &d.rev {
            if !current.starts_with(pinned.as_str()) {
                if let Err(e) = git::checkout_rev(&dep_dir, pinned) {
                    progress(BuildEvent::Warning(format!(
                        "could not checkout pinned rev for `{name}`: {e}"
                    )));
                }
            }
            continue;
        }

        // Branch/tag tracked: warn on drift.
        if !current.starts_with(locked.as_str()) && !locked.starts_with(current.as_str()) {
            progress(BuildEvent::Warning(format!(
                "git dep `{name}` is at {}, lock expects {}; run `freight update` to record the new SHA",
                &current[..current.len().min(12)],
                &locked[..locked.len().min(12)],
            )));
        }
    }
}

// ── Source compilation (module-aware) ────────────────────────────────────────

/// Compile a project's sources, automatically switching to the module-aware pipeline
/// if any C++ source file contains an `export module` declaration.
fn build_sources(
    project_dir: &Path,
    target_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    feature_defines: &[String],
    header_unit_flags: &[String],
    progress: &Progress,
) -> Result<CompileResult, FreightError> {
    let scanned = scan_sources(project_dir, sources);
    if has_modules(&scanned) {
        let mut plan = plan_module_build(project_dir, target_dir, profile, scanned)?;
        compile_module_sources(
            project_dir,
            target_dir,
            manifest,
            backend,
            profile,
            &mut plan,
            include_dirs,
            detected,
            feature_defines,
            header_unit_flags,
            progress,
        )
    } else if manifest.compiler.unity {
        compile::compile_sources_unity(
            project_dir,
            target_dir,
            manifest,
            backend,
            profile,
            sources,
            include_dirs,
            detected,
            feature_defines,
            header_unit_flags,
            progress,
        )
    } else {
        compile_sources(
            project_dir,
            target_dir,
            manifest,
            backend,
            profile,
            sources,
            include_dirs,
            detected,
            feature_defines,
            header_unit_flags,
            progress,
        )
    }
}

// ── Dependency build step ─────────────────────────────────────────────────────

/// Compile and archive all local deps in topological order.
///
/// Takes an already-resolved dep list so callers can reuse the resolution for
/// lockfile generation without a second walk. `root_manifest` is used to look
/// up per-dep feature requests declared by the root project.
fn build_resolved_deps(
    root_manifest: &Manifest,
    _project_dir: &Path,
    backend: &Backend,
    profile: &str,
    templates: &[CompilerTemplate],
    detected: &[DetectedCompiler],
    resolved: &[ResolvedDep],
    progress: &Progress,
) -> Result<BuiltDeps, FreightError> {
    if resolved.is_empty() {
        return Ok(BuiltDeps {
            libs: vec![],
            include_dirs: vec![],
        });
    }

    let mut libs: Vec<PathBuf> = Vec::new();
    let mut all_include_dirs: Vec<PathBuf> = Vec::new();
    // Accumulate include dirs from already-built deps so later deps can see them.
    let mut built_include_dirs: Vec<PathBuf> = Vec::new();

    for dep in resolved {
        // Resolve which features are active for this dep based on the root's dep declaration.
        let dep_feature_defines = {
            let effective = root_manifest.effective_dependencies();
            let (req, use_defaults) = effective
                .get(&dep.name)
                .and_then(|d| {
                    if let Dependency::Detailed(d) = d {
                        Some(d)
                    } else {
                        None
                    }
                })
                .map(|d| (d.features.clone(), d.default_features))
                .unwrap_or_default();
            let resolution =
                features::resolve_features(&dep.manifest.features, &req, use_defaults)?;
            features::to_defines(&resolution.active)
        };

        let dep_found = discover(&dep.dir, &dep.manifest, templates);

        // Collect the include dirs this dep exports regardless of whether it has sources.
        let exported_includes = deps::dep_include_dirs(&dep.dir, &dep.manifest);

        if dep_found.sources.is_empty() {
            // Header-only dep: contribute include dirs but no lib.
            all_include_dirs.extend(exported_includes.iter().cloned());
            built_include_dirs.extend(exported_includes);
            continue;
        }

        // Include dirs for compiling this dep: its own + all previously built dep includes.
        let mut dep_include_dirs = dep_found.include_dirs.clone();
        dep_include_dirs.extend(built_include_dirs.iter().cloned());

        progress(BuildEvent::BuildStarted {
            name: dep.name.clone(),
            profile: profile.to_string(),
        });

        // Unity override: root manifest's `mylib = { ..., unity = true/false }` wins over dep's own flag.
        let effective_unity = root_manifest
            .effective_dependencies()
            .get(&dep.name)
            .and_then(|d| {
                if let Dependency::Detailed(dd) = d {
                    dd.unity
                } else {
                    None
                }
            })
            .unwrap_or(dep.manifest.compiler.unity);

        let dep_target_dir = dep.dir.join("target");
        let compile_result = if effective_unity {
            compile::compile_sources_unity(
                &dep.dir,
                &dep_target_dir,
                &dep.manifest,
                backend,
                profile,
                &dep_found.sources,
                &dep_include_dirs,
                detected,
                &dep_feature_defines,
                &[],
                progress,
            )?
        } else {
            compile_sources(
                &dep.dir,
                &dep_target_dir,
                &dep.manifest,
                backend,
                profile,
                &dep_found.sources,
                &dep_include_dirs,
                detected,
                &dep_feature_defines,
                &[],
                progress,
            )?
        };

        let lib_out = dep_target_dir
            .join(profile)
            .join(format!("lib{}.a", dep.name));
        std::fs::create_dir_all(lib_out.parent().expect("lib_out has parent"))?;

        if !lib_out.exists() || compile_result.compiled > 0 {
            progress(BuildEvent::Archiving {
                name: format!("lib{}.a", dep.name),
            });
            let ar = select_linker(&dep.manifest, backend, detected, templates)
                .map(|l| l.template.ar_binary().to_owned())
                .unwrap_or_else(|| "ar".to_owned());
            link_static_lib(&compile_result.objects, &lib_out, &ar)?;
        }

        libs.push(lib_out);
        all_include_dirs.extend(exported_includes.iter().cloned());
        built_include_dirs.extend(exported_includes);
    }

    Ok(BuiltDeps {
        libs,
        include_dirs: all_include_dirs,
    })
}

// ── Sanitizer override helper ─────────────────────────────────────────────────

/// Patch the active profile's sanitize list in-place.
/// Creates the profile entry if it does not exist yet.
fn apply_sanitize_override(
    manifest: &mut crate::manifest::types::Manifest,
    profile: &str,
    sanitize: &[String],
) {
    let list = sanitize.to_vec();
    match profile {
        "dev" => {
            manifest
                .profile
                .dev
                .get_or_insert_with(Default::default)
                .sanitize = list;
        }
        "release" => {
            manifest
                .profile
                .release
                .get_or_insert_with(Default::default)
                .sanitize = list;
        }
        other => {
            manifest
                .profile
                .custom
                .entry(other.to_string())
                .or_insert_with(Default::default)
                .sanitize = list;
        }
    }
}

// ── Shared project loading ────────────────────────────────────────────────────

fn load_project_at(project_dir: &Path, _profile: &str) -> Result<ProjectContext, FreightError> {
    let mut manifest = load_manifest(project_dir)?;
    let mut global = GlobalConfig::load();
    if let Some(local) = GlobalConfig::load_local(project_dir) {
        global.apply_local(local);
    }
    // Project manifest backend wins over global config; "auto" defers to the next level.
    let configured_backend = if !manifest.compiler.backend.is_auto() {
        manifest.compiler.backend.clone()
    } else {
        global
            .default_backend
            .clone()
            .map(Backend)
            .unwrap_or_default()
    };
    manifest.compiler.target = global.target.clone();
    manifest.compiler.sysroot = std::env::var_os("FREIGHT_SYSROOT")
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
        .or_else(|| global.sysroot.clone());
    manifest.compiler.auto_cpu_tuning = global.auto_cpu_tuning.unwrap_or(true);

    let templates = load_all_templates();

    validate_or_fail(&manifest, project_dir, &templates)?;

    let detected = detect_all_cached(&templates);
    if detected.is_empty() {
        return Err(FreightError::CompilerNotFound(
            "no compilers found on PATH — run `freight toolchain list`".into(),
        ));
    }

    // If the configured backend isn't actually detected, fall back to auto so
    // the build doesn't fail for compilers that aren't installed (e.g. a global
    // `default_backend = "tcc"` set on a machine that doesn't have TCC).
    let effective_backend = if configured_backend.is_auto()
        || detected
            .iter()
            .any(|d| backend_matches(d, configured_backend.name()))
    {
        configured_backend
    } else {
        Backend::default()
    };

    let found = discover(project_dir, &manifest, &templates);
    // Allow projects whose only source files are `.proto` — protoc will generate
    // C++ sources at build time.  For all other projects, fail fast if src/ is empty.
    let proto_only = found.sources.is_empty()
        && manifest.language.contains_key("proto")
        && proto::has_proto_files(project_dir);
    if found.sources.is_empty() && !proto_only {
        return Err(FreightError::CompilerNotFound(
            "no source files found under src/".into(),
        ));
    }

    Ok(ProjectContext {
        project_dir: project_dir.to_path_buf(),
        manifest,
        effective_backend,
        templates,
        detected,
        found,
    })
}

fn validate_or_fail(
    manifest: &Manifest,
    project_dir: &Path,
    templates: &[CompilerTemplate],
) -> Result<(), FreightError> {
    let mut errors = validate(manifest, templates);
    errors.extend(validate_dep_compat(manifest, project_dir, templates));
    if errors.is_empty() {
        return Ok(());
    }
    let msgs: Vec<String> = errors
        .iter()
        .map(|e| format!("{}: {}", e.context, e.message))
        .collect();
    Err(FreightError::ManifestParse(msgs.join("\n")))
}

/// Run `compiler_option` and `language_option` handlers declared in Rhai templates,
/// injecting any resulting flags into the manifest before compilation starts.
///
/// - `language_option` handlers: injected per-language into `manifest.language[key].injected_flags`
/// - `compiler_option` handlers: injected into `manifest.compiler.flags` (active compiler only)
///
/// For `[compiler.<name>]` sections: if the named compiler is detected but not the active
/// backend for any discovered language, handlers still run for validation but flags are discarded.
/// If the compiler is not detected at all, the section is skipped silently.
fn inject_option_handler_flags(ctx: &mut ProjectContext) -> Result<(), FreightError> {
    let arch = ctx
        .manifest
        .target
        .arch
        .as_deref()
        .unwrap_or(std::env::consts::ARCH);
    let os = std::env::consts::OS;

    // Collect the set of language keys actually present in discovered sources.
    let discovered_lang_keys: std::collections::HashSet<String> = ctx
        .found
        .sources
        .iter()
        .map(|s| s.lang_key.clone())
        .collect();

    // Language-option handlers: look up the active compiler for each discovered or configured
    // language. Running with an empty option map allows template-declared defaults to apply
    // even when `[language.<key>]` omits the option.
    let lang_keys: std::collections::HashSet<String> = ctx
        .manifest
        .language
        .keys()
        .cloned()
        .chain(discovered_lang_keys.iter().cloned())
        .collect();
    for lang_key in lang_keys {
        let extra = ctx.manifest.effective_language_settings(&lang_key).extra;
        let Some(compiler) =
            compile::select_compiler(&lang_key, &ctx.effective_backend, &ctx.detected, None)
        else {
            continue;
        };
        let version = compiler.version.clone();
        let template_name = compiler.template.name.clone();
        let flags = ctx
            .detected
            .iter()
            .find(|d| d.template.name == template_name)
            .map(|d| {
                d.template
                    .run_language_option_handlers(&extra, &version, arch, os)
            })
            .transpose()?
            .unwrap_or_default();
        ctx.manifest
            .language
            .entry(lang_key)
            .or_default()
            .injected_flags
            .extend(flags);
    }

    // Compiler-option handlers: dispatched per active compiler and per [compiler.<name>] section.
    // Running active compiler handlers with an empty option map allows template-declared defaults
    // to apply even when `[compiler.<name>]` omits the option.
    let active_tool_names: std::collections::HashSet<String> = discovered_lang_keys
        .iter()
        .filter_map(|lang_key| {
            compile::select_compiler(lang_key, &ctx.effective_backend, &ctx.detected, None)
                .map(|d| d.template.name.clone())
        })
        .collect();

    let mut per_tool: std::collections::HashMap<String, std::collections::HashMap<String, String>> =
        active_tool_names
            .iter()
            .map(|name| (name.clone(), std::collections::HashMap::new()))
            .collect();

    // Expand manifest [compiler.<key>] entries: exact name match wins; if the key matches a
    // template's `alias` field, fan the options out to that template too (lower priority).
    for (key, val) in &ctx.manifest.compiler.per_tool {
        // Always apply to the exact-named tool (if detected).
        per_tool
            .entry(key.clone())
            .or_default()
            .extend(val.options.clone());

        // Also apply to any detected compiler whose `alias` matches this key.
        for d in &ctx.detected {
            if d.template.alias.as_deref() == Some(key.as_str()) && d.template.name != *key {
                // Alias match: insert first so exact-name entries can override below.
                let slot = per_tool.entry(d.template.name.clone()).or_default();
                for (k, v) in &val.options {
                    slot.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
    }

    for (tool_name, options) in per_tool {
        let Some(compiler) = ctx.detected.iter().find(|d| d.template.name == tool_name) else {
            continue; // not detected — skip silently
        };
        let version = compiler.version.clone();
        // Version requirement: prefer exact-name entry; fall back to the alias entry.
        let alias_key = compiler.template.alias.as_deref();
        let version_req = ctx
            .manifest
            .compiler
            .per_tool
            .get(&tool_name)
            .or_else(|| alias_key.and_then(|a| ctx.manifest.compiler.per_tool.get(a)))
            .and_then(|o| o.version.as_deref());
        check_manifest_version_bounds(&tool_name, &version, version_req)?;
        let flags = compiler
            .template
            .run_compiler_option_handlers(&options, &version, arch, os)?;

        // Only propagate flags if this compiler is the active backend for at least one
        // discovered language. If detected but not active, handlers ran for validation only.
        if active_tool_names.contains(&tool_name) {
            ctx.manifest.compiler.flags.extend(flags);
        }
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::{
        collect_dep_include_dirs_and_roots, is_standard_c_family_include_dir,
        lsp_compile_commands_dir, lsp_visible_include_dirs, safe_lsp_profile_dir,
    };
    use crate::manifest::load_manifest;
    use std::path::PathBuf;

    #[test]
    fn lsp_include_filter_keeps_project_paths() {
        let dir = tempfile::tempdir().unwrap();
        let include = dir.path().join("include");
        std::fs::create_dir_all(&include).unwrap();

        let filtered = lsp_visible_include_dirs(dir.path(), &[], vec![include.clone()]);
        assert_eq!(filtered, vec![include]);
    }

    #[test]
    fn lsp_include_filter_keeps_explicit_dep_paths() {
        let dir = tempfile::tempdir().unwrap();
        let dep = tempfile::tempdir().unwrap();
        let include = dep.path().join("include");
        std::fs::create_dir_all(&include).unwrap();

        let filtered = lsp_visible_include_dirs(
            dir.path(),
            &[dep.path().to_path_buf()],
            vec![include.clone()],
        );
        assert_eq!(filtered, vec![include]);
    }

    #[test]
    fn lsp_include_filter_drops_broad_system_paths() {
        let dir = tempfile::tempdir().unwrap();
        let filtered = lsp_visible_include_dirs(
            dir.path(),
            &[],
            vec![
                PathBuf::from("/usr/include"),
                PathBuf::from("/opt/local/include"),
            ],
        );
        assert!(
            filtered.is_empty(),
            "hidden LSP compile DB should not expose broad system include directories"
        );
    }

    #[test]
    fn lsp_include_filter_allows_c_family_standard_dirs() {
        assert!(is_standard_c_family_include_dir(&PathBuf::from(
            "/usr/include/c++/13"
        )));
        assert!(is_standard_c_family_include_dir(&PathBuf::from(
            "/usr/lib/llvm-18/lib/clang/18/include"
        )));
    }

    #[test]
    fn lsp_compile_commands_live_under_freight_dir() {
        let project = PathBuf::from("project");
        let dir = lsp_compile_commands_dir(&project, "dev/debug");

        assert_eq!(dir, project.join(".freight/lsp/dev_debug"));
    }

    #[test]
    fn lsp_profile_dir_is_path_safe() {
        assert_eq!(safe_lsp_profile_dir("../release"), "___release");
        assert_eq!(safe_lsp_profile_dir(""), "dev");
    }

    #[test]
    fn lsp_compile_commands_include_cached_version_dep_headers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/main.cpp"),
            "#include \"vecmath/vec2.h\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("freight.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"

[[bin]]
name = "app"
src = "src/main.cpp"

[dependencies]
vecmath = "0.1.1"
"#,
        )
        .unwrap();

        let vecmath = dir.path().join(".pkgs/vecmath");
        std::fs::create_dir_all(vecmath.join("include/vecmath")).unwrap();
        std::fs::write(vecmath.join("include/vecmath/vec2.h"), "#pragma once\n").unwrap();
        std::fs::write(
            vecmath.join("freight.toml"),
            r#"
[package]
name = "vecmath"
version = "0.1.1"

[compiler]
includes = ["include"]

[lib]
type = "static"
srcs = []
hdrs = ["include/vecmath/vec2.h"]
"#,
        )
        .unwrap();

        let manifest = load_manifest(dir.path()).unwrap();
        let (includes, roots) = collect_dep_include_dirs_and_roots(dir.path(), &manifest);

        assert!(roots.iter().any(|root| root.ends_with(".pkgs/vecmath")));
        assert!(includes
            .iter()
            .any(|include| include.ends_with(".pkgs/vecmath/include")));

        let filtered = lsp_visible_include_dirs(dir.path(), &roots, includes);
        assert!(filtered
            .iter()
            .any(|include| include.ends_with(".pkgs/vecmath/include")));
    }

    #[test]
    fn lsp_compile_commands_include_transitive_cached_version_dep_headers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("freight.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"

[dependencies]
vecmath = "0.1.1"
"#,
        )
        .unwrap();

        let vecmath = dir.path().join(".pkgs/vecmath");
        std::fs::create_dir_all(vecmath.join("include/vecmath")).unwrap();
        std::fs::write(vecmath.join("include/vecmath/vec2.h"), "#pragma once\n").unwrap();
        std::fs::write(
            vecmath.join("freight.toml"),
            r#"
[package]
name = "vecmath"
version = "0.1.1"

[dependencies]
mathlib = "0.1.0"
"#,
        )
        .unwrap();

        let mathlib = dir.path().join(".pkgs/mathlib");
        std::fs::create_dir_all(mathlib.join("include/mathlib")).unwrap();
        std::fs::write(mathlib.join("include/mathlib/mathlib.h"), "#pragma once\n").unwrap();
        std::fs::write(
            mathlib.join("freight.toml"),
            r#"
[package]
name = "mathlib"
version = "0.1.0"
"#,
        )
        .unwrap();

        let manifest = load_manifest(dir.path()).unwrap();
        let (includes, roots) = collect_dep_include_dirs_and_roots(dir.path(), &manifest);

        assert!(roots.iter().any(|root| root.ends_with(".pkgs/vecmath")));
        assert!(roots.iter().any(|root| root.ends_with(".pkgs/mathlib")));
        assert!(includes
            .iter()
            .any(|include| include.ends_with(".pkgs/vecmath/include")));
        assert!(includes
            .iter()
            .any(|include| include.ends_with(".pkgs/mathlib/include")));
    }
}
