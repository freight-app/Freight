pub mod compile;
pub mod deps;
pub mod discover;
pub mod foreign;
pub mod link;
pub mod modules;

pub use compile::{CompileResult, compile_sources, dep_file_path, object_path, select_compiler, settings_for_lang};
pub use deps::{ResolvedDep, resolve_dep_graph};
pub use discover::{DiscoveredSources, SourceFile, discover};
pub use link::{LinkResult, link_static_lib, link_targets, link_test_binary, select_linker};
pub use modules::{ModuleBuildPlan, ModuleRole, ScannedSource, bmi_path, compile_module_sources, has_modules, plan_module_build, scan_sources};

use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

use crate::error::CraneError;
use crate::lock::LockFile;
use crate::manifest::types::Manifest;
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
pub fn build_workspace(profile: &str) -> Result<Vec<BuildOutput>, CraneError> {
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| CraneError::ManifestParse("not a workspace root".into()))?;

    let mut outputs = Vec::new();
    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        outputs.push(build_project_at(&member_dir, profile)?);
    }
    Ok(outputs)
}

/// Clean every member of a workspace rooted at the current working directory.
pub fn clean_workspace() -> Result<(), CraneError> {
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| CraneError::ManifestParse("not a workspace root".into()))?;

    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        clean_project_at(&member_dir)?;
    }
    Ok(())
}

/// Test every member of a workspace rooted at the current working directory.
pub fn test_workspace(profile: &str, filter: Option<&str>) -> Result<TestSummary, CraneError> {
    let cwd = std::env::current_dir()?;
    let ws_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let ws = load_workspace_manifest(&ws_dir)
        .ok_or_else(|| CraneError::ManifestParse("not a workspace root".into()))?;

    let mut total_passed = 0;
    let mut total_failed = 0;
    for member in &ws.members {
        let member_dir = ws_dir.join(member.trim_end_matches('/'));
        let s = test_project_at(&member_dir, profile, filter)?;
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
pub fn build_project(profile: &str) -> Result<BuildOutput, CraneError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    build_project_at(&project_dir, profile)
}

/// Build the project at a specific `project_dir`.
pub fn build_project_at(project_dir: &Path, profile: &str) -> Result<BuildOutput, CraneError> {
    let ctx = load_project_at(project_dir, profile)?;
    let ProjectContext { project_dir, manifest, templates, detected, found } = &ctx;

    use owo_colors::OwoColorize;
    println!("  {} {} ({profile})", "Building".bold(), manifest.package.name);

    let resolved_deps = resolve_dep_graph(project_dir, manifest, false)?;
    let built = build_resolved_deps(project_dir, profile, templates, detected, &resolved_deps)?;
    let foreign_built = foreign::build_foreign_deps(project_dir, manifest, profile)?;

    let mut all_libs = built.libs.clone();
    let mut all_dep_includes = built.include_dirs.clone();
    for f in foreign_built {
        all_libs.extend(f.libs);
        all_dep_includes.extend(f.include_dirs);
    }

    // Merge dep include dirs into the set passed to the root compile step.
    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(all_dep_includes.iter().cloned());

    let compile_result = build_sources(project_dir, manifest, profile, &found.sources, &include_dirs, detected)?;

    let link_result = link_targets(
        project_dir, manifest, profile,
        &compile_result.objects, detected, templates,
        &all_libs,
    )?;

    // Keep crane.lock in sync with the resolved dep graph. Lock-write failures
    // are non-fatal — we surface them on stderr but still return success.
    let lock = LockFile::generate(project_dir, manifest, &resolved_deps);
    if let Err(e) = lock.save(project_dir) {
        eprintln!("warning: could not write crane.lock: {e}");
    }

    let binaries = link_result.outputs.iter()
        .filter(|p| !p.extension().is_some_and(|e| e == "a" || e == "so"))
        .cloned()
        .collect();

    Ok(BuildOutput {
        package_name: manifest.package.name.clone(),
        binaries,
        compiled: compile_result.compiled,
        skipped: compile_result.skipped,
    })
}

/// Wipe the project's `target/` directory (finds project by walking up from cwd).
pub fn clean_project() -> Result<(), CraneError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    clean_project_at(&project_dir)
}

/// Wipe the `target/` directory of the project at `project_dir`.
pub fn clean_project_at(project_dir: &Path) -> Result<(), CraneError> {
    let target = project_dir.join("target");
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    Ok(())
}

/// Build and run the tests of the project rooted at the current working directory.
pub fn test_project(profile: &str, filter: Option<&str>) -> Result<TestSummary, CraneError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    test_project_at(&project_dir, profile, filter)
}

