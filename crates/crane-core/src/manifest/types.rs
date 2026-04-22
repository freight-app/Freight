use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::toolchain::template::BuildSettings;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub package: Package,
    /// Language sections keyed by identifier, e.g. `[language.cpp]`, `[language.fortran]`.
    /// Valid keys: `c`, `cpp`, `fortran`, `ada`, `d`, `cuda`.
    #[serde(default)]
    pub language: HashMap<String, LanguageSettings>,
    #[serde(rename = "lib", default)]
    pub lib: Option<LibTarget>,
    #[serde(rename = "bin", default)]
    pub bins: Vec<BinTarget>,
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
    #[serde(rename = "dev-dependencies", default)]
    pub dev_dependencies: HashMap<String, Dependency>,
    #[serde(default)]
    pub compiler: CompilerConfig,
    #[serde(default)]
    pub profile: Profiles,
    #[serde(default)]
    pub features: HashMap<String, Vec<String>>,
}

impl Manifest {
    /// Produce `BuildSettings` for the named profile (`"dev"` or `"release"`),
    /// starting from the base `[compiler]` settings and applying profile overrides.
    pub fn build_settings_for(&self, profile_name: &str) -> BuildSettings {
        let base = BuildSettings {
            opt_level: self.compiler.opt_level.to_string(),
            debug: self.compiler.debug,
            warnings: self.compiler.warnings.clone(),
            standard: if self.language.len() == 1 {
                self.language.values().next().and_then(|l| l.std.clone())
            } else {
                None // mixed-language: standard resolved per source file in Phase 4
            },
            defines: self.compiler.defines.clone(),
            include_paths: self
                .compiler
                .includes
                .paths
                .iter()
                .map(PathBuf::from)
                .collect(),
            extra_flags: self.compiler.flags.clone(),
            target_triple: self.compiler.target.clone(),
            sysroot: self.compiler.sysroot.as_deref().map(PathBuf::from),
            ..Default::default()
        };

        let profile = match profile_name {
            "dev" => self.profile.dev.as_ref(),
            "release" => self.profile.release.as_ref(),
            _ => None,
        };

        let Some(p) = profile else { return base };

        BuildSettings {
            opt_level: p.opt_level.map(|v| v.to_string()).unwrap_or(base.opt_level),
            debug: p.debug.unwrap_or(base.debug),
            lto: p.lto.unwrap_or(base.lto),
            strip: p.strip.unwrap_or(base.strip),
            sanitize: if p.sanitize.is_empty() {
                base.sanitize
            } else {
                p.sanitize.clone()
            },
            ..base
        }
    }
}

// ── Package ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub readme: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

// ── Language ──────────────────────────────────────────────────────────────────

/// Settings for one language entry under `[language.<key>]`.
/// The key (e.g. `cpp`, `fortran`) is the language identifier.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LanguageSettings {
    #[serde(default)]
    pub std: Option<String>,
}

// ── Targets ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LibTarget {
    #[serde(rename = "type", default)]
    pub lib_type: LibType,
    pub src: String,
    #[serde(default)]
    pub include: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LibType {
    #[default]
    Static,
    Shared,
    HeaderOnly,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BinTarget {
    pub name: String,
    pub src: String,
}

// ── Dependencies ──────────────────────────────────────────────────────────────

/// A dependency can be either a bare version string or a detailed table.
///
/// ```toml
/// libfoo   = "0.3"                          # Simple
/// openssl  = { system = "openssl", version = ">=3.0" }  # Detailed
/// myutils  = { path = "../myutils" }        # Detailed
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Dependency {
    Simple(String),
    Detailed(DetailedDep),
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DetailedDep {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub optional: bool,
    /// Target triples this prebuilt dep is compatible with (e.g. `["x86_64-linux-gnu"]`).
    /// `None` (absent) means compatible with all targets. Source deps ignore this field.
    /// Reserved for the cross-compilation phase.
    #[serde(default)]
    pub targets: Option<Vec<String>>,
}

// ── Compiler config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompilerConfig {
    #[serde(default)]
    pub backend: Backend,
    #[serde(rename = "opt-level", default = "default_opt_level")]
    pub opt_level: u8,
    #[serde(default)]
    pub debug: bool,
    #[serde(default = "default_warnings")]
    pub warnings: String,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub includes: CompilerIncludes,
    #[serde(default)]
    pub overrides: HashMap<String, String>,
    /// Cross-compilation target triple (e.g. `"aarch64-linux-gnu"`).
    /// `None` means native/host build. Reserved for the cross-compilation phase.
    #[serde(default)]
    pub target: Option<String>,
    /// Path to the target sysroot. Reserved for the cross-compilation phase.
    #[serde(default)]
    pub sysroot: Option<String>,
}

impl Default for CompilerConfig {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            opt_level: default_opt_level(),
            debug: false,
            warnings: default_warnings(),
            defines: vec![],
            flags: vec![],
            includes: CompilerIncludes::default(),
            overrides: HashMap::default(),
            target: None,
            sysroot: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CompilerIncludes {
    #[serde(default)]
    pub paths: Vec<String>,
}

/// The compiler backend name from `[compiler] backend = "..."`.
/// Stored as a plain string so user-added templates are supported without a Rust change.
/// Special value `"auto"` (the default) picks the first available compiler for each language.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Backend(pub String);

impl Default for Backend {
    fn default() -> Self { Self("auto".into()) }
}

impl Backend {
    pub fn is_auto(&self) -> bool { self.0.eq_ignore_ascii_case("auto") }
    pub fn name(&self) -> &str { &self.0 }
}

fn default_opt_level() -> u8 { 2 }
fn default_warnings() -> String { "all".to_string() }

// ── Profiles ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Profiles {
    #[serde(default)]
    pub dev: Option<Profile>,
    #[serde(default)]
    pub release: Option<Profile>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Profile {
    #[serde(rename = "opt-level", default)]
    pub opt_level: Option<u8>,
    #[serde(default)]
    pub debug: Option<bool>,
    #[serde(default)]
    pub lto: Option<bool>,
    #[serde(default)]
    pub strip: Option<bool>,
    #[serde(default)]
    pub sanitize: Vec<String>,
}
