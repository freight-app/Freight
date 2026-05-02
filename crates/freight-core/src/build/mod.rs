pub mod compile;
pub mod compile_commands;
pub mod deps;
pub mod discover;
pub mod features;
pub mod foreign;
pub mod header_units;
pub mod http;
pub mod link;
pub mod modules;
pub mod pch;
pub mod script;

pub use compile::{CompileResult, compile_sources, dep_file_path, object_path, primary_family, select_compiler, settings_for_lang};
pub use deps::{ResolvedDep, check_slot_conflicts, resolve_dep_graph};
pub use discover::{DiscoveredSources, SourceFile, discover};
pub use link::{LinkResult, link_static_lib, link_targets, link_test_binary, select_linker};
pub use modules::{ModuleBuildPlan, ModuleRole, ScannedSource, bmi_path, compile_module_sources, has_modules, plan_module_build, scan_sources};

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

use crate::error::FreightError;
use crate::git;
use crate::lock::LockFile;
use crate::manifest::types::{Dependency, Manifest};
use crate::manifest::validate::{validate, validate_dep_compat};
use crate::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};
use crate::toolchain::{CompilerTemplate, DetectedCompiler, detect_all_cached, load_templates, templates_dir};

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

struct ProjectContext {
    project_dir: PathBuf,
    manifest: Manifest,
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
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| FreightError::ManifestParse("not a workspace root".into()))?;

    let mut outputs = Vec::new();
    let mut member_dirs: Vec<PathBuf> = Vec::new();
    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        outputs.push(build_project_at(&member_dir, profile, &[], true, None, &[])?);
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
        eprintln!("warning: could not write workspace compile_commands.json: {e}");
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
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| FreightError::ManifestParse("not a workspace root".into()))?;

    let mut total_passed = 0;
    let mut total_failed = 0;
    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        let s = test_project_at(&member_dir, profile, filter, &[], true, &[])?;
        total_passed += s.passed;
        total_failed += s.failed;
    }
    Ok(TestSummary { passed: total_passed, failed: total_failed, total: total_passed + total_failed })
}

/// Build the project rooted at the current working directory.
///
/// Returns the high-level outcome (binary paths, compile counts) so the
/// caller decides how to present results. Progress-line output (`Building`,
/// `Compiling foo.cpp`, `Linking ...`) currently goes to stdout directly;
/// routing it through a callback is future work.
pub fn build_project(profile: &str, features: &[String], use_defaults: bool, sanitize_override: &[String]) -> Result<BuildOutput, FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    build_project_at(&project_dir, profile, features, use_defaults, None, sanitize_override)
}

