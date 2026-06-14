/// Pipeline configuration types and the unified build pipeline.
///
/// The pipeline is split into discrete stages so each can be called
/// independently from [`super::project::Project`] methods or the thin
/// wrapper functions in `mod.rs`.
///
/// Stages:
///  1. `load`          — parse manifest, detect toolchain, discover sources
///  2. `features`      — resolve feature flags, compute preprocessor defines
///  3. `fetch`         — clone/download missing git + registry deps
///  4. `resolve_deps`  — dep graph topo-sort + slot-conflict check
///  5. `build_deps`    — compile source-built deps + foreign (cmake/make/…) deps
///  6. `assemble_includes` — merge `[compiler] includes`, discovered dirs, dep dirs
///  7. `codegen`       — run `protoc` if `[language.proto]` is declared
///  8. `header_units`  — precompile dep headers as BMIs (C++20 builds only)
///  9. `pch`           — compile precompiled header if configured
/// 10. `compile`       — compile all project sources in parallel
/// 11. goal phase      — link (build), run tests, or run benchmarks
use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

use crate::build::{
    compile, compile_commands, discover, features, header_units, link_targets, link_test_binary,
    object_path, pch, proto, BenchResult, BenchSummary, BuildOutput, CompileResult, PipelineOutput,
    SourceFile, TestSummary,
};
use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::lock::LockFile;
use crate::manifest::types::Manifest;
use crate::toolchain::GlobalConfig;

use super::{
    apply_sanitize_override, build_resolved_deps, build_sources, check_slot_conflicts,
    ensure_git_deps_fetched, inject_option_handler_flags, load_project_at,
    lsp_compile_commands_dir, resolve_dep_graph, safe_lsp_profile_dir, verify_git_dep_shas,
    ProjectContext, ResolvedDep,
};

// ── Pipeline goal / config / output ──────────────────────────────────────────

/// What the pipeline should do after compiling sources.
#[derive(Debug, Clone, Default)]
pub enum PipelineGoal {
    #[default]
    Build,
    Test {
        filter: Option<String>,
    },
    Bench {
        filter: Option<String>,
    },
}

impl PipelineGoal {
    pub fn include_dev_deps(&self) -> bool {
        matches!(self, Self::Test { .. })
    }
}

/// All inputs for a single `run_pipeline_at` call.
#[derive(Debug, Clone, Default)]
pub struct PipelineConfig {
    pub profile: String,
    pub features: Vec<String>,
    pub use_defaults: bool,
    pub target_override: Option<String>,
    pub sanitize_override: Vec<String>,
    pub goal: PipelineGoal,
}

// ── Stage results ─────────────────────────────────────────────────────────────

