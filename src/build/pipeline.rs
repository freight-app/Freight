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
///  7. `header_units`  — precompile dep headers as BMIs (C++20 builds only)
///  8. `pch`           — compile precompiled header if configured
///  9. `compile`       — compile all project sources in parallel
/// 10. goal phase      — link (build), run tests, or run benchmarks
use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

use crate::build::{
    compile, compile_commands, discover, features, header_units, link_targets, link_test_binary,
    object_path, pch, plugin, BenchResult, BenchSummary, BuildOutput, CompileResult,
    PipelineOutput, SourceFile, TestSummary,
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
    /// Build example programs from `examples/` and `[[example]]` into
    /// `target/<profile>/examples/`. `filter` selects a single example by name.
    Examples {
        filter: Option<String>,
    },
}

impl PipelineGoal {
    pub fn include_dev_deps(&self) -> bool {
        matches!(self, Self::Test { .. })
    }

    /// Short name used for plugin `goals` activation filtering.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test { .. } => "test",
            Self::Bench { .. } => "bench",
            Self::Examples { .. } => "examples",
        }
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
    /// Active feature names — used to gate `[[bin]]` `required-features`.
    pub active: std::collections::BTreeSet<String>,
    pub defines: Vec<String>,
    pub activated_deps: std::collections::BTreeSet<String>,
    /// Defines forwarded into specific deps via `<dep>/define:NAME`, keyed by dep.
    pub dep_defines: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// This package's own exported defines (`[lib].defines` + active `pub-define:`).
    /// Already folded into `defines` for its own build; retained for inspection.
    pub public_defines: std::collections::BTreeSet<String>,
}

/// Aggregated dep output: static libs + include dirs + raw link flags + tool paths.
pub struct BuiltDepsOutput {
    pub libs: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub raw_link_flags: Vec<String>,
    pub tool_paths: Vec<PathBuf>,
    /// `[os.*] features` declared by dependencies — folded into the final link's
    /// system libraries so a dep's needs (e.g. pthread) are linked without the
    /// root re-declaring them.
    pub system_features: Vec<String>,
    /// Exported defines from deps (`[lib].defines` + active `pub-define:`), applied
    /// to the consumer's own compilation so it builds in the same configuration as
    /// the libraries it links.
    pub public_defines: Vec<String>,
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
        "debug" | "dev" => manifest
            .profile
            .debug
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
    // This package's OWN exported defines — always-on `[lib].defines` plus any
    // active `pub-define:` entries — also land in its own compilation. (Propagation
    // to dependents, when this package is itself a dep, is handled in
    // `build_resolved_deps`.)
    let mut public_defines = resolution.public_defines.clone();
    if let Some(lib) = manifest.lib.as_ref() {
        public_defines.extend(lib.defines.iter().cloned());
    }
    defines.extend(public_defines.iter().cloned());
    Ok(FeatureResolution {
        active: resolution.active,
        defines,
        activated_deps: resolution.activated_deps,
        dep_defines: resolution.dep_defines,
        public_defines,
    })
}

/// True when `--offline`/`--frozen` was requested (set by the CLI as an env var):
/// freight must not touch the network and instead use whatever is already in
/// `.pkgs/`. A missing dep then surfaces as a normal "run `freight fetch`" error.
pub fn is_offline() -> bool {
    crate::environment::Environment::offline()
}

/// True when `--locked`/`--frozen` was requested: freight.lock must already be
/// up to date and is never rewritten by the build.
pub fn is_locked() -> bool {
    crate::environment::Environment::locked()
}

