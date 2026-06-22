pub mod compile;
pub mod compile_commands;
pub mod deps;
pub(crate) mod diagnostics;
pub mod discover;
pub mod features;
pub mod header_ownership;
pub mod header_units;
pub mod include_policy;
pub mod link;
pub mod modules;
pub mod pch;
pub mod pipeline;
pub mod plugin;
pub mod std_module;

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
// `Project` is the central project model — it now lives at `crate::project`.
// Re-exported here so existing `crate::build::{Project, …}` paths keep working.
pub use crate::project::{source_package_dirs, PackageKind, Project};

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
    CompilerTemplate, DetectedCompiler,
};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

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
            .is_some_and(|l| l.srcs.iter().any(|s| has(s)))
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
    Examples(BuildOutput),
}

pub(crate) struct ProjectContext {
    project_dir: PathBuf,
    manifest: Manifest,
    // Read by `crate::project::Project` (e.g. for `emit_sources`); crate-visible.
    pub(crate) effective_backend: Backend,
    templates: Vec<CompilerTemplate>,
    pub(crate) detected: Vec<DetectedCompiler>,
    pub(crate) found: DiscoveredSources,
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
    let mut project = Project::open(project_dir)?;
    if let Some(root) = parent_root {
        project = project.with_parent_root(root.to_path_buf());
    }
    project.build(&config, progress)
}

/// Build example programs (from `examples/` and `[[example]]`). `filter` selects
/// a single example by name; `None` builds them all. Returns the linked
/// executables in `target/<profile>/examples/`.
pub fn build_examples_with(
    profile: &str,
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
    sanitize_override: &[String],
    progress: &Progress,
) -> Result<BuildOutput, FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let config = PipelineConfig {
        profile: profile.to_string(),
        features: features.to_vec(),
        use_defaults,
        target_override: None,
        sanitize_override: sanitize_override.to_vec(),
        goal: PipelineGoal::Examples {
            filter: filter.map(str::to_string),
        },
    };
    match run_pipeline_at(&project_dir, &config, None, progress)? {
        PipelineOutput::Examples(out) => Ok(out),
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
    include_dirs.extend(plugin::plugin_include_dirs(project_dir, profile));

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
        // cmd.file is relative to cmd.directory (e.g. "src/main.cpp").
        // ClangIndexer looks up by absolute URI path, so canonicalize the key.
        let abs_file = if cmd.file.is_absolute() {
            cmd.file
        } else {
            cmd.directory.join(&cmd.file)
        };
        map.insert(abs_file, (compiler, dir, flags));
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
    // Generated headers from active plugins (e.g. `foo.pb.h`) live under
    // target/<profile>/plugin-gen/<section>; expose them so clangd resolves them
    // and the undeclared-include check (which reads these dirs back from
    // compile_commands.json) treats them as project-owned.
    include_dirs.extend(plugin::plugin_include_dirs(project_dir, profile));
    let include_dirs = lsp_visible_include_dirs(project_dir, &dep_roots, include_dirs);

    let mut commands = compile_commands::generate(
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
    );
    inject_std_module_flags(project_dir, profile, &found.sources, &mut commands);
    Ok(commands)
}