/// Output of the feature-resolution stage.
pub struct FeatureResolution {
    pub defines: Vec<String>,
    pub activated_deps: std::collections::BTreeSet<String>,
    /// Defines forwarded into specific deps via `<dep>/define:NAME`, keyed by dep.
    pub dep_defines: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

/// Aggregated dep output: static libs + include dirs + raw link flags + tool paths.
pub struct BuiltDepsOutput {
    pub libs: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub raw_link_flags: Vec<String>,
    pub tool_paths: Vec<PathBuf>,
}

/// Compiler flags from the PCH stage.
pub struct PchFlags {
    /// Flags injected into every source compilation (`-include-pch …`).
    pub compile: Vec<String>,
    /// Flags for `clangd` entries in `compile_commands.json`.
    pub clangd: Vec<String>,
}

// ── Stage functions ───────────────────────────────────────────────────────────

/// Stage 2: resolve feature flags and compute preprocessor defines.
pub fn stage_features(
    manifest: &Manifest,
    config: &PipelineConfig,
) -> Result<FeatureResolution, FreightError> {
    let profile_features: Vec<String> = match config.profile.as_str() {
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
    let all_requested: Vec<String> = config
        .features
        .iter()
        .chain(profile_features.iter())
        .cloned()
        .collect();
    let resolution =
        features::resolve_features(&manifest.features, &all_requested, config.use_defaults)?;
    // Auto `-D<FEATURE>` defines from active feature names, plus any explicit
    // `define:NAME[=value]` entries those features carried.
    let mut defines = features::to_defines(&resolution.active);
    defines.extend(resolution.defines.iter().cloned());
    Ok(FeatureResolution {
        defines,
        activated_deps: resolution.activated_deps,
        dep_defines: resolution.dep_defines,
    })
}

/// Stage 3: fetch missing git + registry deps and verify the lock file.
pub fn stage_fetch(
    project_dir: &Path,
    root_dir: &Path,
    manifest: &Manifest,
    progress: &Progress,
) -> Result<Option<LockFile>, FreightError> {
    ensure_git_deps_fetched(project_dir, manifest, progress)?;
    {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(project_dir) {
            cfg.apply_local(local);
        }
        if let Ok(outcomes) = crate::dep_cmds::fetch_registry_deps(root_dir, &cfg) {
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
    let lock = LockFile::load(project_dir);
    if let Some(ref l) = lock {
        verify_git_dep_shas(project_dir, manifest, l, progress);
    }
    Ok(lock)
}

/// Stages 4 + 5: resolve the dep graph, check slot conflicts, build all deps.
pub(crate) fn stage_build_deps(
    project_dir: &Path,
    root_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    include_dev: bool,
    activated_deps: &std::collections::BTreeSet<String>,
    dep_defines: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    ctx: &ProjectContext,
    progress: &Progress,
) -> Result<(Vec<ResolvedDep>, BuiltDepsOutput), FreightError> {
    let resolved = resolve_dep_graph(project_dir, manifest, include_dev, activated_deps)?;
    let to_drop = check_slot_conflicts(&resolved, manifest)?;
    let resolved: Vec<ResolvedDep> = resolved
        .into_iter()
        .filter(|d| !to_drop.contains(&d.name))
        .collect();

    let built = build_resolved_deps(
        manifest,
        project_dir,
        &ctx.effective_backend,
        profile,
        &ctx.templates,
        &ctx.detected,
        &resolved,
        dep_defines,
        progress,
    )?;
    let (foreign, _pc, tool_paths) = crate::adaptors::build_foreign_deps(
        project_dir,
        root_dir,
        manifest,
        profile,
        dep_defines,
        progress,
    )?;

    let mut output = BuiltDepsOutput {
        libs: built.libs,
        include_dirs: built.include_dirs,
        raw_link_flags: Vec::new(),
        tool_paths,
    };
    for f in foreign {
        output.libs.extend(f.libs);
        output.include_dirs.extend(f.include_dirs);
        output.raw_link_flags.extend(f.raw_link_flags);
    }
    Ok((resolved, output))
}

/// Stage 6: merge `[compiler] includes` + discovered dirs + dep include dirs.
pub fn stage_assemble_includes(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    found_include_dirs: &[PathBuf],
    dep_include_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    let settings = manifest.build_settings_for(profile);
    let mut dirs: Vec<PathBuf> = settings
        .include_paths
        .iter()
        .map(|p| project_dir.join(p))
        .collect();
    for d in found_include_dirs {
        let abs = project_dir.join(d);
        if !dirs.contains(&abs) {
            dirs.push(abs);
        }
    }
    dirs.extend_from_slice(dep_include_dirs);
    dirs
}

/// Stage 7: run `protoc` codegen if `[language.proto]` is declared.
/// Returns the (possibly extended) source list and include dirs.
pub fn stage_codegen(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    base_sources: Vec<SourceFile>,
    include_dirs: &mut Vec<PathBuf>,
    tool_paths: &[PathBuf],
    progress: &Progress,
) -> Result<Vec<SourceFile>, FreightError> {
    if !manifest.language.contains_key("proto") {
        return Ok(base_sources);
    }
    let proto_settings = manifest.effective_language_settings("proto");
    let result = proto::run_protoc(project_dir, profile, &proto_settings, tool_paths, progress)?;
    if !result.generated_include_dir.as_os_str().is_empty() {
        include_dirs.push(result.generated_include_dir);
    }
    let mut srcs = base_sources;
    srcs.extend(result.generated_sources);
    Ok(srcs)
}

/// Stage 8: precompile dep headers as BMIs (C++20 builds only; no-op otherwise).
pub(crate) fn stage_header_units(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    dep_include_dirs: &[PathBuf],
    ctx: &ProjectContext,
) -> Vec<String> {
    let Some(cpp_std) = manifest
        .language
        .get("cpp")
        .and_then(|l| l.std.as_deref())
        .filter(|s| header_units::is_module_std(s))
    else {
        return vec![];
    };

    let units = header_units::precompile_dep_headers(
        project_dir,
        dep_include_dirs,
        cpp_std,
        &ctx.effective_backend,
        &ctx.detected,
        profile,
    );
    if let Some(compiler) =
        compile::select_compiler("cpp", &ctx.effective_backend, &ctx.detected, None)
    {
        header_units::import_flags(&units, compiler)
    } else {
        vec![]
    }
}

/// Stage 9: compile the precompiled header if configured; returns compile + clangd flags.
pub(crate) fn stage_pch(
    project_dir: &Path,
    target_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    include_dirs: &[PathBuf],
    defines: &[String],
    ctx: &ProjectContext,
    progress: &Progress,
) -> PchFlags {
    let Some(ref pch_header) = manifest.compiler.pch.clone() else {
        return PchFlags {
            compile: vec![],
            clangd: vec![],
        };
    };
    let primary = compile::select_compiler("cpp", &ctx.effective_backend, &ctx.detected, None)
        .or_else(|| compile::select_compiler("c", &ctx.effective_backend, &ctx.detected, None));
    let Some(compiler) = primary else {
        return PchFlags {
            compile: vec![],
            clangd: vec![],
        };
    };
    match pch::compile_pch(
        project_dir,
        target_dir,
        pch_header,
        profile,
        compiler,
        include_dirs,
        defines,
        &[],
    ) {
        Ok(Some(compiled)) => PchFlags {
            compile: compiled
                .use_flag
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            clangd: compiled
                .clangd_flag
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        },
        Ok(None) => PchFlags {
            compile: vec![],
            clangd: vec![],
        },
        Err(e) => {
            progress(BuildEvent::Warning(format!("PCH skipped: {e}")));
            PchFlags {
                compile: vec![],
                clangd: vec![],
            }
        }
    }
}

// ── Goal phase helpers ────────────────────────────────────────────────────────

fn lib_objects_excluding_bins(
    manifest: &Manifest,
    target_dir: &Path,
    profile: &str,
    objects: &[PathBuf],
) -> Vec<PathBuf> {
    let bin_objs: std::collections::HashSet<PathBuf> = manifest
        .bins
        .iter()
        .map(|b| object_path(target_dir, profile, Path::new(&b.src)))
        .collect();
    objects
        .iter()
        .filter(|o| !bin_objs.contains(*o))
        .cloned()
        .collect()
}

fn discover_goal_sources(
    dir: &Path,
    project_dir: &Path,
    manifest: &Manifest,
    templates: &[crate::toolchain::CompilerTemplate],
    filter: Option<&str>,
    default_stem: &str,
) -> Vec<SourceFile> {
    if !dir.is_dir() {
        return vec![];
    }
    let ext_map = discover::build_ext_map(manifest, templates);
    let mut srcs: Vec<SourceFile> = WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let path = e.path();
            let ext = format!(".{}", path.extension()?.to_str()?);
            let lang_key = ext_map.get(ext.as_str())?.clone();
            let stem = path.file_stem()?.to_str().unwrap_or(default_stem);
            if filter.is_none_or(|f| f == stem) {
                let rel = path.strip_prefix(project_dir).unwrap_or(path).to_path_buf();
                Some(SourceFile {
                    path: rel,
                    lang_key,
                })
            } else {
                None
            }
        })
        .collect();
    srcs.sort_by(|a, b| a.path.cmp(&b.path));
    srcs
}

fn run_test_goal(
    filter: Option<&str>,
    project_dir: &Path,
    target_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    ctx: &ProjectContext,
    compile_result: &CompileResult,
    deps: &BuiltDepsOutput,
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    progress: &Progress,
) -> Result<TestSummary, FreightError> {
    let lib_objs =
        lib_objects_excluding_bins(manifest, target_dir, profile, &compile_result.objects);
    let srcs = discover_goal_sources(
        &project_dir.join("tests"),
        project_dir,
        manifest,
        &ctx.templates,
        filter,
        "test",
    );
    if srcs.is_empty() {
        return Ok(TestSummary {
            passed: 0,
            failed: 0,
            total: 0,
        });
    }

    let compiled = crate::build::compile_sources(
        project_dir,
        target_dir,
        manifest,
        &ctx.effective_backend,
        profile,
        &srcs,
        include_dirs,
        &ctx.detected,
        feature_defines,
        &[],
        progress,
    )?;

    let out_dir = target_dir.join(profile).join("tests");
    std::fs::create_dir_all(&out_dir)?;

    let (mut passed, mut failed) = (0usize, 0usize);
    for (src, obj) in srcs.iter().zip(compiled.objects.iter()) {
        let stem = src
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("test");
        let bin = out_dir.join(stem);
        progress(BuildEvent::TestLinking {
            name: stem.to_string(),
        });
        let all_objs: Vec<PathBuf> = std::iter::once(obj.clone())
            .chain(lib_objs.iter().cloned())
            .collect();
        link_test_binary(
            &all_objs,
            &bin,
            manifest,
            &ctx.effective_backend,
            profile,
            &ctx.detected,
            &ctx.templates,
            &deps.libs,
            &deps.raw_link_flags,
        )?;
        progress(BuildEvent::TestRunning {
            name: stem.to_string(),
        });
        let ok = Command::new(&bin)
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

fn run_bench_goal(
    filter: Option<&str>,
    project_dir: &Path,
    target_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    ctx: &ProjectContext,
    compile_result: &CompileResult,
    deps: &BuiltDepsOutput,
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    progress: &Progress,
) -> Result<BenchSummary, FreightError> {
    const BENCH_RUNS: usize = 5;
    let lib_objs =
        lib_objects_excluding_bins(manifest, target_dir, profile, &compile_result.objects);
    let srcs = discover_goal_sources(
        &project_dir.join("benches"),
        project_dir,
        manifest,
        &ctx.templates,
        filter,
        "bench",
    );
    if srcs.is_empty() {
        return Ok(BenchSummary { results: vec![] });
    }

    let compiled = crate::build::compile_sources(
        project_dir,
        target_dir,
        manifest,
        &ctx.effective_backend,
        profile,
        &srcs,
        include_dirs,
        &ctx.detected,
        feature_defines,
        &[],
        progress,
    )?;

    let out_dir = target_dir.join(profile).join("benches");
    std::fs::create_dir_all(&out_dir)?;
    let mut results = Vec::new();

    for (src, obj) in srcs.iter().zip(compiled.objects.iter()) {
        let stem = src
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("bench");
        let bin = out_dir.join(stem);
        progress(BuildEvent::BenchLinking {
            name: stem.to_string(),
        });
        let all_objs: Vec<PathBuf> = std::iter::once(obj.clone())
            .chain(lib_objs.iter().cloned())
            .collect();
        link_test_binary(
            &all_objs,
            &bin,
            manifest,
            &ctx.effective_backend,
            profile,
            &ctx.detected,
            &ctx.templates,
            &deps.libs,
            &deps.raw_link_flags,
        )?;
        progress(BuildEvent::BenchRunning {
            name: stem.to_string(),
        });

        let mut samples: Vec<u64> = Vec::with_capacity(BENCH_RUNS);
        for _ in 0..BENCH_RUNS {
            let t0 = std::time::Instant::now();
            if Command::new(&bin)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                samples.push(t0.elapsed().as_nanos() as u64);
            }
        }
        let runs = samples.len();
        let (mean_ns, min_ns, max_ns) = if runs == 0 {
            (0, 0, 0)
        } else {
            let sum: u64 = samples.iter().sum();
            (
                sum / runs as u64,
                *samples.iter().min().unwrap(),
                *samples.iter().max().unwrap(),
            )
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

// ── Orchestrator ──────────────────────────────────────────────────────────────

/// Run all pipeline stages for the project at `project_dir`.
///
/// `parent_root` — when building a dep from source, pass the root project's
/// directory so the flat `.pkgs/` pool is anchored there instead of inside the
/// dep's own directory.
pub fn run_pipeline_at(
    project_dir: &Path,
    config: &PipelineConfig,
    parent_root: Option<&Path>,
    progress: &Progress,
) -> Result<PipelineOutput, FreightError> {
    let profile = &config.profile;

    // ── Stage 1: Load ────────────────────────────────────────────────────────
    let mut ctx = load_project_at(project_dir, profile)?;
    if let Some(t) = &config.target_override {
        ctx.manifest.compiler.target = Some(t.clone());
    }
    if !config.sanitize_override.is_empty() {
        apply_sanitize_override(&mut ctx.manifest, profile, &config.sanitize_override);
    }
    inject_option_handler_flags(&mut ctx)?;
    let ProjectContext {
        project_dir,
        manifest,
        found,
        ..
    } = &ctx;

    let root_dir: &Path = parent_root.unwrap_or(project_dir);
    let target_dir: PathBuf = if let Some(pr) = parent_root {
        pr.join("target").join("deps").join(&manifest.package.name)
    } else {
        project_dir.join("target")
    };

    progress(BuildEvent::BuildStarted {
        name: manifest.package.name.clone(),
        profile: profile.to_string(),
    });

    // ── Stage 2: Features ────────────────────────────────────────────────────
    let feat = stage_features(manifest, config)?;

    // ── Stage 3: Fetch ───────────────────────────────────────────────────────
    stage_fetch(project_dir, root_dir, manifest, progress)?;

    // ── Stage 4+5: Resolve + build deps ──────────────────────────────────────
    let include_dev = config.goal.include_dev_deps();
    let (resolved, native_deps) = stage_build_deps(
        project_dir,
        root_dir,
        manifest,
        profile,
        include_dev,
        &feat.activated_deps,
        &feat.dep_defines,
        &ctx,
        progress,
    )?;
    let deps = native_deps;

    // ── Stage 6: Assemble include dirs ───────────────────────────────────────
    let mut include_dirs = stage_assemble_includes(
        project_dir,
        manifest,
        profile,
        &found.include_dirs,
        &deps.include_dirs,
    );

    // ── Stage 7: Proto codegen ───────────────────────────────────────────────
    let all_sources = stage_codegen(
        project_dir,
        manifest,
        profile,
        found.sources.clone(),
        &mut include_dirs,
        &deps.tool_paths,
        progress,
    )?;

    // ── Stage 8: Header units (build goal only) ──────────────────────────────
    let hu_flags = if matches!(config.goal, PipelineGoal::Build) {
        stage_header_units(project_dir, manifest, profile, &deps.include_dirs, &ctx)
    } else {
        vec![]
    };

    // ── Stage 9: PCH ─────────────────────────────────────────────────────────
    let pch = stage_pch(
        project_dir,
        &target_dir,
        manifest,
        profile,
        &include_dirs,
        &feat.defines,
        &ctx,
        progress,
    );

    // ── Stage 10: Compile ────────────────────────────────────────────────────
    let mut extra_flags = hu_flags;
    extra_flags.extend(pch.compile.iter().cloned());
    let compile_result = build_sources(
        project_dir,
        &target_dir,
        manifest,
        &ctx.effective_backend,
        profile,
        &all_sources,
        &include_dirs,
        &ctx.detected,
        &feat.defines,
        &extra_flags,
        progress,
    )?;

    // ── Goal phase ───────────────────────────────────────────────────────────
    match &config.goal {
        PipelineGoal::Build => {
            let link_result = link_targets(
                project_dir,
                &target_dir,
                manifest,
                &ctx.effective_backend,
                profile,
                &compile_result.objects,
                &ctx.detected,
                &ctx.templates,
                &deps.libs,
                &deps.raw_link_flags,
                progress,
            )?;

            // Write freight.lock.
            let lock = LockFile::generate(project_dir, manifest, &resolved);
            if let Err(e) = lock.save(project_dir) {
                progress(BuildEvent::Warning(format!(
                    "could not write freight.lock: {e}"
                )));
            }

            // Write compile_commands.json to .freight/lsp/<profile>/, merging dep databases.
            let cc = compile_commands::generate_incremental(
                project_dir,
                &target_dir,
                manifest,
                &ctx.effective_backend,
                &ctx.detected,
                profile,
                &all_sources,
                &include_dirs,
                &feat.defines,
                &pch.clangd,
                Some(compile_result.compiled_sources.as_slice()),
            );
            let cc = merge_dep_compile_commands(cc, &root_dir.join(".pkgs"), profile);
            let lsp_dir = lsp_compile_commands_dir(project_dir, profile);
            if let Err(e) = compile_commands::write_to(&lsp_dir.join("compile_commands.json"), &cc)
                .and_then(|_| {
                    compile_commands::write_incremental_cache(
                        project_dir,
                        manifest,
                        &ctx.effective_backend,
                        &ctx.detected,
                        profile,
                        &all_sources,
                        &include_dirs,
                        &feat.defines,
                        &pch.clangd,
                    )
                })
            {
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
            Ok(PipelineOutput::Build(BuildOutput {
                package_name: manifest.package.name.clone(),
                binaries,
                compiled: compile_result.compiled,
                skipped: compile_result.skipped,
            }))
        }

        PipelineGoal::Test { filter } => {
            let summary = run_test_goal(
                filter.as_deref(),
                project_dir,
                &target_dir,
                manifest,
                profile,
                &ctx,
                &compile_result,
                &deps,
                &include_dirs,
                &feat.defines,
                progress,
            )?;
            Ok(PipelineOutput::Test(summary))
        }

        PipelineGoal::Bench { filter } => {
            let summary = run_bench_goal(
                filter.as_deref(),
                project_dir,
                &target_dir,
                manifest,
                profile,
                &ctx,
                &compile_result,
                &deps,
                &include_dirs,
                &feat.defines,
                progress,
            )?;
            Ok(PipelineOutput::Bench(summary))
        }
    }
}

// ── Compile-commands merge helper ─────────────────────────────────────────────

fn merge_dep_compile_commands(
    base: Vec<compile_commands::CompileCommand>,
    pkgs_dir: &Path,
    profile: &str,
) -> Vec<compile_commands::CompileCommand> {
    let mut merged = base;
    let lsp_sub = std::path::Path::new(".freight")
        .join("lsp")
        .join(safe_lsp_profile_dir(profile));
    if let Ok(entries) = std::fs::read_dir(pkgs_dir) {
        for entry in entries.flatten() {
            let dep_cc = entry.path().join(&lsp_sub).join("compile_commands.json");
            if dep_cc.exists() {
                merged = compile_commands::merge(merged, compile_commands::load_from(&dep_cc));
            }
        }
    }
    merged
}