/// Build the project at a specific `project_dir`.
///
/// `target_override` replaces `[compiler] target` from the manifest — useful
/// for `freight install --target <triple>` and `freight package --target <triple>`.
/// `sanitize_override` replaces the profile's `sanitize` list when non-empty.
pub fn build_project_at(project_dir: &Path, profile: &str, features: &[String], use_defaults: bool, target_override: Option<&str>, sanitize_override: &[String]) -> Result<BuildOutput, FreightError> {
    let mut ctx = load_project_at(project_dir, profile)?;
    if let Some(t) = target_override {
        ctx.manifest.compiler.target = Some(t.to_string());
    }
    if !sanitize_override.is_empty() {
        apply_sanitize_override(&mut ctx.manifest, profile, sanitize_override);
    }
    let ProjectContext { project_dir, manifest, templates, detected, found } = &ctx;

    use owo_colors::OwoColorize;
    println!("  {} {} ({profile})", "Building".bold(), manifest.package.name);

    // Merge profile-level features into the caller-supplied list before resolving.
    let profile_features_buf: Vec<String> = match profile {
        "dev"     => manifest.profile.dev.as_ref().map_or(vec![], |p| p.features.clone()),
        "release" => manifest.profile.release.as_ref().map_or(vec![], |p| p.features.clone()),
        other     => manifest.profile.custom.get(other).map_or(vec![], |p| p.features.clone()),
    };
    let all_requested: Vec<String> = features.iter().chain(profile_features_buf.iter()).cloned().collect();

    let resolution = features::resolve_features(&manifest.features, &all_requested, use_defaults)?;
    let feature_defines = features::to_defines(&resolution.active);
    let activated_deps = resolution.activated_deps;

    ensure_git_deps_fetched(project_dir, manifest)?;
    let existing_lock = LockFile::load(project_dir);
    if let Some(ref lock) = existing_lock {
        verify_git_dep_shas(project_dir, manifest, lock);
    }

    let resolved_deps = resolve_dep_graph(project_dir, manifest, false, &activated_deps)?;
    let to_drop = check_slot_conflicts(&resolved_deps, manifest)?;
    let resolved_deps: Vec<ResolvedDep> = resolved_deps.into_iter()
        .filter(|d| !to_drop.contains(&d.name))
        .collect();
    let built = build_resolved_deps(manifest, project_dir, profile, templates, detected, &resolved_deps)?;
    let (foreign_built, pkg_configs) = foreign::build_foreign_deps(project_dir, manifest, profile)?;

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

    // Run build.freight (if present) and collect extra settings.
    let script_out = script::run_build_script(project_dir, manifest, profile, detected, &pkg_configs)?;
    include_dirs.extend(script_out.include_dirs.iter().cloned());
    let mut compile_defines = feature_defines.clone();
    compile_defines.extend(script_out.to_defines());
    compile_defines.extend(script_out.extra_flags.iter().cloned());
    for lib in &script_out.link_libs {
        all_raw_link_flags.push(format!("-l{lib}"));
    }
    all_raw_link_flags.extend(script_out.link_flags.iter().cloned());

    // Merge script-generated sources (add_source / compile_proto) into the
    // source list.  Language key is derived from file extension via ext_map.
    let mut all_sources = found.sources.clone();
    if !script_out.extra_sources.is_empty() {
        let ext_map = discover::build_ext_map(manifest, templates);
        for src_path in &script_out.extra_sources {
            let rel = src_path.strip_prefix(project_dir).unwrap_or(src_path).to_path_buf();
            let ext = rel.extension().and_then(|e| e.to_str())
                .map(|e| format!(".{e}")).unwrap_or_default();
            if let Some(lang_key) = ext_map.get(ext.as_str()) {
                if !all_sources.iter().any(|s| s.path == rel) {
                    all_sources.push(SourceFile { path: rel, lang_key: lang_key.clone() });
                }
            }
        }
    }

    // When the project uses C++20+, precompile dep headers as header units so
    // consumers can write `import "dep.h";` instead of `#include "dep.h"`.
    // Failures are non-fatal — we just skip and compile normally.
    let hu_flags: Vec<String> = if let Some(cpp_std) = manifest.language.get("cpp")
        .and_then(|l| l.std.as_deref())
        .filter(|s| header_units::is_module_std(s))
    {
        let units = header_units::precompile_dep_headers(
            project_dir, &all_dep_includes, cpp_std,
            &manifest.compiler.backend, detected, profile,
        );
        if let Some(compiler) = compile::select_compiler("cpp", &manifest.compiler.backend, detected, None) {
            header_units::import_flags(&units, compiler)
        } else { vec![] }
    } else { vec![] };

    // If a PCH header is configured, compile it and inject the use flag into
    // every source file. Failures are non-fatal.
    let pch_extra: Vec<String> = if let Some(ref pch_header) = manifest.compiler.pch.clone() {
        let primary = compile::select_compiler("cpp", &manifest.compiler.backend, detected, None)
            .or_else(|| compile::select_compiler("c", &manifest.compiler.backend, detected, None));
        if let Some(compiler) = primary {
            match pch::compile_pch(
                project_dir, pch_header, profile, compiler,
                &include_dirs, &compile_defines, &[],
            ) {
                Ok(Some(compiled)) => compiled.use_flag
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                Ok(None) => vec![],
                Err(e) => { eprintln!("warning: PCH skipped: {e}"); vec![] }
            }
        } else { vec![] }
    } else { vec![] };

    let mut extra_flags = hu_flags.clone();
    extra_flags.extend(pch_extra);
    let compile_result = build_sources(project_dir, manifest, profile, &all_sources, &include_dirs, detected, &compile_defines, &extra_flags)?;

    let link_result = link_targets(
        project_dir, manifest, profile,
        &compile_result.objects, detected, templates,
        &all_libs, &all_raw_link_flags,
    )?;

    // Keep freight.lock in sync with the resolved dep graph. Lock-write failures
    // are non-fatal — we surface them on stderr but still return success.
    let lock = LockFile::generate(project_dir, manifest, &resolved_deps);
    if let Err(e) = lock.save(project_dir) {
        eprintln!("warning: could not write freight.lock: {e}");
    }

    // Regenerate compile_commands.json so IDEs (clangd, fortls, serve-d…) stay
    // in sync. Non-fatal — a write failure must not abort a successful build.
    let cc = compile_commands::generate(
        project_dir, manifest, detected, profile,
        &all_sources, &include_dirs, &feature_defines,
    );
    if let Err(e) = compile_commands::write(project_dir, &cc) {
        eprintln!("warning: could not write compile_commands.json: {e}");
    }

    let binaries = link_result.outputs.iter()
        .filter(|p| !p.extension().is_some_and(|e| e == "a" || e == "so" || e == "dylib" || e == "dll"))
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
pub fn generate_compile_commands_at(project_dir: &Path, profile: &str) -> Result<usize, FreightError> {
    let ctx = load_project_at(project_dir, profile)?;
    let ProjectContext { project_dir, manifest, templates: _, detected, found } = &ctx;

    let resolution = features::resolve_features(&manifest.features, &[], true)?;
    let feature_defines = features::to_defines(&resolution.active);

    // Collect dep include dirs without triggering compilation.  Resolution
    // failures are non-fatal — we fall back to project-local dirs only.
    let dep_includes = collect_dep_include_dirs(project_dir, manifest);
    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(dep_includes);

    let commands = compile_commands::generate(
        project_dir, manifest, detected, profile,
        &found.sources, &include_dirs, &feature_defines,
    );
    let count = commands.len();
    compile_commands::write(project_dir, &commands)?;
    Ok(count)
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
pub fn test_project(profile: &str, filter: Option<&str>, features: &[String], use_defaults: bool, sanitize_override: &[String]) -> Result<TestSummary, FreightError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    test_project_at(&project_dir, profile, filter, features, use_defaults, sanitize_override)
}

/// Build and execute the project's test binaries.
pub fn test_project_at(project_dir: &Path, profile: &str, filter: Option<&str>, features: &[String], use_defaults: bool, sanitize_override: &[String]) -> Result<TestSummary, FreightError> {
    let mut ctx = load_project_at(project_dir, profile)?;
    if !sanitize_override.is_empty() {
        apply_sanitize_override(&mut ctx.manifest, profile, sanitize_override);
    }
    let ProjectContext { project_dir, manifest, templates, detected, found } = &ctx;

    use owo_colors::OwoColorize;
    println!("  {} {} ({profile})", "Testing".bold(), manifest.package.name);

    let profile_features_buf: Vec<String> = match profile {
        "dev"     => manifest.profile.dev.as_ref().map_or(vec![], |p| p.features.clone()),
        "release" => manifest.profile.release.as_ref().map_or(vec![], |p| p.features.clone()),
        other     => manifest.profile.custom.get(other).map_or(vec![], |p| p.features.clone()),
    };
    let all_requested: Vec<String> = features.iter().chain(profile_features_buf.iter()).cloned().collect();

    let resolution = features::resolve_features(&manifest.features, &all_requested, use_defaults)?;
    let feature_defines = features::to_defines(&resolution.active);
    let activated_deps = resolution.activated_deps;

    ensure_git_deps_fetched(project_dir, manifest)?;
    let existing_lock = LockFile::load(project_dir);
    if let Some(ref lock) = existing_lock {
        verify_git_dep_shas(project_dir, manifest, lock);
    }

    // Build deps (include dev-dependencies for test runs).
    let resolved_deps = resolve_dep_graph(project_dir, manifest, true, &activated_deps)?;
    let to_drop = check_slot_conflicts(&resolved_deps, manifest)?;
    let resolved_deps: Vec<ResolvedDep> = resolved_deps.into_iter()
        .filter(|d| !to_drop.contains(&d.name))
        .collect();
    let built = build_resolved_deps(manifest, project_dir, profile, templates, detected, &resolved_deps)?;
    let (foreign_built, pkg_configs) = foreign::build_foreign_deps(project_dir, manifest, profile)?;

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

    let script_out = script::run_build_script(project_dir, manifest, profile, detected, &pkg_configs)?;
    include_dirs.extend(script_out.include_dirs.iter().cloned());
    let mut compile_defines = feature_defines.clone();
    compile_defines.extend(script_out.to_defines());
    compile_defines.extend(script_out.extra_flags.iter().cloned());
    for lib in &script_out.link_libs {
        all_raw_link_flags.push(format!("-l{lib}"));
    }
    all_raw_link_flags.extend(script_out.link_flags.iter().cloned());

    let mut all_sources = found.sources.clone();
    if !script_out.extra_sources.is_empty() {
        let ext_map = discover::build_ext_map(manifest, templates);
        for src_path in &script_out.extra_sources {
            let rel = src_path.strip_prefix(project_dir).unwrap_or(src_path).to_path_buf();
            let ext = rel.extension().and_then(|e| e.to_str())
                .map(|e| format!(".{e}")).unwrap_or_default();
            if let Some(lang_key) = ext_map.get(ext.as_str()) {
                if !all_sources.iter().any(|s| s.path == rel) {
                    all_sources.push(SourceFile { path: rel, lang_key: lang_key.clone() });
                }
            }
        }
    }

    // PCH injection for test builds (same logic as build_project_at).
    let pch_extra_test: Vec<String> = if let Some(ref pch_header) = manifest.compiler.pch.clone() {
        let primary = compile::select_compiler("cpp", &manifest.compiler.backend, detected, None)
            .or_else(|| compile::select_compiler("c", &manifest.compiler.backend, detected, None));
        if let Some(compiler) = primary {
            match pch::compile_pch(
                project_dir, pch_header, profile, compiler,
                &include_dirs, &compile_defines, &[],
            ) {
                Ok(Some(compiled)) => compiled.use_flag
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                Ok(None) => vec![],
                Err(e) => { eprintln!("warning: PCH skipped: {e}"); vec![] }
            }
        } else { vec![] }
    } else { vec![] };

    let compile_result = build_sources(project_dir, manifest, profile, &all_sources, &include_dirs, detected, &compile_defines, &pch_extra_test)?;

    // Objects from [[bin]] sources contain a main() — exclude from test linking.
    let bin_obj_paths: std::collections::HashSet<PathBuf> = manifest.bins.iter()
        .map(|b| object_path(project_dir, profile, Path::new(&b.src)))
        .collect();
    let lib_objects: Vec<PathBuf> = compile_result.objects.iter()
        .filter(|o| !bin_obj_paths.contains(*o))
        .cloned()
        .collect();

    let test_dir = project_dir.join("tests");
    if !test_dir.is_dir() {
        return Ok(TestSummary { passed: 0, failed: 0, total: 0 });
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
                test_srcs.push(SourceFile { path: rel, lang_key: lang_key.clone() });
            }
        }
    }
    test_srcs.sort_by(|a, b| a.path.cmp(&b.path));

    if test_srcs.is_empty() {
        return Ok(TestSummary { passed: 0, failed: 0, total: 0 });
    }

    let test_compile = compile_sources(
        project_dir, manifest, profile, &test_srcs, &include_dirs, detected, &feature_defines, &[],
    )?;

    let out_dir = project_dir.join("target").join(profile).join("tests");
    std::fs::create_dir_all(&out_dir)?;

    println!("   {} tests\n", "Running".bold());

    let mut passed = 0usize;
    let mut failed = 0usize;

    for (src, test_obj) in test_srcs.iter().zip(test_compile.objects.iter()) {
        let stem = src.path.file_stem().and_then(|s| s.to_str()).unwrap_or("test");
        let test_bin = out_dir.join(stem);

        println!("   {} {stem}", "Linking".bold().cyan());

        let all_objs: Vec<PathBuf> = std::iter::once(test_obj.clone())
            .chain(lib_objects.iter().cloned())
            .collect();
        link_test_binary(
            &all_objs, &test_bin, manifest, profile,
            detected, templates, &all_libs, &all_raw_link_flags,
        )?;

        print!("test {stem} ... ");
        let ok = Command::new(&test_bin).status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ok {
            println!("{}", "ok".green());
            passed += 1;
        } else {
            println!("{}", "FAILED".red().bold());
            failed += 1;
        }
    }

    Ok(TestSummary { passed, failed, total: passed + failed })
}