/// When sources `import std;` / `import std.compat;`, build the toolchain's
/// standard-library module BMI and append `-fmodule-file=std=<bmi>` to every C++
/// compile command so clangd (and a future build path) can resolve the import.
fn inject_std_module_flags(
    project_dir: &Path,
    profile: &str,
    sources: &[discover::SourceFile],
    commands: &mut [compile_commands::CompileCommand],
) {
    let scanned = modules::scan_sources(project_dir, sources);
    let mut wanted: Vec<&str> = Vec::new();
    for s in &scanned {
        for imp in &s.imports {
            match imp.as_str() {
                "std" if !wanted.contains(&"std") => wanted.push("std"),
                "std.compat" if !wanted.contains(&"std.compat") => wanted.push("std.compat"),
                _ => {}
            }
        }
    }
    if wanted.is_empty() {
        return;
    }

    // Compiler + `-std=` from an existing C++ compile command.
    let Some(cmd) = commands
        .iter()
        .find(|c| c.arguments.iter().any(|a| a.starts_with("-std=c++")))
    else {
        return;
    };
    let compiler = PathBuf::from(&cmd.arguments[0]);
    let std_flag = cmd
        .arguments
        .iter()
        .find_map(|a| a.strip_prefix("-std="))
        .unwrap_or("c++23")
        .to_string();

    let cache = lsp_compile_commands_dir(project_dir, profile).join("std-modules");
    let flags = std_module::module_file_flags(&compiler, &std_flag, &cache, &wanted);
    if flags.is_empty() {
        return;
    }
    for c in commands.iter_mut() {
        if c.arguments.iter().any(|a| a.starts_with("-std=c++")) {
            c.arguments.extend(flags.iter().cloned());
        }
    }
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
        "debug".to_string()
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
    Project::open(project_dir)?.test(&config, filter, progress)
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
    Project::open(project_dir)?.bench(&config, filter, progress)
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
        // A `[patch]` override replaces this dep with a local source — nothing to clone.
        if manifest.patch.contains_key(name) {
            continue;
        }
        let Dependency::Detailed(d) = dep else {
            continue;
        };
        let Some(url) = &d.url else { continue };
        // Only git deps are cloned here; url-archive deps (`.tar.gz`, …) are
        // fetched on demand at build time, not git-cloned.
        if !d.is_git() {
            continue;
        }

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
/// Language keys whose sources use the C preprocessor and are subject to
/// include hygiene.
fn is_c_family_lang(lang_key: &str) -> bool {
    matches!(lang_key, "c" | "cpp" | "cuda" | "hip" | "objc" | "objcpp")
}

/// Include-hygiene Phase 2 — the build-time enforcement pass.
///
/// Re-runs the Phase-1 classification ([`include_policy::check_includes`]) over
/// every C-family source's `#include`/`import` directives. Headers that resolve
/// to no declared dependency (and aren't standard-library headers) are reported
/// per `[lints].undeclared-include`: `warn` emits build warnings, `deny` fails
/// the build, `allow` skips the pass entirely.
#[allow(clippy::too_many_arguments)]
fn validate_include_hygiene(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    progress: &Progress,
) -> Result<(), FreightError> {
    use crate::build::include_policy as ip;
    use crate::manifest::LintLevel;

    let level = manifest.lints.undeclared_include;
    if level == LintLevel::Allow {
        return Ok(());
    }

    // Names a project may declare to legitimise a system header. The dependency
    // key is the freight package name — what pkg-config is queried by and what
    // the ownership map is keyed on.
    let declared_names: Vec<String> = {
        let mut v: Vec<String> = manifest.effective_dependencies().into_keys().collect();
        v.sort();
        v.dedup();
        v
    };

    // The declared include dirs are the allowlist; make them absolute so
    // resolution is independent of the process working directory.
    let mut declared: Vec<PathBuf> = include_dirs
        .iter()
        .map(|d| {
            if d.is_absolute() {
                d.clone()
            } else {
                project_dir.join(d)
            }
        })
        .collect();

    // Phase 3 / Tier B: a declared dep's pkg-config dedicated include dirs (a
    // `/usr/include/SDL2`-style subdir, never bare `/usr/include`) are part of
    // the allowlist — version-correct and safe from over-allowing.
    for name in &declared_names {
        declared.extend(header_ownership::pkg_config_dedicated_dirs(name));
    }
    declared.sort();
    declared.dedup();

    // Phase 3 / Tier A: ownership of bare-`/usr/include` headers by declared
    // packages/slots (e.g. a declared BLAS provider owns `cblas.h`).
    let ownership = header_ownership::load();
    let owned_globs = ownership.owned_globs_for(&declared_names);

    // The compiler's built-in system dirs only confirm that an undeclared header
    // actually exists (so "undeclared but present" is distinguished from a
    // missing header, which the compiler reports itself). Probed once per
    // (compiler, language) and cached.
    let mut sys_cache: std::collections::HashMap<(PathBuf, bool), Vec<PathBuf>> =
        std::collections::HashMap::new();
    // Cross build: resolve system headers against the target sysroot, not the host.
    let sysroot: Option<PathBuf> = manifest.compiler.sysroot.as_ref().map(PathBuf::from);

    let mut findings: Vec<(PathBuf, ip::UndeclaredInclude)> = Vec::new();
    for src in sources {
        if !is_c_family_lang(&src.lang_key) {
            continue;
        }
        let abs = project_dir.join(&src.path);
        let Ok(text) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let file_dir = abs.parent().map(Path::to_path_buf).unwrap_or_default();
        let lang = ip::Language::from_path(&abs);
        let is_cxx = lang == ip::Language::Cxx;

        let system = match compile::select_compiler(&src.lang_key, backend, detected, None) {
            Some(cc) => sys_cache
                .entry((cc.path.clone(), is_cxx))
                .or_insert_with(|| ip::system_include_dirs(&cc.path, lang, sysroot.as_deref()))
                .clone(),
            None => Vec::new(),
        };

        for f in ip::check_includes(&text, &file_dir, &declared, &system, lang) {
            // Tier A: suppress a header a declared package/slot owns.
            let name = header_name(&f.spelling);
            if owned_globs
                .iter()
                .any(|g| header_ownership::glob_match(g, name))
            {
                continue;
            }
            findings.push((src.path.clone(), f));
        }
    }

    if findings.is_empty() {
        return Ok(());
    }

    // Turn a finding into a message — naming the candidate package(s) when the
    // header is one the ownership map knows about ("provided by openblas, mkl").
    let describe = |f: &ip::UndeclaredInclude| -> String {
        let name = header_name(&f.spelling);
        let candidates: Vec<String> = ownership
            .candidates_for_header(name)
            .into_iter()
            .filter(|c| !declared_names.contains(c))
            .collect();
        if candidates.is_empty() {
            format!(
                "{} is not provided by any declared dependency; add the dependency that \
                 provides it to [dependencies] in freight.toml",
                f.spelling
            )
        } else {
            format!(
                "{} is provided by {} — add one to [dependencies] in freight.toml",
                f.spelling,
                candidates.join(", ")
            )
        }
    };

    match level {
        LintLevel::Deny => {
            let lines: Vec<String> = findings
                .iter()
                .map(|(path, f)| format!("  {}:{}: {}", path.display(), f.line + 1, describe(f)))
                .collect();
            Err(FreightError::UndeclaredInclude(lines.join("\n")))
        }
        LintLevel::Warn => {
            for (path, f) in &findings {
                progress(BuildEvent::Warning(format!(
                    "{}:{}: {}",
                    path.display(),
                    f.line + 1,
                    describe(f)
                )));
            }
            Ok(())
        }
        LintLevel::Allow => Ok(()),
    }
}

/// Strip the `<…>` / `"…"` delimiters from an include spelling.
fn header_name(spelling: &str) -> &str {
    spelling.trim_matches(|c| matches!(c, '<' | '>' | '"'))
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
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
    tool_flags: &[crate::build::plugin::ToolFlag],
    progress: &Progress,
) -> Result<CompileResult, FreightError> {
    // Include-hygiene Phase 2: reject (or warn about) `#include`/`import` of
    // headers no declared dependency provides, before invoking the compiler.
    validate_include_hygiene(
        project_dir,
        manifest,
        backend,
        sources,
        include_dirs,
        detected,
        progress,
    )?;

    let scanned = scan_sources(project_dir, sources);

    // C++23 `import std;` — build the standard-library module BMI and add
    // `-fmodule-file=std=<bmi>` to every compile in this project. A plain
    // `import std;` TU does not declare a module, so it would otherwise take the
    // non-module path and fail. Merged into the per-build extra-flags slot.
    let mut extra_flags: Vec<String> = header_unit_flags.to_vec();
    extra_flags.extend(std_module_build_flags(
        &scanned, manifest, backend, detected, target_dir, profile,
    ));
    let extra_flags = extra_flags.as_slice();

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
            extra_flags,
            tool_flags,
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
            extra_flags,
            tool_flags,
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
            extra_flags,
            tool_flags,
            progress,
        )
    }
}