/// Build and execute the project's test binaries.
pub fn test_project_at(project_dir: &Path, profile: &str, filter: Option<&str>) -> Result<TestSummary, CraneError> {
    let ctx = load_project_at(project_dir, profile)?;
    let ProjectContext { project_dir, manifest, templates, detected, found } = &ctx;

    use owo_colors::OwoColorize;
    println!("  {} {} ({profile})", "Testing".bold(), manifest.package.name);

    // Build deps (include dev-dependencies for test runs).
    let resolved_deps = resolve_dep_graph(project_dir, manifest, true)?;
    let built = build_resolved_deps(project_dir, profile, templates, detected, &resolved_deps)?;
    let foreign_built = foreign::build_foreign_deps(project_dir, manifest, profile)?;

    let mut all_libs = built.libs.clone();
    let mut all_dep_includes = built.include_dirs.clone();
    for f in foreign_built {
        all_libs.extend(f.libs);
        all_dep_includes.extend(f.include_dirs);
    }

    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(all_dep_includes.iter().cloned());

    let compile_result = build_sources(project_dir, manifest, profile, &found.sources, &include_dirs, detected)?;

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
        project_dir, manifest, profile, &test_srcs, &include_dirs, detected,
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
            &all_objs, &test_bin, manifest, profile, project_dir,
            detected, templates, &all_libs,
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
) -> Result<CompileResult, CraneError> {
    let scanned = scan_sources(project_dir, sources);
    if has_modules(&scanned) {
        let mut plan = plan_module_build(project_dir, profile, scanned)?;
        compile_module_sources(project_dir, manifest, profile, &mut plan, include_dirs, detected)
    } else {
        compile_sources(project_dir, manifest, profile, sources, include_dirs, detected)
    }
}

// ── Dependency build step ─────────────────────────────────────────────────────

/// Compile and archive all local deps in topological order.
///
/// Takes an already-resolved dep list so callers can reuse the resolution for
/// lockfile generation without a second walk.
fn build_resolved_deps(
    _project_dir: &Path,
    profile: &str,
    templates: &[CompilerTemplate],
    detected: &[DetectedCompiler],
    resolved: &[ResolvedDep],
) -> Result<BuiltDeps, CraneError> {
    if resolved.is_empty() {
        return Ok(BuiltDeps { libs: vec![], include_dirs: vec![] });
    }

    let mut libs: Vec<PathBuf> = Vec::new();
    let mut all_include_dirs: Vec<PathBuf> = Vec::new();
    // Accumulate include dirs from already-built deps so later deps can see them.
    let mut built_include_dirs: Vec<PathBuf> = Vec::new();

    for dep in resolved {
        use owo_colors::OwoColorize;

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
            &dep_found.sources, &dep_include_dirs, detected,
        )?;

        let lib_out = dep.dir.join("target").join(profile)
            .join(format!("lib{}.a", dep.name));
        std::fs::create_dir_all(lib_out.parent().expect("lib_out has parent"))?;

        if !lib_out.exists() || compile_result.compiled > 0 {
            println!(" {} lib{}.a", "Archiving".bold().cyan(), dep.name);
            link_static_lib(&compile_result.objects, &lib_out)?;
        }

        libs.push(lib_out);
        all_include_dirs.extend(exported_includes.iter().cloned());
        built_include_dirs.extend(exported_includes);
    }

    Ok(BuiltDeps { libs, include_dirs: all_include_dirs })
}

// ── Shared project loading ────────────────────────────────────────────────────

fn load_project_at(project_dir: &Path, _profile: &str) -> Result<ProjectContext, CraneError> {
    let manifest = load_manifest(project_dir)?;

    let tdir = templates_dir()
        .ok_or_else(|| CraneError::CompilerNotFound(
            "compiler-templates directory not found; set CRANE_TEMPLATES_DIR".into(),
        ))?;
    let templates = load_templates(&tdir);

    validate_or_fail(&manifest, project_dir, &templates)?;

    let detected = detect_all_cached(&templates);
    if detected.is_empty() {
        return Err(CraneError::CompilerNotFound(
            "no compilers found on PATH — run `crane toolchain list`".into(),
        ));
    }

    let found = discover(project_dir, &manifest, &templates);
    if found.sources.is_empty() {
        return Err(CraneError::CompilerNotFound(
            "no source files found under src/".into(),
        ));
    }

    Ok(ProjectContext { project_dir: project_dir.to_path_buf(), manifest, templates, detected, found })
}

fn validate_or_fail(
    manifest: &Manifest,
    project_dir: &Path,
    templates: &[CompilerTemplate],
) -> Result<(), CraneError> {
    let mut errors = validate(manifest, templates);
    errors.extend(validate_dep_compat(manifest, project_dir, templates));
    if errors.is_empty() { return Ok(()); }
    let msgs: Vec<String> = errors.iter()
        .map(|e| format!("{}: {}", e.context, e.message))
        .collect();
    Err(CraneError::ManifestParse(msgs.join("\n")))
}