// ── Git dep helpers ───────────────────────────────────────────────────────────

/// Auto-clone any git deps whose `.deps/<name>/` directory doesn't exist yet.
/// Runs silently when all deps are present.
fn ensure_git_deps_fetched(project_dir: &Path, manifest: &Manifest) -> Result<(), FreightError> {
    let deps_dir = project_dir.join(".deps");

    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else { continue };
        let Some(url) = &d.git else { continue };

        let dest = deps_dir.join(name);
        if dest.exists() {
            continue;
        }

        use owo_colors::OwoColorize;
        println!("    {} {} (git+{})", "Fetching".bold().cyan(), name, url);
        std::fs::create_dir_all(&deps_dir)?;
        git::clone_dep(&dest, url, d.branch.as_deref(), d.tag.as_deref(), d.rev.as_deref())?;
        println!();
    }

    Ok(())
}

/// After the dep graph is resolved, check each git dep's current commit against
/// the lock file. If a dep was pinned with `rev =` in the manifest, silently
/// enforce the pin by checking out that exact SHA. For branch-tracked deps,
/// print a warning when the repo has drifted from the locked SHA so the user
/// knows to run `freight update`.
fn verify_git_dep_shas(project_dir: &Path, manifest: &Manifest, lock: &LockFile) {
    for (name, dep) in &manifest.dependencies {
        let Dependency::Detailed(d) = dep else { continue };
        let Some(_url) = &d.git else { continue };

        let dep_dir = project_dir.join(".deps").join(name);
        if !dep_dir.exists() { continue; }

        let current = match git::current_rev(&dep_dir) {
            Some(sha) => sha,
            None => continue,
        };

        // Find the SHA the lock file recorded for this dep.
        let locked_sha = lock.packages.iter()
            .find(|p| &p.name == name)
            .and_then(|p| p.source.as_deref())
            .and_then(|src| src.split('#').nth(1))
            .map(str::to_string);

        let Some(locked) = locked_sha else { continue };

        // Rev-pinned: enforce the exact SHA.
        if let Some(pinned) = &d.rev {
            if !current.starts_with(pinned.as_str()) {
                if let Err(e) = git::checkout_rev(&dep_dir, pinned) {
                    eprintln!("warning: could not checkout pinned rev for `{name}`: {e}");
                }
            }
            continue;
        }

        // Branch/tag tracked: warn on drift.
        if !current.starts_with(locked.as_str()) && !locked.starts_with(current.as_str()) {
            eprintln!(
                "warning: git dep `{name}` is at {}, lock expects {}; \
                 run `freight update` to record the new SHA",
                &current[..current.len().min(12)],
                &locked[..locked.len().min(12)],
            );
        }
    }
}