/// Build the standard-library module BMI(s) requested by `scanned` (via
/// `import std;` / `import std.compat;`) and return the `-fmodule-file=` flags.
/// Empty when nothing imports std or the toolchain has no std module.
fn std_module_build_flags(
    scanned: &[ScannedSource],
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    target_dir: &Path,
    profile: &str,
) -> Vec<String> {
    let mut wanted: Vec<&str> = Vec::new();
    for s in scanned {
        for imp in &s.imports {
            match imp.as_str() {
                "std" if !wanted.contains(&"std") => wanted.push("std"),
                "std.compat" if !wanted.contains(&"std.compat") => wanted.push("std.compat"),
                _ => {}
            }
        }
    }
    if wanted.is_empty() {
        return Vec::new();
    }
    let pf = compile::primary_family(backend, detected);
    let Some(compiler) = compile::select_compiler("cpp", backend, detected, pf) else {
        return Vec::new();
    };
    let std_flag = manifest
        .language
        .get("cpp")
        .and_then(|l| l.std.clone())
        .unwrap_or_else(|| "c++23".to_string());
    let cache = std_module_cache_dir(target_dir, profile);
    std_module::module_file_flags(&compiler.path, &std_flag, &cache, &wanted)
}

fn std_module_cache_dir(target_dir: &Path, profile: &str) -> PathBuf {
    target_dir.join(profile).join("std-modules")
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
    dep_defines: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
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
        // A plugin-only dependency (declares `[plugin]` but no library/binary to
        // build) contributes solely through the plugin codegen stage — there is
        // nothing to compile or link here, and it has no `src/` to discover.
        if dep.manifest.plugin.is_some()
            && dep.manifest.lib.is_none()
            && dep.manifest.bins.is_empty()
            && dep.manifest.package.build.is_none()
        {
            continue;
        }

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
            let mut defs = features::to_defines(&resolution.active);
            // Explicit defines forwarded from the root via `<dep>/define:NAME`.
            if let Some(fwd) = dep_defines.get(&dep.name) {
                defs.extend(fwd.iter().cloned());
            }
            defs
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

        let dep_target_dir = dep.dir.join("target");
        let compile_result = if dep.manifest.compiler.unity {
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
        "debug" => {
            manifest
                .profile
                .debug
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
                .or_default()
                .sanitize = list;
        }
    }
}

