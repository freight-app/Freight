//! `Environment` — the host and target environment a build runs in.
//!
//! Where [`crate::project::Project`] models *what* is being built (the manifest,
//! its packages, and the dependency graph), `Environment` models *where and how*:
//! the host machine, the (optionally cross-compilation) target, the configured
//! toolchain/sysroot, and the degree of parallelism.
//!
//! It consolidates facts that were previously read ad hoc from
//! `std::env::consts`, [`GlobalConfig`], and triple parsing, so build, install,
//! and toolchain code can take a single resolved value instead of re-deriving it.

use std::path::{Path, PathBuf};

use crate::manifest::types::Manifest;
use crate::toolchain::GlobalConfig;
use crate::vendor::resolve_target;

/// The resolved host + target environment for a build.
#[derive(Debug, Clone)]
pub struct Environment {
    /// Host operating system (`std::env::consts::OS`): `linux`, `macos`, `windows`, …
    pub host_os: String,
    /// Host CPU architecture (`std::env::consts::ARCH`): `x86_64`, `aarch64`, …
    pub host_arch: String,
    /// Cross-compilation target triple (e.g. `aarch64-linux-gnu`). `None` = native.
    pub target_triple: Option<String>,
    /// Effective target OS — parsed from the triple, else the host OS.
    pub target_os: String,
    /// Effective target CPU architecture — parsed from the triple, else the host.
    pub target_arch: String,
    /// Cross-compilation sysroot, if configured.
    pub sysroot: Option<PathBuf>,
    /// Default compiler backend (`gcc`/`clang`/…); `None` = auto-detect per language.
    pub default_backend: Option<String>,
    /// Default debugger backend (`gdb`/`lldb`/…); `None` = auto-detect.
    pub default_debugger: Option<String>,
    /// Whether CPU tuning flags are derived from the target/sysroot. Defaults to
    /// `true` when unset in config (matching the build's behavior).
    pub auto_cpu_tuning: bool,
    /// Number of parallel build jobs.
    pub jobs: usize,
}

impl Environment {
    /// Detect the environment from host facts and the merged global config
    /// (`/etc/freight/config.toml` then `~/.freight/config.toml`).
    pub fn detect() -> Self {
        Self::from_config(GlobalConfig::load(), None, None)
    }

    /// Build the environment from an explicit config plus optional overrides for
    /// the target triple and sysroot (e.g. from `--target` / `--sysroot`); the
    /// overrides win over the config.
    pub fn from_config(
        config: GlobalConfig,
        target_override: Option<String>,
        sysroot_override: Option<PathBuf>,
    ) -> Self {
        let host_os = std::env::consts::OS.to_string();
        let host_arch = std::env::consts::ARCH.to_string();

        let target_triple = target_override.or(config.target);
        let (target_arch, target_os) = resolve_target(target_triple.as_deref());
        let sysroot = sysroot_override.or_else(|| config.sysroot.map(PathBuf::from));

        Self {
            host_os,
            host_arch,
            target_triple,
            target_os,
            target_arch,
            sysroot,
            default_backend: config.default_backend,
            default_debugger: config.default_debugger,
            auto_cpu_tuning: config.auto_cpu_tuning.unwrap_or(true),
            jobs: default_jobs(),
        }
    }