// ── Source compilation (module-aware) ────────────────────────────────────────

/// Compile a project's sources, automatically switching to the module-aware pipeline
/// if any C++ source file contains an `export module` declaration.
fn build_sources(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    feature_defines: &[String],
    header_unit_flags: &[String],
) -> Result<CompileResult, FreightError> {
    let scanned = scan_sources(project_dir, sources);
    if has_modules(&scanned) {
        let mut plan = plan_module_build(project_dir, profile, scanned)?;
        compile_module_sources(project_dir, manifest, profile, &mut plan, include_dirs, detected, feature_defines, header_unit_flags)
    } else {
        compile_sources(project_dir, manifest, profile, sources, include_dirs, detected, feature_defines, header_unit_flags)
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
    profile: &str,
    templates: &[CompilerTemplate],
    detected: &[DetectedCompiler],
    resolved: &[ResolvedDep],
) -> Result<BuiltDeps, FreightError> {
    if resolved.is_empty() {
        return Ok(BuiltDeps { libs: vec![], include_dirs: vec![] });
    }

    let mut libs: Vec<PathBuf> = Vec::new();
    let mut all_include_dirs: Vec<PathBuf> = Vec::new();
    // Accumulate include dirs from already-built deps so later deps can see them.
    let mut built_include_dirs: Vec<PathBuf> = Vec::new();

    for dep in resolved {
        use owo_colors::OwoColorize;

        // Resolve which features are active for this dep based on the root's dep declaration.
        let dep_feature_defines = {
            let effective = root_manifest.effective_dependencies();
            let (req, use_defaults) = effective
                .get(&dep.name)
                .and_then(|d| if let Dependency::Detailed(d) = d { Some(d) } else { None })
                .map(|d| (d.features.clone(), d.default_features))
                .unwrap_or_default();
            let resolution = features::resolve_features(&dep.manifest.features, &req, use_defaults)?;
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

        println!("  {} {} ({profile})", "Building".dimmed(), dep.name);

        let compile_result = compile_sources(
            &dep.dir, &dep.manifest, profile,
            &dep_found.sources, &dep_include_dirs, detected, &dep_feature_defines, &[],
        )?;

        let lib_out = dep.dir.join("target").join(profile)
            .join(format!("lib{}.a", dep.name));
        std::fs::create_dir_all(lib_out.parent().expect("lib_out has parent"))?;

        if !lib_out.exists() || compile_result.compiled > 0 {
            println!(" {} lib{}.a", "Archiving".bold().cyan(), dep.name);
            let ar = select_linker(&dep.manifest, detected, templates)
                .map(|l| l.template.ar_binary().to_owned())
                .unwrap_or_else(|| "ar".to_owned());
            link_static_lib(&compile_result.objects, &lib_out, &ar)?;
        }

        libs.push(lib_out);
        all_include_dirs.extend(exported_includes.iter().cloned());
        built_include_dirs.extend(exported_includes);
    }

    Ok(BuiltDeps { libs, include_dirs: all_include_dirs })
}

// ── Sanitizer override helper ─────────────────────────────────────────────────

/// Patch the active profile's sanitize list in-place.
/// Creates the profile entry if it does not exist yet.
fn apply_sanitize_override(manifest: &mut crate::manifest::types::Manifest, profile: &str, sanitize: &[String]) {
    let list = sanitize.to_vec();
    match profile {
        "dev" => {
            manifest.profile.dev.get_or_insert_with(Default::default).sanitize = list;
        }
        "release" => {
            manifest.profile.release.get_or_insert_with(Default::default).sanitize = list;
        }
        other => {
            manifest.profile.custom
                .entry(other.to_string())
                .or_insert_with(Default::default)
                .sanitize = list;
        }
    }
}

// ── Shared project loading ────────────────────────────────────────────────────

fn load_project_at(project_dir: &Path, _profile: &str) -> Result<ProjectContext, FreightError> {
    let manifest = load_manifest(project_dir)?;

    let tdir = templates_dir()
        .ok_or_else(|| FreightError::CompilerNotFound(
            "toolchains directory not found; set CRANE_TEMPLATES_DIR".into(),
        ))?;
    let templates = load_templates(&tdir);

    validate_or_fail(&manifest, project_dir, &templates)?;

    let detected = detect_all_cached(&templates);
    if detected.is_empty() {
        return Err(FreightError::CompilerNotFound(
            "no compilers found on PATH — run `freight toolchain list`".into(),
        ));
    }

    let found = discover(project_dir, &manifest, &templates);
    if found.sources.is_empty() {
        return Err(FreightError::CompilerNotFound(
            "no source files found under src/".into(),
        ));
    }

    Ok(ProjectContext { project_dir: project_dir.to_path_buf(), manifest, templates, detected, found })
}

fn validate_or_fail(
    manifest: &Manifest,
    project_dir: &Path,
    templates: &[CompilerTemplate],
) -> Result<(), FreightError> {
    let mut errors = validate(manifest, templates);
    errors.extend(validate_dep_compat(manifest, project_dir, templates));
    if errors.is_empty() { return Ok(()); }
    let msgs: Vec<String> = errors.iter()
        .map(|e| format!("{}: {}", e.context, e.message))
        .collect();
    Err(FreightError::ManifestParse(msgs.join("\n")))
}