// ── Shared project loading ────────────────────────────────────────────────────

pub(crate) fn load_project_at(
    project_dir: &Path,
    _profile: &str,
) -> Result<ProjectContext, FreightError> {
    let mut manifest = load_manifest(project_dir)?;
    // Resolve the host/target environment once (merged config + FREIGHT_SYSROOT).
    let env = crate::environment::Environment::for_project(project_dir);

    // Project manifest backend wins over the environment default; "auto" defers.
    let configured_backend = if !manifest.compiler.backend.is_auto() {
        manifest.compiler.backend.clone()
    } else {
        env.default_backend.clone().map(Backend).unwrap_or_default()
    };
    env.apply_to_manifest(&mut manifest);

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
    // A foreign package (`[package].build` set, e.g. a vcpkg-scraper port) has no
    // local sources — it's fetched and built with its own build system. For all
    // other projects, fail fast if src/ is empty.
    let foreign = manifest.package.build.is_some();
    if found.sources.is_empty() && !foreign {
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
        collect_dep_include_dirs_and_roots, ensure_git_deps_fetched,
        is_standard_c_family_include_dir, lsp_compile_commands_dir, lsp_visible_include_dirs,
        safe_lsp_profile_dir, silent,
    };
    use crate::manifest::load_manifest;
    use std::path::PathBuf;

    #[test]
    fn url_archive_dep_is_not_git_cloned() {
        // Regression: the build fetch stage used to git-clone every dep with a
        // `url`, so a `.tar.gz` archive dep 404'd. It must be skipped here
        // (url-archive deps are fetched on demand at build time instead).
        let dir = tempfile::tempdir().unwrap();
        let manifest = crate::manifest::load_manifest_str(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.c]\n\
             [[bin]]\nname=\"p\"\nsrc=\"src/main.c\"\n\
             [dependencies]\nfoo = { url = \"https://example.com/foo-1.0.tar.gz\", type = \"cmake\" }\n",
        )
        .unwrap();
        // Must return Ok without any network git clone, and create no .pkgs/foo.
        ensure_git_deps_fetched(dir.path(), &manifest, &silent()).unwrap();
        assert!(
            !dir.path().join(".pkgs/foo").exists(),
            "url-archive dep must not be git-cloned"
        );
    }

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
        assert_eq!(safe_lsp_profile_dir(""), "debug");
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