/// Stage 3: fetch missing git + registry deps and verify the lock file.
pub fn stage_fetch(
    project_dir: &Path,
    root_dir: &Path,
    manifest: &Manifest,
    progress: &Progress,
) -> Result<Option<LockFile>, FreightError> {
    if !is_offline() {
        ensure_git_deps_fetched(project_dir, manifest, progress)?;
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

    // System-lib features each dependency declares (`[os.*] features`), unioned so
    // the final link picks them up without the root having to re-declare them.
    let mut system_features: Vec<String> = Vec::new();
    for d in &resolved {
        for f in d.manifest.system_features() {
            if !system_features.contains(&f) {
                system_features.push(f);
            }
        }
    }

    let mut output = BuiltDepsOutput {
        libs: built.libs,
        include_dirs: built.include_dirs,
        raw_link_flags: Vec::new(),
        tool_paths,
        system_features,
        public_defines: built.public_defines,
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

/// Stage 7: precompile dep headers as BMIs (C++20 builds only; no-op otherwise).
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

/// Stage 8: compile the precompiled header if configured; returns compile + clangd flags.
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
            &deps.system_features,
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
            &deps.system_features,
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

/// Collect example targets: every compilable file under `examples/` (name = file
/// stem) plus any declared `[[example]]` (declared name + src, overriding an
/// auto-discovered file with the same source). Optionally filtered by name and
/// gated by `required-features`.
fn collect_examples(
    project_dir: &Path,
    manifest: &Manifest,
    templates: &[crate::toolchain::CompilerTemplate],
    active_features: &std::collections::BTreeSet<String>,
    filter: Option<&str>,
) -> Vec<(String, SourceFile)> {
    let mut by_src: std::collections::BTreeMap<PathBuf, (String, SourceFile)> =
        std::collections::BTreeMap::new();

    // Auto-discovered files under examples/.
    for sf in discover_goal_sources(
        &project_dir.join("examples"),
        project_dir,
        manifest,
        templates,
        None,
        "example",
    ) {
        let name = sf
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("example")
            .to_string();
        by_src.insert(sf.path.clone(), (name, sf));
    }

    // Declared [[example]] sections override (custom name / required-features).
    let ext_map = discover::build_ext_map(manifest, templates);
    for ex in &manifest.examples {
        if !ex
            .required_features
            .iter()
            .all(|f| active_features.contains(f))
        {
            by_src.remove(Path::new(&ex.src));
            continue;
        }
        let rel = PathBuf::from(&ex.src);
        let ext = rel
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"));
        let Some(lang_key) = ext.and_then(|e| ext_map.get(e.as_str()).cloned()) else {
            continue;
        };
        by_src.insert(
            rel.clone(),
            (
                ex.name.clone(),
                SourceFile {
                    path: rel,
                    lang_key,
                },
            ),
        );
    }

    by_src
        .into_values()
        .filter(|(name, _)| filter.is_none_or(|f| f == name))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_examples_goal(
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
    active_features: &std::collections::BTreeSet<String>,
    progress: &Progress,
) -> Result<Vec<PathBuf>, FreightError> {
    let lib_objs =
        lib_objects_excluding_bins(manifest, target_dir, profile, &compile_result.objects);
    let examples = collect_examples(
        project_dir,
        manifest,
        &ctx.templates,
        active_features,
        filter,
    );
    if examples.is_empty() {
        return Ok(vec![]);
    }

    let srcs: Vec<SourceFile> = examples.iter().map(|(_, sf)| sf.clone()).collect();
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
        &[],
        progress,
    )?;

    let out_dir = target_dir.join(profile).join("examples");
    std::fs::create_dir_all(&out_dir)?;

    let mut outputs = Vec::new();
    for ((name, _), obj) in examples.iter().zip(compiled.objects.iter()) {
        let bin = out_dir.join(name);
        progress(BuildEvent::Linking { name: name.clone() });
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
            &deps.system_features,
            &deps.raw_link_flags,
        )?;
        outputs.push(bin);
    }
    Ok(outputs)
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
    let mut feat = stage_features(manifest, config)?;
    for w in manifest.cpu_tuning_warnings(profile) {
        progress(BuildEvent::Warning(w));
    }

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

    // Exported defines from dependencies (`[lib].defines` + their active
    // `pub-define:` entries) apply to this package's own compilation, so it builds
    // in the same configuration as the libraries it links (e.g. a consumer of the
    // spdlog port sees SPDLOG_COMPILED_LIB / SPDLOG_FMT_EXTERNAL without restating
    // them).
    for d in &deps.public_defines {
        if !feat.defines.contains(d) {
            feat.defines.push(d.clone());
        }
    }

    // ── Foreign self-build ───────────────────────────────────────────────────
    // A foreign package (`[package].build` set, no native targets — the
    // vcpkg-scraper port shape) is fetched + built with its own build system,
    // its deps' install prefixes fed in via CMAKE_PREFIX_PATH.
    if manifest.package.build.is_some() && manifest.bins.is_empty() && manifest.lib.is_none() {
        let prefixes: Vec<PathBuf> = deps
            .include_dirs
            .iter()
            .filter_map(|d| d.parent().map(|p| p.to_path_buf()))
            .collect();
        let libs = crate::adaptors::build_foreign_self(
            project_dir,
            &target_dir,
            manifest,
            profile,
            &prefixes,
            &deps.tool_paths,
            progress,
        )?;
        return Ok(PipelineOutput::Build(BuildOutput {
            package_name: manifest.package.name.clone(),
            binaries: vec![],
            compiled: libs.len(),
            skipped: 0,
        }));
    }

    // ── Stage 6: Assemble include dirs ───────────────────────────────────────
    let mut include_dirs = stage_assemble_includes(
        project_dir,
        manifest,
        profile,
        &found.include_dirs,
        &deps.include_dirs,
    );

    // ── Stage 6b: Build plugins (codegen) ────────────────────────────────────
    // Path-dep plugins that handle a declared section generate sources, include
    // dirs, and defines folded into the build below. Core-resolved deps' install
    // prefixes seed the plugins' `CFG.prefixes` (parent of each dep include dir,
    // same heuristic as the foreign-self path) so a foreign dep built by a plugin
    // can find_package an already-resolved freight dep.
    let seed_prefixes: Vec<PathBuf> = deps
        .include_dirs
        .iter()
        .filter_map(|d| d.parent().map(Path::to_path_buf))
        .collect();
    let plugin_out = plugin::run_plugins(
        project_dir,
        profile,
        config.goal.name(),
        &deps.tool_paths,
        &seed_prefixes,
        progress,
    )?;
    include_dirs.extend(plugin_out.include_dirs);
    feat.defines.extend(plugin_out.defines);
    let mut all_sources = found.sources.clone();
    all_sources.extend(plugin_out.sources);

    // ── Stage 7: Header units (build goal only) ──────────────────────────────
    let hu_flags = if matches!(config.goal, PipelineGoal::Build) {
        stage_header_units(project_dir, manifest, profile, &deps.include_dirs, &ctx)
    } else {
        vec![]
    };

    // ── Stage 8: PCH ─────────────────────────────────────────────────────────
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

    // ── Stage 9: Compile ────────────────────────────────────────────────────
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
        &plugin_out.tool_flags,
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
                &deps.system_features,
                &deps.raw_link_flags,
                &feat.active,
                &plugin_out.tool_flags,
                progress,
            )?;

            // Write freight.lock — or, under --locked, verify it without writing.
            let lock = LockFile::generate(project_dir, manifest, &resolved);
            if is_locked() {
                match LockFile::load(project_dir) {
                    Some(existing) if existing == lock => {}
                    Some(_) => {
                        return Err(FreightError::OptionError(
                            "freight.lock is out of date but --locked/--frozen was given; \
                             run `freight fetch` (or build without --locked) to update it"
                                .to_string(),
                        ));
                    }
                    None => {
                        return Err(FreightError::OptionError(
                            "freight.lock is missing but --locked/--frozen was given; \
                             run `freight fetch` (or build without --locked) to create it"
                                .to_string(),
                        ));
                    }
                }
            } else if let Err(e) = lock.save(project_dir) {
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

        PipelineGoal::Examples { filter } => {
            let binaries = run_examples_goal(
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
                &feat.active,
                progress,
            )?;
            Ok(PipelineOutput::Examples(BuildOutput {
                package_name: manifest.package.name.clone(),
                binaries,
                compiled: compile_result.compiled,
                skipped: compile_result.skipped,
            }))
        }
    }
}

// ── CMake transitive-dep build + export ───────────────────────────────────────

/// Provide a single CMake dependency **on demand** for the cmake plugin's
/// dependency provider — invoked as `freight cmake-provide <name>` from inside a
/// CMake configure (see `plugins/cmake/cmake.freight`). Returns an install prefix
/// to add to `CMAKE_PREFIX_PATH`, or `None` when freight provides nothing (the
/// dep is already on the host, or freight has no copy of it).
///
/// Resolution: already-installed (pkg-config / `<Name>Config.cmake`) → `None`
/// (the parent's CMake finds it). A freight package fetched under `.pkgs/<name>`
/// → built natively + wrapped in a generated `.pc` + `<Name>Config.cmake`. A
/// foreign CMake project under `.pkgs/<name>` → built via the cmake plugin (which
/// installs its own real config).
pub fn provide_cmake_package(
    cmake_name: &str,
    project_dir: &Path,
    profile: &str,
    progress: &Progress,
) -> Option<PathBuf> {
    use crate::resolve::cmake::{cmake_to_freight_name, is_installed_cmake_package};

    let freight_name = cmake_to_freight_name(cmake_name);
    // Already on the host → the parent's CMake finds it directly.
    if !crate::resolve::pkg_config::pkg_config_version(&freight_name).is_empty()
        || is_installed_cmake_package(cmake_name)
    {
        return None;
    }

    let target_dir = project_dir.join("target");
    let dep_dir = project_dir.join(".pkgs").join(&freight_name);

    if dep_dir.join("freight.toml").is_file() {
        provide_native(cmake_name, &freight_name, &dep_dir, project_dir, &target_dir, profile, progress)
    } else if dep_dir.join("CMakeLists.txt").is_file() {
        provide_foreign(cmake_name, &dep_dir, project_dir, &target_dir, profile, progress)
    } else {
        None
    }
}

/// A freight package fetched under `.pkgs/`: build it natively (sharing the root
/// `.pkgs` pool) and wrap it in a generated `.pc` + `<Name>Config.cmake`.
fn provide_native(
    cmake_name: &str,
    freight_name: &str,
    dep_dir: &Path,
    root_dir: &Path,
    target_dir: &Path,
    profile: &str,
    progress: &Progress,
) -> Option<PathBuf> {
    let cfg = PipelineConfig {
        profile: profile.to_string(),
        goal: PipelineGoal::Build,
        ..Default::default()
    };
    if let Err(e) = run_pipeline_at(dep_dir, &cfg, Some(root_dir), progress) {
        progress(BuildEvent::Warning(format!(
            "failed to build cmake dep '{cmake_name}': {e}"
        )));
        return None;
    }

    let pkg_name = crate::manifest::load_manifest(dep_dir)
        .map(|m| m.package.name)
        .unwrap_or_else(|_| freight_name.to_string());
    let version = crate::manifest::load_manifest(dep_dir)
        .map(|m| m.package.version)
        .unwrap_or_default();
    let lib_out = root_dir.join("target").join("deps").join(&pkg_name).join(profile);
    let mut libs = Vec::new();
    for ext in ["a", "so", "dylib"] {
        if let Ok(paths) = glob::glob(&lib_out.join(format!("*.{ext}")).to_string_lossy()) {
            libs.extend(paths.flatten());
        }
    }
    let includes: Vec<PathBuf> = [dep_dir.join("include")]
        .into_iter()
        .filter(|p| p.is_dir())
        .collect();
    if libs.is_empty() && includes.is_empty() {
        return None;
    }

    let prefix = target_dir.join("cmake-export").join(cmake_name);
    let spec = crate::build::cmake_export::ExportSpec {
        cmake_name,
        pc_name: freight_name,
        version: &version,
    };
    if let Err(e) =
        crate::build::cmake_export::assemble_export_prefix(&prefix, &includes, &libs, &spec)
    {
        progress(BuildEvent::Warning(format!(
            "failed to export cmake dep '{cmake_name}': {e}"
        )));
        return None;
    }
    Some(prefix)
}

/// A foreign CMake project fetched under `.pkgs/`: build it via the cmake plugin
/// (which configures, builds, and runs its own `install`). Returns its install
/// prefix (with the project's real `<Name>Config.cmake`).
fn provide_foreign(
    cmake_name: &str,
    dep_dir: &Path,
    root_dir: &Path,
    target_dir: &Path,
    profile: &str,
    progress: &Progress,
) -> Option<PathBuf> {
    let out_dir = target_dir.join("cmake-source").join(cmake_name);
    match crate::build::plugin::run_build_system(
        "cmake", cmake_name, dep_dir, &out_dir, root_dir, profile, &[], &[], &[], progress,
    ) {
        Ok(built) => built
            .include_dirs
            .iter()
            .find_map(|inc| inc.parent().map(Path::to_path_buf)),
        Err(e) => {
            progress(BuildEvent::Warning(format!(
                "failed to build source dep '{cmake_name}': {e}"
            )));
            None
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

#[cfg(test)]
mod example_tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    #[test]
    fn collect_examples_auto_and_declared() {
        let dir = tempfile::tempdir().unwrap();
        let ex = dir.path().join("examples");
        fs::create_dir_all(&ex).unwrap();
        fs::write(ex.join("a.c"), "int main(){return 0;}").unwrap();
        fs::write(ex.join("b.c"), "int main(){return 0;}").unwrap();

        // Declared [[example]] renames a.c to "custom"; b.c stays auto-discovered.
        let manifest = crate::manifest::load_manifest_str(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.c]\n\
             [lib]\ntype=\"static\"\nsrcs=[\"src/lib.c\"]\n\
             [[example]]\nname=\"custom\"\nsrc=\"examples/a.c\"\n",
        )
        .unwrap();
        let templates = crate::toolchain::load_all_templates();
        let active = BTreeSet::new();

        let all = collect_examples(dir.path(), &manifest, &templates, &active, None);
        let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"custom"),
            "declared name present: {names:?}"
        );
        assert!(names.contains(&"b"), "auto name present: {names:?}");
        assert!(!names.contains(&"a"), "a.c renamed to custom: {names:?}");

        // Filter selects a single example by name.
        let only = collect_examples(dir.path(), &manifest, &templates, &active, Some("b"));
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].0, "b");
    }

    #[test]
    fn collect_examples_gates_on_required_features() {
        let dir = tempfile::tempdir().unwrap();
        let ex = dir.path().join("examples");
        fs::create_dir_all(&ex).unwrap();
        fs::write(ex.join("gated.c"), "int main(){return 0;}").unwrap();

        let manifest = crate::manifest::load_manifest_str(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.c]\n[features]\nextra=[]\n\
             [lib]\ntype=\"static\"\nsrcs=[\"src/lib.c\"]\n\
             [[example]]\nname=\"gated\"\nsrc=\"examples/gated.c\"\nrequired-features=[\"extra\"]\n",
        )
        .unwrap();
        let templates = crate::toolchain::load_all_templates();

        let off = collect_examples(dir.path(), &manifest, &templates, &BTreeSet::new(), None);
        assert!(
            off.is_empty(),
            "gated example excluded without feature: {off:?}"
        );

        let mut active = BTreeSet::new();
        active.insert("extra".to_string());
        let on = collect_examples(dir.path(), &manifest, &templates, &active, None);
        assert_eq!(on.len(), 1, "gated example included with feature");
    }
}