    /// Resolve the environment for a project: the merged config layers
    /// (`/etc/freight`, `~/.freight`, then `<project>/.freight/config.toml`) with
    /// the `FREIGHT_SYSROOT` env override applied. The single place the build,
    /// dep-resolution, and LSP paths derive machine-local target/sysroot.
    pub fn for_project(project_dir: &Path) -> Self {
        let mut config = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(project_dir) {
            config.apply_local(local);
        }
        let sysroot_override = std::env::var_os("FREIGHT_SYSROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self::from_config(config, None, sysroot_override)
    }

    /// Apply this environment's machine-local target/sysroot/CPU-tuning onto a
    /// manifest. These fields are `#[serde(skip)]`, so a freshly-parsed manifest
    /// never carries them — every consumer (build, dep fetch, LSP) sets them from
    /// the resolved environment, and this is that single setter.
    pub fn apply_to_manifest(&self, manifest: &mut Manifest) {
        manifest.compiler.target = self.target_triple.clone();
        manifest.compiler.sysroot = self
            .sysroot
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        manifest.compiler.auto_cpu_tuning = self.auto_cpu_tuning;
    }

    /// Whether the build targets a different OS/arch than the host.
    pub fn is_cross(&self) -> bool {
        self.target_triple.is_some()
            && (self.target_os != self.host_os || self.target_arch != self.host_arch)
    }

    /// The target triple, or a synthesized `<arch>-<os>` descriptor when native.
    pub fn target(&self) -> String {
        self.target_triple
            .clone()
            .unwrap_or_else(|| format!("{}-{}", self.target_arch, self.target_os))
    }

    /// Override the job count (e.g. from `--jobs N`).
    pub fn with_jobs(mut self, jobs: usize) -> Self {
        self.jobs = jobs;
        self
    }

    // ── Session flags ──────────────────────────────────────────────────────
    //
    // Process-wide build-session toggles, carried as `FREIGHT_*` env vars so
    // they reach deep pipeline code without threading an `Environment` through
    // every call. The CLI sets them once at startup via [`set_session_flags`];
    // library code reads them through these accessors, so the variable names
    // live in exactly one place.

    /// Whether verbose command echoing is enabled (`FREIGHT_VERBOSE`).
    pub fn verbose() -> bool {
        flag("FREIGHT_VERBOSE")
    }

    /// Whether the build is offline — no network access (`FREIGHT_OFFLINE`).
    pub fn offline() -> bool {
        flag("FREIGHT_OFFLINE")
    }

    /// Whether the lockfile must not be rewritten (`FREIGHT_LOCKED`).
    pub fn locked() -> bool {
        flag("FREIGHT_LOCKED")
    }

    /// Set the process-wide session flags (called once by the CLI before
    /// building). Only ever *sets* — never clears — so an externally-exported
    /// `FREIGHT_*` variable is still honored when the flag is false.
    pub fn set_session_flags(verbose: bool, offline: bool, locked: bool) {
        if verbose {
            std::env::set_var("FREIGHT_VERBOSE", "1");
        }
        if offline {
            std::env::set_var("FREIGHT_OFFLINE", "1");
        }
        if locked {
            std::env::set_var("FREIGHT_LOCKED", "1");
        }
    }
}

fn flag(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

/// Default parallelism: `min(available_parallelism, 6)`. The single source of
/// truth for the default job count (used by `Environment` and the CLI's
/// `--jobs` handling).
pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_environment_uses_host_facts() {
        let env = Environment::from_config(GlobalConfig::default(), None, None);
        assert_eq!(env.host_os, std::env::consts::OS);
        assert_eq!(env.target_os, env.host_os);
        assert_eq!(env.target_arch, env.host_arch);
        assert!(env.target_triple.is_none());
        assert!(!env.is_cross());
        assert!(env.jobs >= 1);
    }

    #[test]
    fn target_override_parses_triple_and_marks_cross() {
        let env = Environment::from_config(
            GlobalConfig::default(),
            Some("aarch64-linux-gnu".to_string()),
            Some(PathBuf::from("/opt/sysroots/aarch64")),
        );
        assert_eq!(env.target_arch, "aarch64");
        assert_eq!(env.target_os, "linux");
        assert_eq!(env.target(), "aarch64-linux-gnu");
        assert_eq!(env.sysroot, Some(PathBuf::from("/opt/sysroots/aarch64")));
        // Cross only when the target differs from the host.
        let host_aarch64_linux =
            std::env::consts::ARCH == "aarch64" && std::env::consts::OS == "linux";
        assert_eq!(env.is_cross(), !host_aarch64_linux);
    }

    #[test]
    fn apply_to_manifest_sets_machine_local_fields() {
        let mut m: Manifest =
            toml::from_str("[package]\nname = \"x\"\nversion = \"1\"\n").expect("parse manifest");
        let config = GlobalConfig {
            target: Some("aarch64-linux-gnu".to_string()),
            sysroot: Some("/opt/sysroots/aarch64".to_string()),
            auto_cpu_tuning: Some(false),
            ..GlobalConfig::default()
        };
        let env = Environment::from_config(config, None, None);
        env.apply_to_manifest(&mut m);
        assert_eq!(m.compiler.target.as_deref(), Some("aarch64-linux-gnu"));
        assert_eq!(m.compiler.sysroot.as_deref(), Some("/opt/sysroots/aarch64"));
        assert!(!m.compiler.auto_cpu_tuning);
    }

    #[test]
    fn override_wins_over_config() {
        let config = GlobalConfig {
            target: Some("x86_64-linux-gnu".to_string()),
            ..GlobalConfig::default()
        };
        let env = Environment::from_config(config, Some("riscv64-linux-gnu".to_string()), None);
        assert_eq!(env.target_arch, "riscv64");
    }
}
