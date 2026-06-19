use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{
    check_slot_conflicts, clean_project_at, emit_sources, generate_compile_commands_at, pipeline,
    resolve_dep_graph, run_pipeline_at, BenchSummary, BuildOutput, EmitTarget, PipelineOutput,
    ResolvedDep, TestSummary,
};
use crate::error::FreightError;
use crate::event::Progress;
use crate::install::{InstallOptions, InstallResult};
use crate::manifest::{
    find_manifest_dir, load_manifest, load_manifest_cached, load_workspace_manifest,
    types::Dependency, types::Manifest,
};

// ── Package source-dir enumeration ──────────────────────────────────────────

/// How a package source directory relates to the project being inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    /// The project itself, or a workspace member.
    Own,
    /// A `path = "…"` dependency.
    PathDep,
}

/// Enumerate the local package source directories of the project (or workspace)
/// rooted at `base`: the project itself, every workspace member, and every
/// `path` dependency (transitively, de-duplicated by canonical path). Each entry
/// is `(dir, kind, dep_key)` where `dir` holds the package's `freight.toml` and
/// `src/`.
///
/// Read-only and tolerant of missing/unfetched deps (skipped, never an error) —
/// the single shared enumerator behind the build's include collection and the
/// LSP's header/module indexes, so the dependency graph is walked in one place.
pub fn source_package_dirs(base: &Path) -> Vec<(PathBuf, PackageKind, Option<String>)> {
    let mut out: Vec<(PathBuf, PackageKind, Option<String>)> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let add_own = |out: &mut Vec<(PathBuf, PackageKind, Option<String>)>,
                   seen: &mut HashSet<PathBuf>,
                   dir: PathBuf| {
        if !dir.is_dir() {
            return;
        }
        let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if seen.insert(canon) {
            out.push((dir, PackageKind::Own, None));
        }
    };

    if let Some(ws) = load_workspace_manifest(base) {
        // Workspace root is not a package; each member is `Own`.
        for member in &ws.members {
            let member_dir = base.join(member.trim_end_matches('/'));
            add_own(&mut out, &mut seen, member_dir.clone());
            if let Ok(m) = load_manifest_cached(&member_dir) {
                collect_path_deps(&member_dir, &m, &mut out, &mut seen);
            }
        }
    } else {
        add_own(&mut out, &mut seen, base.to_path_buf());
        if let Ok(m) = load_manifest_cached(base) {
            collect_path_deps(base, &m, &mut out, &mut seen);
        }
    }
    out
}

