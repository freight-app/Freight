pub mod compile;
pub mod deps;
pub mod discover;
pub mod link;

pub use compile::{CompileResult, compile_sources, dep_file_path, object_path, select_compiler, settings_for_lang};
pub use deps::{ResolvedDep, resolve_dep_graph};
pub use discover::{DiscoveredSources, SourceFile, discover};
pub use link::{LinkResult, link_static_lib, link_targets, link_test_binary, select_linker};

use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

use crate::error::CraneError;
use crate::manifest::types::Manifest;
use crate::manifest::validate::{validate, validate_dep_compat};
use crate::manifest::{find_manifest_dir, load_manifest};
use crate::output::{print_error, print_success};
use crate::toolchain::{CompilerTemplate, DetectedCompiler, detect_all_cached, load_templates, templates_dir};

// ── Public commands ───────────────────────────────────────────────────────────

/// Implementation of `crane build [--release]`.
pub fn cmd_build(release: bool) {
    let profile = if release { "release" } else { "dev" };
    match build_project(profile) {
        Ok(output) => {
            println!();
            print_success(&format!(
                "{} ({} compiled, {} up to date)",
                output.package_name, output.compiled, output.skipped,
            ));
            for bin in &output.binaries {
                println!("    {}", bin.display());
            }
        }
        Err(e) => {
            println!();
            print_error(&e.to_string());
        }
    }
}

/// Implementation of `crane run [--release] [-- args...]`.
pub fn cmd_run(release: bool, run_args: &[String]) {
    let profile = if release { "release" } else { "dev" };
    let output = match build_project(profile) {
        Ok(o) => o,
        Err(e) => { println!(); print_error(&e.to_string()); return; }
    };

    match output.binaries.as_slice() {
        [] => {
            print_error("no binary target produced — add a [[bin]] section to crane.toml");
        }
        [bin] => {
            println!();
            use owo_colors::OwoColorize;
            println!("    {} {}", "Running".bold().green(), bin.display());
            println!();
            let status = Command::new(bin).args(run_args).status();
            match status {
                Ok(s) if !s.success() => {
                    if let Some(code) = s.code() {
                        print_error(&format!("process exited with code {code}"));
                    }
                }
                Err(e) => print_error(&format!("failed to run binary: {e}")),
                Ok(_) => {}
            }
        }
        _ => {
            print_error("multiple [[bin]] targets — specify which to run (not yet supported)");
            for b in &output.binaries {
                eprintln!("  {}", b.display());
            }
        }
    }
}

/// Implementation of `crane clean` — remove the `target/` directory.
pub fn cmd_clean() {
    match clean_project() {
        Ok(()) => print_success("cleaned target/"),
        Err(e) => { println!(); print_error(&e.to_string()); }
    }
}

/// Implementation of `crane test [name]`.
pub fn cmd_test(filter: Option<&str>) {
    match test_project("dev", filter) {
        Ok(summary) => {
            println!();
            if summary.total == 0 {
                println!("no test files found under tests/");
                return;
            }
            if summary.failed == 0 {
                print_success(&format!(
                    "test result: ok. {} passed; 0 failed", summary.passed,
                ));
            } else {
                print_error(&format!(
                    "test result: FAILED. {} passed; {} failed",
                    summary.passed, summary.failed,
                ));
            }
        }
        Err(e) => {
            println!();
            print_error(&e.to_string());
        }
    }
}

// ── Internal structs ──────────────────────────────────────────────────────────

struct BuildOutput {
    package_name: String,
    binaries: Vec<PathBuf>,
    compiled: usize,
    skipped: usize,
}

struct TestSummary {
    passed: usize,
    failed: usize,
    total: usize,
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

fn build_project(profile: &str) -> Result<BuildOutput, CraneError> {
    let ctx = load_project(profile)?;
    let ProjectContext { project_dir, manifest, templates, detected, found } = &ctx;

    use owo_colors::OwoColorize;
    println!("  {} {} ({profile})", "Building".bold(), manifest.package.name);

    let built = build_deps(project_dir, manifest, profile, templates, detected, false)?;

    // Merge dep include dirs into the set passed to the root compile step.
    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(built.include_dirs.iter().cloned());

    let compile_result = compile_sources(
        project_dir, manifest, profile, &found.sources, &include_dirs, detected,
    )?;

    let link_result = link_targets(
        project_dir, manifest, profile,
        &compile_result.objects, detected, templates,
        &built.libs,
    )?;

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

fn clean_project() -> Result<(), CraneError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let target = project_dir.join("target");
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    Ok(())
}

fn test_project(profile: &str, filter: Option<&str>) -> Result<TestSummary, CraneError> {
    let ctx = load_project(profile)?;
    let ProjectContext { project_dir, manifest, templates, detected, found } = &ctx;

    use owo_colors::OwoColorize;
    println!("  {} {} ({profile})", "Testing".bold(), manifest.package.name);

    // Build deps (include dev-dependencies for test runs).
    let built = build_deps(project_dir, manifest, profile, templates, detected, true)?;

    let mut include_dirs = found.include_dirs.clone();
    include_dirs.extend(built.include_dirs.iter().cloned());

    let compile_result = compile_sources(
        project_dir, manifest, profile, &found.sources, &include_dirs, detected,
    )?;

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
            detected, templates, &built.libs,
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

// ── Dependency build step ─────────────────────────────────────────────────────

/// Compile and archive all local deps in topological order.
///
/// Returns the set of `.a` lib paths and include directories to expose to the
/// root project's compile and link steps.
fn build_deps(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    templates: &[CompilerTemplate],
    detected: &[DetectedCompiler],
    include_dev: bool,
) -> Result<BuiltDeps, CraneError> {
    let resolved = resolve_dep_graph(project_dir, manifest, include_dev)?;

    if resolved.is_empty() {
        return Ok(BuiltDeps { libs: vec![], include_dirs: vec![] });
    }

    let mut libs: Vec<PathBuf> = Vec::new();
    let mut all_include_dirs: Vec<PathBuf> = Vec::new();
    // Accumulate include dirs from already-built deps so later deps can see them.
    let mut built_include_dirs: Vec<PathBuf> = Vec::new();

    for dep in &resolved {
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

fn load_project(_profile: &str) -> Result<ProjectContext, CraneError> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd)
        .ok_or_else(|| CraneError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
    let manifest = load_manifest(&project_dir)?;

    let tdir = templates_dir()
        .ok_or_else(|| CraneError::CompilerNotFound(
            "compiler-templates directory not found; set CRANE_TEMPLATES_DIR".into(),
        ))?;
    let templates = load_templates(&tdir);

    validate_or_fail(&manifest, &project_dir, &templates)?;

    let detected = detect_all_cached(&templates);
    if detected.is_empty() {
        return Err(CraneError::CompilerNotFound(
            "no compilers found on PATH — run `crane toolchain list`".into(),
        ));
    }

    let found = discover(&project_dir, &manifest, &templates);
    if found.sources.is_empty() {
        return Err(CraneError::CompilerNotFound(
            "no source files found under src/".into(),
        ));
    }

    Ok(ProjectContext { project_dir, manifest, templates, detected, found })
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
