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
pub mod proto;

pub use compile::{
    compile_sources, compile_sources_unity, dep_file_path, emit_asm_sources, object_path,
    primary_family, select_compiler, settings_for_lang, CompileResult, UNITY_SUPPORTED_LANGS,
};
pub use deps::{check_slot_conflicts, resolve_dep_graph, ResolvedDep};
pub use discover::{discover, DiscoveredSources, SourceFile};
pub use link::{link_static_lib, link_targets, link_test_binary, select_linker, LinkResult};
pub use modules::{
    bmi_path, compile_module_sources, has_modules, plan_module_build, scan_sources,
    ModuleBuildPlan, ModuleRole, ScannedSource,
};

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

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
pub(super) fn has_lang(
    manifest: &Manifest,
    lang_key: &str,
    detected: &[DetectedCompiler],
) -> bool {
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

#[cfg(test)]
mod tests {
    use super::{is_standard_c_family_include_dir, lsp_visible_include_dirs};
    use std::path::PathBuf;

    #[test]
    fn lsp_include_filter_keeps_project_paths() {
        let dir = tempfile::tempdir().unwrap();
        let include = dir.path().join("include");
        std::fs::create_dir_all(&include).unwrap();

        let filtered = lsp_visible_include_dirs(dir.path(), vec![include.clone()]);
        assert_eq!(filtered, vec![include]);
    }

    #[test]
    fn lsp_include_filter_drops_broad_system_paths() {
        let dir = tempfile::tempdir().unwrap();
        let filtered = lsp_visible_include_dirs(
            dir.path(),
            vec![PathBuf::from("/usr/include"), PathBuf::from("/opt/local/include")],
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
    )
}

/// Build the project at a specific `project_dir`.
///
/// `target_override` overrides the target triple from `~/.freight/config.toml`.
/// `sanitize_override` replaces the profile's `sanitize` list when non-empty.
/// All progress events are sent through `progress`.
pub fn build_project_at(
    project_dir: &Path,
    profile: &str,
    features: &[String],
    use_defaults: bool,
    target_override: Option<&str>,
    sanitize_override: &[String],
    progress: &Progress,
) -> Result<BuildOutput, FreightError> {
    let mut ctx = load_project_at(project_dir, profile)?;
    if let Some(t) = target_override {
        ctx.manifest.compiler.target = Some(t.to_string());
    }
    if !sanitize_override.is_empty() {
        apply_sanitize_override(&mut ctx.manifest, profile, sanitize_override);
    }
    inject_option_handler_flags(&mut ctx)?;
    let ProjectContext {
        project_dir,
        manifest,
        effective_backend,
        templates,
        detected,
        found,
    } = &ctx;

    progress(BuildEvent::BuildStarted {
        name: manifest.package.name.clone(),
        profile: profile.to_string(),
    });

    // Merge profile-level features into the caller-supplied list before resolving.
    let profile_features_buf: Vec<String> = match profile {
        "dev" => manifest
            .profile
            .dev
            .as_ref()
            .map_or(vec![], |p| p.features.clone()),
        "release" => manifest
            .profile
            .release
            .as_ref()
            .map_or(vec![], |p| p.features.clone()),
        other => manifest
            .profile
            .custom
            .get(other)
            .map_or(vec![], |p| p.features.clone()),
    };
    let all_requested: Vec<String> = features
        .iter()
        .chain(profile_features_buf.iter())
        .cloned()
        .collect();

    let resolution = features::resolve_features(&manifest.features, &all_requested, use_defaults)?;
    let feature_defines = features::to_defines(&resolution.active);
    let activated_deps = resolution.activated_deps;

    ensure_git_deps_fetched(project_dir, manifest, progress)?;

    // Auto-fetch any registry deps not yet in target/deps/ — same as `freight fetch`
    // but implicit, so `freight add` + `freight build` is the only workflow needed.
    {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(project_dir) {
            cfg.apply_local(local);
        }
        if let Ok(outcomes) = crate::dep_cmds::fetch_registry_deps(project_dir, &cfg) {
            for o in outcomes {
                if matches!(o.action, crate::dep_cmds::RegistryDepAction::Downloaded) {
                    progress(BuildEvent::FetchingDep {
                        name: o.name.clone(),
                        source: format!("registry@{}", o.version),
                    });
                }
            }
        }
    }

    let existing_lock = LockFile::load(project_dir);
    if let Some(ref lock) = existing_lock {
        verify_git_dep_shas(project_dir, manifest, lock, progress);
    }

    let resolved_deps = resolve_dep_graph(project_dir, manifest, false, &activated_deps)?;
    let to_drop = check_slot_conflicts(&resolved_deps, manifest)?;
    let resolved_deps: Vec<ResolvedDep> = resolved_deps
        .into_iter()
        .filter(|d| !to_drop.contains(&d.name))
        .collect();
    let built = build_resolved_deps(
        manifest,
        project_dir,
        effective_backend,
        profile,
        templates,
        detected,
        &resolved_deps,
        progress,
    )?;
    let (foreign_built, _pkg_configs, tool_paths) =
        crate::adaptors::build_foreign_deps(project_dir, manifest, profile, progress)?;

    let mut all_libs = built.libs.clone();
    let mut all_dep_includes = built.include_dirs.clone();
    let mut all_raw_link_flags: Vec<String> = Vec::new();
    for f in foreign_built {
        all_libs.extend(f.libs);
        all_dep_includes.extend(f.include_dirs);
        all_raw_link_flags.extend(f.raw_link_flags);
    }

    // Merge dep include dirs into the set passed to the root compile step.
    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(all_dep_includes.iter().cloned());

    let compile_defines = feature_defines.clone();

    // ── Proto code generation ────────────────────────────────────────────────
    // When [language.proto] is declared, run protoc on .proto files in src/,
    // inject the generated .pb.cc files into the compile list, and add the
    // generated header directory to include_dirs so #include "foo.pb.h" works.
    let all_sources = if manifest.language.contains_key("proto") {
        let proto_settings = manifest.effective_language_settings("proto");
        let result =
            proto::run_protoc(project_dir, profile, &proto_settings, &tool_paths, progress)?;
        if !result.generated_include_dir.as_os_str().is_empty() {
            include_dirs.push(result.generated_include_dir);
        }
        let mut srcs = found.sources.clone();
        srcs.extend(result.generated_sources);
        srcs
    } else {
        found.sources.clone()
    };

    // When the project uses C++20+, precompile dep headers as header units so
    // consumers can write `import "dep.h";` instead of `#include "dep.h"`.
    // Failures are non-fatal — we just skip and compile normally.
    let hu_flags: Vec<String> = if let Some(cpp_std) = manifest
        .language
        .get("cpp")
        .and_then(|l| l.std.as_deref())
        .filter(|s| header_units::is_module_std(s))
    {
        let units = header_units::precompile_dep_headers(
            project_dir,
            &all_dep_includes,
            cpp_std,
            effective_backend,
            detected,
            profile,
        );
        if let Some(compiler) = compile::select_compiler("cpp", effective_backend, detected, None) {
            header_units::import_flags(&units, compiler)
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // If a PCH header is configured, compile it and inject the use flag into
    // every source file. Failures are non-fatal.
    let mut pch_compile_flags: Vec<String> = vec![];
    let mut pch_clangd_flags: Vec<String> = vec![];
    if let Some(ref pch_header) = manifest.compiler.pch.clone() {
        let primary = compile::select_compiler("cpp", effective_backend, detected, None)
            .or_else(|| compile::select_compiler("c", effective_backend, detected, None));
        if let Some(compiler) = primary {
            match pch::compile_pch(
                project_dir,
                pch_header,
                profile,
                compiler,
                &include_dirs,
                &compile_defines,
                &[],
            ) {
                Ok(Some(compiled)) => {
                    pch_compile_flags = compiled
                        .use_flag
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                    pch_clangd_flags = compiled
                        .clangd_flag
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                }
                Ok(None) => {}
                Err(e) => progress(BuildEvent::Warning(format!("PCH skipped: {e}"))),
            }
        }
    }

    let mut extra_flags = hu_flags.clone();
    extra_flags.extend(pch_compile_flags);
    let compile_result = build_sources(
        project_dir,
        manifest,
        effective_backend,
        profile,
        &all_sources,
        &include_dirs,
        detected,
        &compile_defines,
        &extra_flags,
        progress,
    )?;

    let link_result = link_targets(
        project_dir,
        manifest,
        effective_backend,
        profile,
        &compile_result.objects,
        detected,
        templates,
        &all_libs,
        &all_raw_link_flags,
        progress,
    )?;

    // Keep freight.lock in sync with the resolved dep graph. Lock-write failures
    // are non-fatal — we surface them on stderr but still return success.
    let lock = LockFile::generate(project_dir, manifest, &resolved_deps);
    if let Err(e) = lock.save(project_dir) {
        progress(BuildEvent::Warning(format!(
            "could not write freight.lock: {e}"
        )));
    }

    let cc = compile_commands::generate_incremental(
        project_dir,
        manifest,
        effective_backend,
        detected,
        profile,
        &all_sources,
        &include_dirs,
        &feature_defines,
        &pch_clangd_flags,
        Some(&compile_result.compiled_sources),
    );
    if let Err(e) = compile_commands::write(project_dir, &cc).and_then(|_| {
        compile_commands::write_incremental_cache(
            project_dir,
            manifest,
            effective_backend,
            detected,
            profile,
            &all_sources,
            &include_dirs,
            &feature_defines,
            &pch_clangd_flags,
        )
    }) {
        progress(BuildEvent::Warning(format!(
            "could not write compile_commands.json: {e}"
        )));
    }

    let binaries = link_result
        .outputs
        .iter()
        .filter(|p| {
            !p.extension()
                .is_some_and(|e| e == "a" || e == "so" || e == "dylib" || e == "dll")
        })
        .cloned()
        .collect();

    Ok(BuildOutput {
        package_name: manifest.package.name.clone(),
        binaries,
        compiled: compile_result.compiled,
        skipped: compile_result.skipped,
    })
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

    let commands = compile_commands::generate_incremental(
        project_dir,
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
    let count = commands.len();
    compile_commands::write(project_dir, &commands).and_then(|_| {
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
    })?;
    Ok(count)
}

/// Generate the compile database used internally by `freight lsp`.
///
/// Unlike [`generate_compile_commands_at`], this writes to a backend cache
/// directory outside the project tree
/// so editor integrations can point source language servers at Freight's
/// manifest-scoped view without adding `compile_commands.json` to the project
/// root or the explorer.
pub fn generate_lsp_compile_commands_at(
    project_dir: &Path,
    profile: &str,
) -> Result<PathBuf, FreightError> {
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

    let dep_includes = collect_dep_include_dirs(project_dir, manifest);
    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(dep_includes);
    let include_dirs = lsp_visible_include_dirs(project_dir, include_dirs);

    let commands = compile_commands::generate(
        project_dir,
        manifest,
        effective_backend,
        detected,
        profile,
        &found.sources,
        &include_dirs,
        &feature_defines,
        &[],
    );
    let dir = lsp_compile_commands_dir(project_dir, profile);
    compile_commands::write_to(&dir.join("compile_commands.json"), &commands)?;
    Ok(dir)
}

fn lsp_compile_commands_dir(project_dir: &Path, profile: &str) -> PathBuf {
    let stable_project_dir = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    stable_project_dir.hash(&mut hasher);
    profile.hash(&mut hasher);
    std::env::temp_dir()
        .join("freight")
        .join("lsp")
        .join(format!("{:016x}", hasher.finish()))
}

fn lsp_visible_include_dirs(project_dir: &Path, include_dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let project_root = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    include_dirs
        .into_iter()
        .filter(|dir| {
            if dir.is_relative() {
                return true;
            }
            let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            canonical.starts_with(&project_root) || is_standard_c_family_include_dir(&canonical)
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
    let empty = BTreeSet::new();
    let Ok(resolved) = resolve_dep_graph(project_dir, manifest, false, &empty) else {
        return vec![];
    };
    resolved
        .iter()
        .flat_map(|dep| deps::dep_include_dirs(&dep.dir, &dep.manifest))
        .collect()
}

/// Emit assembly files for all sources of the project at the current working directory.
///
/// Writes `.s` files to `target/{profile}/asm/` preserving the source tree structure.
/// Skips pure assembler sources (gas/nasm/yasm) and languages without `-S` support.
pub fn emit_asm_project_with(profile: &str, progress: &Progress) -> Result<(), FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ctx = load_project_at(&project_dir, profile)?;
    let ProjectContext {
        project_dir,
        manifest,
        effective_backend,
        detected,
        found,
        ..
    } = &ctx;

    let resolution = features::resolve_features(&manifest.features, &[], true)?;
    let feature_defines = features::to_defines(&resolution.active);
    let include_dirs = found.include_dirs.clone();

    compile::emit_asm_sources(
        project_dir,
        manifest,
        effective_backend,
        profile,
        &found.sources,
        &include_dirs,
        detected,
        &feature_defines,
        progress,
    )
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
    let mut ctx = load_project_at(project_dir, profile)?;
    if !sanitize_override.is_empty() {
        apply_sanitize_override(&mut ctx.manifest, profile, sanitize_override);
    }
    inject_option_handler_flags(&mut ctx)?;
    let ProjectContext {
        project_dir,
        manifest,
        effective_backend,
        templates,
        detected,
        found,
    } = &ctx;

    progress(BuildEvent::BuildStarted {
        name: manifest.package.name.clone(),
        profile: profile.to_string(),
    });

    let profile_features_buf: Vec<String> = match profile {
        "dev" => manifest
            .profile
            .dev
            .as_ref()
            .map_or(vec![], |p| p.features.clone()),
        "release" => manifest
            .profile
            .release
            .as_ref()
            .map_or(vec![], |p| p.features.clone()),
        other => manifest
            .profile
            .custom
            .get(other)
            .map_or(vec![], |p| p.features.clone()),
    };
    let all_requested: Vec<String> = features
        .iter()
        .chain(profile_features_buf.iter())
        .cloned()
        .collect();

    let resolution = features::resolve_features(&manifest.features, &all_requested, use_defaults)?;
    let feature_defines = features::to_defines(&resolution.active);
    let activated_deps = resolution.activated_deps;

    ensure_git_deps_fetched(project_dir, manifest, progress)?;
    let existing_lock = LockFile::load(project_dir);
    if let Some(ref lock) = existing_lock {
        verify_git_dep_shas(project_dir, manifest, lock, progress);
    }

    // Build deps (include dev-dependencies for test runs).
    let resolved_deps = resolve_dep_graph(project_dir, manifest, true, &activated_deps)?;
    let to_drop = check_slot_conflicts(&resolved_deps, manifest)?;
    let resolved_deps: Vec<ResolvedDep> = resolved_deps
        .into_iter()
        .filter(|d| !to_drop.contains(&d.name))
        .collect();
    let built = build_resolved_deps(
        manifest,
        project_dir,
        effective_backend,
        profile,
        templates,
        detected,
        &resolved_deps,
        progress,
    )?;
    let (foreign_built, _pkg_configs, tool_paths) =
        crate::adaptors::build_foreign_deps(project_dir, manifest, profile, progress)?;

    let mut all_libs = built.libs.clone();
    let mut all_dep_includes = built.include_dirs.clone();
    let mut all_raw_link_flags: Vec<String> = Vec::new();
    for f in foreign_built {
        all_libs.extend(f.libs);
        all_dep_includes.extend(f.include_dirs);
        all_raw_link_flags.extend(f.raw_link_flags);
    }

    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(all_dep_includes.iter().cloned());

    let compile_defines = feature_defines.clone();

    // Proto codegen — same step as in build_project_at.
    let all_sources = if manifest.language.contains_key("proto") {
        let proto_settings = manifest.effective_language_settings("proto");
        let result =
            proto::run_protoc(project_dir, profile, &proto_settings, &tool_paths, progress)?;
        if !result.generated_include_dir.as_os_str().is_empty() {
            include_dirs.push(result.generated_include_dir);
        }
        let mut srcs = found.sources.clone();
        srcs.extend(result.generated_sources);
        srcs
    } else {
        found.sources.clone()
    };

    // PCH injection for test builds (same logic as build_project_at).
    let pch_extra_test: Vec<String> = if let Some(ref pch_header) = manifest.compiler.pch.clone() {
        let primary = compile::select_compiler("cpp", effective_backend, detected, None)
            .or_else(|| compile::select_compiler("c", effective_backend, detected, None));
        if let Some(compiler) = primary {
            match pch::compile_pch(
                project_dir,
                pch_header,
                profile,
                compiler,
                &include_dirs,
                &compile_defines,
                &[],
            ) {
                Ok(Some(compiled)) => compiled
                    .use_flag
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                Ok(None) => vec![],
                Err(e) => {
                    progress(BuildEvent::Warning(format!("PCH skipped: {e}")));
                    vec![]
                }
            }
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let compile_result = build_sources(
        project_dir,
        manifest,
        effective_backend,
        profile,
        &all_sources,
        &include_dirs,
        detected,
        &compile_defines,
        &pch_extra_test,
        progress,
    )?;

    // Objects from [[bin]] sources contain a main() — exclude from test linking.
    let bin_obj_paths: std::collections::HashSet<PathBuf> = manifest
        .bins
        .iter()
        .map(|b| object_path(project_dir, profile, Path::new(&b.src)))
        .collect();
    let lib_objects: Vec<PathBuf> = compile_result
        .objects
        .iter()
        .filter(|o| !bin_obj_paths.contains(*o))
        .cloned()
        .collect();

    let test_dir = project_dir.join("tests");
    if !test_dir.is_dir() {
        return Ok(TestSummary {
            passed: 0,
            failed: 0,
            total: 0,
        });
    }

    let ext_map = discover::build_ext_map(manifest, templates);
    let mut test_srcs: Vec<SourceFile> = Vec::new();

    for entry in WalkDir::new(&test_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => format!(".{e}"),
            None => continue,
        };
        if let Some(lang_key) = ext_map.get(ext.as_str()) {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("test");
            if filter.map_or(true, |f| f == stem) {
                let rel = path.strip_prefix(project_dir).unwrap_or(path).to_path_buf();
                test_srcs.push(SourceFile {
                    path: rel,
                    lang_key: lang_key.clone(),
                });
            }
        }
    }
    test_srcs.sort_by(|a, b| a.path.cmp(&b.path));

    if test_srcs.is_empty() {
        return Ok(TestSummary {
            passed: 0,
            failed: 0,
            total: 0,
        });
    }

    let test_compile = compile_sources(
        project_dir,
        manifest,
        effective_backend,
        profile,
        &test_srcs,
        &include_dirs,
        detected,
        &feature_defines,
        &[],
        progress,
    )?;

    let out_dir = project_dir.join("target").join(profile).join("tests");
    std::fs::create_dir_all(&out_dir)?;

    let mut passed = 0usize;
    let mut failed = 0usize;

    for (src, test_obj) in test_srcs.iter().zip(test_compile.objects.iter()) {
        let stem = src
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("test");
        let test_bin = out_dir.join(stem);

        progress(BuildEvent::TestLinking {
            name: stem.to_string(),
        });

        let all_objs: Vec<PathBuf> = std::iter::once(test_obj.clone())
            .chain(lib_objects.iter().cloned())
            .collect();
        link_test_binary(
            &all_objs,
            &test_bin,
            manifest,
            effective_backend,
            profile,
            detected,
            templates,
            &all_libs,
            &all_raw_link_flags,
        )?;

        progress(BuildEvent::TestRunning {
            name: stem.to_string(),
        });
        let ok = Command::new(&test_bin)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        progress(BuildEvent::TestResult {
            name: stem.to_string(),
            passed: ok,
        });
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
    }

    Ok(TestSummary {
        passed,
        failed,
        total: passed + failed,
    })
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
///
/// Benchmarks are discovered in the `benches/` directory — any compilable
/// source file there is treated as a standalone bench binary with its own
/// `main()`. Each binary is run [`BENCH_RUNS`] times and the wall-clock min,
/// max, and mean are reported. Pass a filter to run only benches whose file
/// stem matches exactly.
pub fn bench_project_at(
    project_dir: &Path,
    profile: &str,
    filter: Option<&str>,
    features: &[String],
    use_defaults: bool,
    progress: &Progress,
) -> Result<BenchSummary, FreightError> {
    let mut ctx = load_project_at(project_dir, profile)?;
    inject_option_handler_flags(&mut ctx)?;
    let ProjectContext {
        project_dir,
        manifest,
        effective_backend,
        templates,
        detected,
        found,
    } = &ctx;
    let project_dir = project_dir.as_path();

    let profile_features_buf: Vec<String> = manifest
        .build_settings_for(profile)
        .sanitize
        .iter()
        .map(|s| format!("sanitize_{s}"))
        .collect();
    let all_requested: Vec<String> = features
        .iter()
        .chain(profile_features_buf.iter())
        .cloned()
        .collect();

    let resolution = features::resolve_features(&manifest.features, &all_requested, use_defaults)?;
    let feature_defines = features::to_defines(&resolution.active);
    let activated_deps = resolution.activated_deps;

    ensure_git_deps_fetched(project_dir, manifest, progress)?;

    let resolved_deps = resolve_dep_graph(project_dir, manifest, false, &activated_deps)?;
    let to_drop = check_slot_conflicts(&resolved_deps, manifest)?;
    let resolved_deps: Vec<ResolvedDep> = resolved_deps
        .into_iter()
        .filter(|d| !to_drop.contains(&d.name))
        .collect();
    let built = build_resolved_deps(
        manifest,
        project_dir,
        effective_backend,
        profile,
        templates,
        detected,
        &resolved_deps,
        progress,
    )?;
    let (foreign_built, _pkg_configs, tool_paths) =
        crate::adaptors::build_foreign_deps(project_dir, manifest, profile, progress)?;

    let mut all_libs = built.libs.clone();
    let mut all_dep_includes = built.include_dirs.clone();
    let mut all_raw_link_flags: Vec<String> = Vec::new();
    for f in foreign_built {
        all_libs.extend(f.libs);
        all_dep_includes.extend(f.include_dirs);
        all_raw_link_flags.extend(f.raw_link_flags);
    }

    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(all_dep_includes.iter().cloned());

    // Proto codegen — same step as in build_project_at.
    let bench_sources = if manifest.language.contains_key("proto") {
        let proto_settings = manifest.effective_language_settings("proto");
        let result =
            proto::run_protoc(project_dir, profile, &proto_settings, &tool_paths, progress)?;
        if !result.generated_include_dir.as_os_str().is_empty() {
            include_dirs.push(result.generated_include_dir);
        }
        let mut srcs = found.sources.clone();
        srcs.extend(result.generated_sources);
        srcs
    } else {
        found.sources.clone()
    };

    let compile_result = build_sources(
        project_dir,
        manifest,
        effective_backend,
        profile,
        &bench_sources,
        &include_dirs,
        detected,
        &feature_defines,
        &[],
        progress,
    )?;

    // Exclude [[bin]] entry-point objects (contain a main()) from bench linking.
    let bin_obj_paths: std::collections::HashSet<PathBuf> = manifest
        .bins
        .iter()
        .map(|b| object_path(project_dir, profile, Path::new(&b.src)))
        .collect();
    let lib_objects: Vec<PathBuf> = compile_result
        .objects
        .iter()
        .filter(|o| !bin_obj_paths.contains(*o))
        .cloned()
        .collect();

    let bench_dir = project_dir.join("benches");
    if !bench_dir.is_dir() {
        return Ok(BenchSummary { results: vec![] });
    }

    let ext_map = discover::build_ext_map(manifest, templates);
    let mut bench_srcs: Vec<SourceFile> = Vec::new();

    for entry in WalkDir::new(&bench_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => format!(".{e}"),
            None => continue,
        };
        if let Some(lang_key) = ext_map.get(ext.as_str()) {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("bench");
            if filter.map_or(true, |f| f == stem) {
                let rel = path.strip_prefix(project_dir).unwrap_or(path).to_path_buf();
                bench_srcs.push(SourceFile {
                    path: rel,
                    lang_key: lang_key.clone(),
                });
            }
        }
    }
    bench_srcs.sort_by(|a, b| a.path.cmp(&b.path));

    if bench_srcs.is_empty() {
        return Ok(BenchSummary { results: vec![] });
    }

    let bench_compile = compile_sources(
        project_dir,
        manifest,
        effective_backend,
        profile,
        &bench_srcs,
        &include_dirs,
        detected,
        &feature_defines,
        &[],
        progress,
    )?;

    let out_dir = project_dir.join("target").join(profile).join("benches");
    std::fs::create_dir_all(&out_dir)?;

    const BENCH_RUNS: usize = 5;
    let mut results = Vec::new();

    for (src, bench_obj) in bench_srcs.iter().zip(bench_compile.objects.iter()) {
        let stem = src
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("bench");
        let bench_bin = out_dir.join(stem);

        progress(BuildEvent::BenchLinking {
            name: stem.to_string(),
        });

        let all_objs: Vec<PathBuf> = std::iter::once(bench_obj.clone())
            .chain(lib_objects.iter().cloned())
            .collect();
        link_test_binary(
            &all_objs,
            &bench_bin,
            manifest,
            effective_backend,
            profile,
            detected,
            templates,
            &all_libs,
            &all_raw_link_flags,
        )?;

        progress(BuildEvent::BenchRunning {
            name: stem.to_string(),
        });

        let mut samples_ns: Vec<u64> = Vec::with_capacity(BENCH_RUNS);
        for _ in 0..BENCH_RUNS {
            let t0 = std::time::Instant::now();
            let ok = Command::new(&bench_bin)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            let elapsed = t0.elapsed().as_nanos() as u64;
            if ok {
                samples_ns.push(elapsed);
            }
        }

        let runs = samples_ns.len();
        let (mean_ns, min_ns, max_ns) = if runs == 0 {
            (0, 0, 0)
        } else {
            let sum: u64 = samples_ns.iter().sum();
            let min = *samples_ns.iter().min().unwrap();
            let max = *samples_ns.iter().max().unwrap();
            (sum / runs as u64, min, max)
        };

        progress(BuildEvent::BenchResult {
            name: stem.to_string(),
            mean_ns,
        });
        results.push(BenchResult {
            name: stem.to_string(),
            mean_ns,
            min_ns,
            max_ns,
            runs,
        });
    }

    Ok(BenchSummary { results })
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
        let Some(url) = &d.git else { continue };

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
        let Some(_url) = &d.git else { continue };

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
        // C++20 modules have their own dependency ordering — unity doesn't apply.
        let mut plan = plan_module_build(project_dir, profile, scanned)?;
        compile_module_sources(
            project_dir,
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

        let compile_result = if effective_unity {
            compile::compile_sources_unity(
                &dep.dir,
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

        let lib_out = dep
            .dir
            .join("target")
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