/// Append the `path` dependencies (runtime + dev) of `manifest` rooted at
/// `project_dir`, de-duplicating by canonical path.
fn collect_path_deps(
    project_dir: &Path,
    manifest: &Manifest,
    out: &mut Vec<(PathBuf, PackageKind, Option<String>)>,
    seen: &mut HashSet<PathBuf>,
) {
    let deps = manifest.effective_dependencies().into_iter().chain(
        manifest
            .dev_dependencies
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    for (dep_key, dep) in deps {
        let Dependency::Detailed(detail) = dep else {
            continue;
        };
        let Some(rel_path) = detail.path else {
            continue;
        };
        let dep_dir = project_dir.join(&rel_path);
        if !dep_dir.is_dir() {
            continue;
        }
        let canon = dep_dir.canonicalize().unwrap_or_else(|_| dep_dir.clone());
        if seen.insert(canon) {
            out.push((dep_dir, PackageKind::PathDep, Some(dep_key)));
        }
    }
}

// ── Project ───────────────────────────────────────────────────────────────────

/// Handle to a freight project on disk.
///
/// Provides high-level methods (`build`, `test`, `bench`, `run`, `clean`, …)
/// that all funnel through the unified ten-stage pipeline (`run_pipeline_at`).
/// Construct with [`Project::open`] or [`Project::from_cwd`].
pub struct Project {
    /// Absolute path to the directory containing `freight.toml`.
    pub dir: PathBuf,
    /// Parsed and validated project manifest.
    pub manifest: Manifest,
    /// When building a dep from source, the root project's directory anchors
    /// the flat `.pkgs/` pool and target dirs.  `None` for top-level builds.
    pub parent_root: Option<PathBuf>,
    /// Resolved and topo-sorted dependency list.  Empty until [`Project::resolve`] is called.
    pub deps: Vec<ResolvedDep>,
}

impl Project {
    /// Load the project at `dir` (must contain a `freight.toml`).
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, FreightError> {
        let dir = dir.into();
        let manifest = load_manifest(&dir)?;
        Ok(Self {
            dir,
            manifest,
            parent_root: None,
            deps: vec![],
        })
    }

    /// Open the project whose `freight.toml` is an ancestor of the current
    /// working directory.
    pub fn from_cwd() -> Result<Self, FreightError> {
        let cwd = std::env::current_dir()?;
        let dir = find_manifest_dir(&cwd)
            .ok_or_else(|| FreightError::ManifestNotFound(cwd.to_string_lossy().into_owned()))?;
        Self::open(dir)
    }

    /// Attach a parent root dir so dep source-builds anchor to the root `.pkgs/` pool.
    pub fn with_parent_root(mut self, root: PathBuf) -> Self {
        self.parent_root = Some(root);
        self
    }

    /// Fetch missing deps and resolve the dependency graph, populating [`Project::deps`].
    ///
    /// Runs stages 2–4 of the pipeline (features → fetch → resolve) without
    /// compiling anything. Call this before inspecting `deps` or hand the
    /// `Project` to other tools that need the dep list.
    pub fn resolve(
        &mut self,
        config: &pipeline::PipelineConfig,
        progress: &Progress,
    ) -> Result<(), FreightError> {
        let root_dir = self.parent_root.as_deref().unwrap_or(&self.dir);
        let feat = pipeline::stage_features(&self.manifest, config)?;
        pipeline::stage_fetch(&self.dir, root_dir, &self.manifest, progress)?;
        let include_dev = config.goal.include_dev_deps();
        let raw = resolve_dep_graph(&self.dir, &self.manifest, include_dev, &feat.activated_deps)?;
        let drop = check_slot_conflicts(&raw, &self.manifest)?;
        self.deps = raw
            .into_iter()
            .filter(|d| !drop.contains(&d.name))
            .collect();
        Ok(())
    }

    /// Compile and link the project.
    pub fn build(
        &self,
        config: &pipeline::PipelineConfig,
        progress: &Progress,
    ) -> Result<BuildOutput, FreightError> {
        let cfg = pipeline::PipelineConfig {
            goal: pipeline::PipelineGoal::Build,
            ..config.clone()
        };
        match run_pipeline_at(&self.dir, &cfg, self.parent_root.as_deref(), progress)? {
            PipelineOutput::Build(out) => Ok(out),
            _ => unreachable!(),
        }
    }

    /// Compile and run test binaries found in `tests/`.
    pub fn test(
        &self,
        config: &pipeline::PipelineConfig,
        filter: Option<&str>,
        progress: &Progress,
    ) -> Result<TestSummary, FreightError> {
        let cfg = pipeline::PipelineConfig {
            goal: pipeline::PipelineGoal::Test {
                filter: filter.map(str::to_string),
            },
            ..config.clone()
        };
        match run_pipeline_at(&self.dir, &cfg, self.parent_root.as_deref(), progress)? {
            PipelineOutput::Test(out) => Ok(out),
            _ => unreachable!(),
        }
    }

    /// Compile and run benchmark binaries found in `benches/`.
    pub fn bench(
        &self,
        config: &pipeline::PipelineConfig,
        filter: Option<&str>,
        progress: &Progress,
    ) -> Result<BenchSummary, FreightError> {
        let cfg = pipeline::PipelineConfig {
            goal: pipeline::PipelineGoal::Bench {
                filter: filter.map(str::to_string),
            },
            ..config.clone()
        };
        match run_pipeline_at(&self.dir, &cfg, self.parent_root.as_deref(), progress)? {
            PipelineOutput::Bench(out) => Ok(out),
            _ => unreachable!(),
        }
    }

    /// Build then execute a binary target, passing `args` to it.
    ///
    /// `bin` selects which binary to run when the project has multiple
    /// `[[bin]]` targets.  When `None` the project must have exactly one.
    pub fn run(
        &self,
        config: &pipeline::PipelineConfig,
        bin: Option<&str>,
        args: &[String],
        progress: &Progress,
    ) -> Result<std::process::ExitStatus, FreightError> {
        let built = self.build(config, progress)?;

        let binary = match bin {
            Some(name) => built
                .binaries
                .iter()
                .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
                .cloned()
                .ok_or_else(|| FreightError::ManifestParse(format!("no binary named `{name}`")))?,
            None => match built.binaries.as_slice() {
                [] => {
                    return Err(FreightError::ManifestParse(
                        "no binary target produced — add a [[bin]] to freight.toml".into(),
                    ))
                }
                [b] => b.clone(),
                _ => built.binaries[0].clone(),
            },
        };

        std::process::Command::new(&binary)
            .args(args)
            .status()
            .map_err(FreightError::Io)
    }

    /// Emit intermediate output for all sources without linking.
    ///
    /// Writes files to `target/{profile}/{target}/` (e.g. `target/dev/asm/`).
    /// Returns the output directory path.
    pub fn emit(
        &self,
        target: EmitTarget,
        config: &pipeline::PipelineConfig,
        progress: &Progress,
    ) -> Result<PathBuf, FreightError> {
        let profile = &config.profile;
        let feat = pipeline::stage_features(&self.manifest, config)?;
        let ctx = super::load_project_at(&self.dir, profile)?;
        let target_dir = self.dir.join("target");
        emit_sources(
            &target,
            &self.dir,
            &target_dir,
            &self.manifest,
            &ctx.effective_backend,
            profile,
            &ctx.found.sources,
            &ctx.found.include_dirs,
            &ctx.detected,
            &feat.defines,
            progress,
        )
    }

    /// Build and install outputs to `opts.prefix`.
    pub fn install(
        &self,
        opts: &InstallOptions,
        progress: &Progress,
    ) -> Result<InstallResult, FreightError> {
        if !opts.no_build {
            let profile = if opts.release { "release" } else { "debug" };
            let config = pipeline::PipelineConfig {
                profile: profile.to_string(),
                use_defaults: true,
                target_override: opts.target.clone(),
                goal: pipeline::PipelineGoal::Build,
                ..Default::default()
            };
            self.build(&config, progress)?;
        }
        crate::install::install_project_built(&self.dir, &self.manifest, opts)
    }

    /// Build and create a distributable archive (`target/package/<name>-<ver>-<arch>-<os>.tar.gz`).
    ///
    /// Returns the path to the created archive.
    pub fn package(
        &self,
        release: bool,
        target_triple: Option<&str>,
        progress: &Progress,
    ) -> Result<PathBuf, FreightError> {
        let profile = if release { "release" } else { "debug" };
        let config = pipeline::PipelineConfig {
            profile: profile.to_string(),
            use_defaults: true,
            target_override: target_triple.map(str::to_string),
            goal: pipeline::PipelineGoal::Build,
            ..Default::default()
        };
        self.build(&config, progress)?;
        crate::install::package_project_built(&self.dir, &self.manifest, release, target_triple)
    }

    /// Remove build artifacts (`target/`).
    pub fn clean(&self) -> Result<(), FreightError> {
        clean_project_at(&self.dir)
    }

    /// Regenerate `compile_commands.json` for this project without building.
    pub fn generate_compile_commands(&self, profile: &str) -> Result<usize, FreightError> {
        generate_compile_commands_at(&self.dir, profile)
    }
}

#[cfg(test)]
mod tests {
    use super::{source_package_dirs, PackageKind};
    use std::fs;

    #[test]
    fn collects_project_and_path_deps() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\nvecmath = { path = \"vecmath\" }\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("vecmath")).unwrap();
        fs::write(
            root.join("vecmath/freight.toml"),
            "[package]\nname=\"vecmath\"\nversion=\"1.0.0\"\n",
        )
        .unwrap();

        let dirs = source_package_dirs(root);
        assert!(
            dirs.iter().any(|(d, k, _)| *k == PackageKind::Own
                && d.canonicalize().unwrap() == root.canonicalize().unwrap()),
            "project itself should be Own: {dirs:?}"
        );
        let dep = dirs
            .iter()
            .find(|(_, k, _)| *k == PackageKind::PathDep)
            .expect("path dep present");
        assert_eq!(dep.2.as_deref(), Some("vecmath"));
        assert!(dep.0.ends_with("vecmath"));
    }

    #[test]
    fn tolerates_missing_path_dep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\nghost = { path = \"nope\" }\n",
        )
        .unwrap();
        // Missing dep dir is skipped, not an error — only the project remains.
        let dirs = source_package_dirs(root);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].1, PackageKind::Own);
    }
}
