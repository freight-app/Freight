use std::path::PathBuf;

use crate::error::FreightError;
use crate::event::Progress;
use crate::manifest::{find_manifest_dir, load_manifest, types::Manifest};
use super::{
    clean_project_at, generate_compile_commands_at, pipeline, run_pipeline_at, BenchSummary,
    BuildOutput, PipelineOutput, TestSummary,
};

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
}

impl Project {
    /// Load the project at `dir` (must contain a `freight.toml`).
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, FreightError> {
        let dir = dir.into();
        let manifest = load_manifest(&dir)?;
        Ok(Self { dir, manifest, parent_root: None })
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
                [] => return Err(FreightError::ManifestParse(
                    "no binary target produced — add a [[bin]] to freight.toml".into(),
                )),
                [b] => b.clone(),
                _ => built.binaries[0].clone(),
            },
        };

        std::process::Command::new(&binary)
            .args(args)
            .status()
            .map_err(FreightError::Io)
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
