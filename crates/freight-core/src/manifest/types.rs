use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::toolchain::template::BuildSettings;

// ── Workspace ─────────────────────────────────────────────────────────────────

/// The `[workspace]` section of a workspace-root `freight.toml`.
///
/// A workspace root has **only** this section — no `[package]`. Members are
/// ordinary freight projects whose own `freight.toml` files contain `[package]`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct WorkspaceSection {
    /// Relative paths to member directories (e.g. `["app/", "libfoo/"]`).
    pub members: Vec<String>,
}

/// Thin deserialisation target for workspace-root manifests.
#[derive(Debug, Deserialize)]
pub(crate) struct WorkspaceToml {
    pub workspace: WorkspaceSection,
}

// ── Project manifest ──────────────────────────────────────────────────────────

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
    /// Assembly / CPU target configuration: arch override and CPU extensions.
    #[serde(default)]
    pub target: TargetConfig,
    /// Formatter requirements for this project (`[formatter]`).
    #[serde(default)]
    pub formatter: FormatterConfig,
    /// Linter requirements for this project (`[linter]`).
    #[serde(default)]
    pub linter: LinterConfig,
    /// Per-platform overlays keyed by OS name (`linux`, `windows`, `macos`,
    /// `freebsd`, …) or family alias (`unix`, `bsd`). Matching overlays are
    /// merged into the base config in a defined order (see [`host_platforms`])
    /// so users can declare e.g. `[platform.windows]` deps that are only
    /// linked on Windows builds.
    #[serde(default)]
    pub platform: HashMap<String, PlatformOverlay>,
}

impl Manifest {
    /// Produce `BuildSettings` for the named profile (`"dev"` or `"release"`),
    /// starting from the base `[compiler]` settings and applying profile and
    /// platform overrides.
    pub fn build_settings_for(&self, profile_name: &str) -> BuildSettings {
        let mut defines = self.compiler.defines.clone();
        let mut flags = self.compiler.flags.clone();
        let mut include_paths: Vec<PathBuf> = self
            .compiler
            .includes
            .paths
            .iter()
            .map(PathBuf::from)
            .collect();

        // Apply matching platform overlays in family-then-specific order so a
        // Linux build picks up `[platform.unix]` first then `[platform.linux]`.
        // Lookup is case-insensitive against the manifest keys.
        for plat in host_platforms() {
            if let Some(ov) = self.platform_overlay(plat) {
                merge_string_vec(&mut defines, &ov.compiler.defines);
                merge_string_vec(&mut flags, &ov.compiler.flags);
                for p in &ov.compiler.includes.paths {
                    let buf = PathBuf::from(p);
                    if !include_paths.contains(&buf) {
                        include_paths.push(buf);
                    }
                }
            }
        }

        let base = BuildSettings {
            opt_level: self.compiler.opt_level.to_string(),
            debug: self.compiler.debug,
            warnings: self.compiler.warnings.clone(),
            standard: if self.language.len() == 1 {
                self.language.values().next().and_then(|l| l.std.clone())
            } else {
                None // mixed-language: standard resolved per source file in Phase 4
            },
            defines,
            include_paths,
            extra_flags: flags,
            target_triple: self.compiler.target.clone(),
            sysroot: self.compiler.sysroot.as_deref().map(PathBuf::from),
            arch: self.target.arch.clone()
                .unwrap_or_else(|| std::env::consts::ARCH.to_string()),
            cpu_extensions: self.target.cpu_extensions.clone(),
            ..Default::default()
        };

        let resolved = self.resolve_profile(profile_name);
        let Some(p) = resolved else { return base };

        BuildSettings {
            opt_level: p.opt_level.map(|v| v.to_string()).unwrap_or(base.opt_level),
            debug: p.debug.unwrap_or(base.debug),
            lto: p.lto.unwrap_or(base.lto),
            strip: p.strip.unwrap_or(base.strip),
            sanitize: if p.sanitize.is_empty() { base.sanitize } else { p.sanitize },
            ..base
        }
    }

    /// Walk the `inherits` chain for `name` and return a merged `Profile`.
    ///
    /// Resolution order: leaf overrides parent where `Some`/non-empty; the root
    /// (deepest ancestor) provides defaults. Cycles are broken at the first
    /// repeated name; the chain is capped at 16 hops.
    fn resolve_profile(&self, name: &str) -> Option<Profile> {
        let mut visited: Vec<String> = Vec::new();
        let mut current_name = name.to_string();
        // Accumulate layers from leaf to root, then merge root-first.
        let mut chain: Vec<Profile> = Vec::new();
        loop {
            if visited.len() >= 16 { break; } // max-hop guard
            if visited.contains(&current_name) { break; } // cycle guard
            visited.push(current_name.clone());
            let p = match current_name.as_str() {
                "dev"     => self.profile.dev.clone()?,
                "release" => self.profile.release.clone()?,
                _         => self.profile.custom.get(&current_name).cloned()?,
            };
            let next = p.inherits.clone();
            chain.push(p);
            match next {
                Some(parent) => current_name = parent,
                None => break,
            }
        }
        // Merge: last in chain is the root (no inherits), first is the leaf.
        // Root provides defaults; each child overrides.
        let mut merged = chain.pop()?; // root
        for child in chain.into_iter().rev() {
            if child.opt_level.is_some() { merged.opt_level = child.opt_level; }
            if child.debug.is_some()     { merged.debug     = child.debug; }
            if child.lto.is_some()       { merged.lto       = child.lto; }
            if child.strip.is_some()     { merged.strip      = child.strip; }
            if !child.sanitize.is_empty() { merged.sanitize  = child.sanitize; }
            if !child.features.is_empty() {
                for f in child.features {
                    if !merged.features.contains(&f) { merged.features.push(f); }
                }
            }
        }
        Some(merged)
    }

    /// Iterate over `(name, dep)` pairs for the base `[dependencies]` plus any
    /// `[platform.X.dependencies]` whose `X` matches the host. A platform
    /// overlay can shadow a base dep with the same key — common when a package
    /// links a different system library on Windows vs Linux.
    ///
    /// Deps are filtered by three optional fields on the dep itself:
    /// - `targets`: cross-compilation triple allowlist (see `[compiler] target`)
    /// - `os`: host OS allowlist; supports family aliases like `"unix"`
    /// - `arch`: host CPU architecture allowlist (e.g. `"x86_64"`)
    /// All absent fields are unconditional.
    pub fn effective_dependencies(&self) -> HashMap<String, Dependency> {
        let current_target = self.compiler.target.as_deref();
        let mut out: HashMap<String, Dependency> = self.dependencies.iter()
            .filter(|(_, dep)| dep_matches_env(dep, current_target))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for plat in host_platforms() {
            if let Some(ov) = self.platform_overlay(plat) {
                for (name, dep) in &ov.dependencies {
                    if dep_matches_env(dep, current_target) {
                        out.insert(name.clone(), dep.clone());
                    }
                }
            }
        }
        out
    }

    /// Return the effective `LanguageSettings` for `lang_key`, applying any matching
    /// platform overlays on top of the base `[language.<key>]` section.
    /// Platform overlay fields win over base; `None` means no override.
    pub fn effective_language_settings(&self, lang_key: &str) -> LanguageSettings {
        let mut s = self.language.get(lang_key).cloned().unwrap_or_default();
        for plat in host_platforms() {
            if let Some(ov) = self.platform_overlay(plat) {
                if let Some(lang_ov) = ov.language.get(lang_key) {
                    if lang_ov.std.is_some()    { s.std    = lang_ov.std.clone(); }
                    if lang_ov.stdlib.is_some() { s.stdlib = lang_ov.stdlib.clone(); }
                }
            }
        }
        s
    }

    /// Case-insensitive lookup of a platform overlay.
    fn platform_overlay(&self, name: &str) -> Option<&PlatformOverlay> {
        self.platform
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }
}

/// Returns `true` if the dependency should be included given the current build environment.
///
/// Checks three optional filter fields on a `DetailedDep`:
/// - `targets`: only included when `current_target` matches; absent = unconditional.
///   A `None` current target (native build) never satisfies a present `targets` list.
/// - `os`: only included when the host OS (or a family alias from `host_platforms()`)
///   appears in the list; absent = unconditional.
/// - `arch`: only included when `std::env::consts::ARCH` appears in the list;
///   absent = unconditional.
fn dep_matches_env(dep: &Dependency, current_target: Option<&str>) -> bool {
    let Dependency::Detailed(d) = dep else { return true };

    if let Some(targets) = &d.targets {
        let ok = match current_target {
            Some(t) => targets.iter().any(|wanted| wanted == t),
            None => false,
        };
        if !ok { return false; }
    }

    if let Some(os_req) = &d.os {
        let host_plats = host_platforms();
        let ok = os_req.iter().any(|req| {
            host_plats.iter().any(|p| p.eq_ignore_ascii_case(req.as_str()))
        });
        if !ok { return false; }
    }

    if let Some(arch_req) = &d.arch {
        let host_arch = std::env::consts::ARCH;
        let ok = arch_req.iter().any(|req| req.eq_ignore_ascii_case(host_arch));
        if !ok { return false; }
    }

    true
}

fn merge_string_vec(into: &mut Vec<String>, items: &[String]) {
    for item in items {
        if !into.iter().any(|x| x == item) {
            into.push(item.clone());
        }
    }
}

/// Platform names that match the current host, ordered family-first so
/// specific overlays win. On Linux this returns `["unix", "linux"]`; on
/// Windows just `["windows"]`; on FreeBSD `["unix", "bsd", "freebsd"]`.
pub fn host_platforms() -> Vec<&'static str> {
    let os = std::env::consts::OS;
    let mut chain = Vec::new();
    let unix = matches!(
        os,
        "linux" | "macos" | "freebsd" | "openbsd" | "netbsd" | "dragonfly"
            | "android" | "ios" | "solaris" | "illumos"
    );
    let bsd = matches!(os, "freebsd" | "openbsd" | "netbsd" | "dragonfly");
    if unix {
        chain.push("unix");
    }
    if bsd {
        chain.push("bsd");
    }
    chain.push(match os {
        // Map a few rust os names back to the friendlier freight keys.
        other => other,
    });
    chain
}

/// Set of platform / family names freight recognizes in `[platform.X]` keys and
/// dep `os` fields. Used by validation.
pub fn known_platform_keys() -> &'static [&'static str] {
    &[
        "unix", "bsd",
        "linux", "windows", "macos",
        "freebsd", "openbsd", "netbsd", "dragonfly",
        "android", "ios", "solaris", "illumos",
    ]
}

/// Set of CPU architecture names accepted in dep `arch` fields.
/// Values mirror `std::env::consts::ARCH` plus common aliases.
pub fn known_arch_keys() -> &'static [&'static str] {
    &[
        "x86_64", "x86",
        "aarch64", "arm",
        "mips", "mips64",
        "powerpc", "powerpc64",
        "riscv64",
        "s390x",
        "sparc64",
        "wasm32", "wasm64",
    ]
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
    /// Virtual slots this package fills (e.g. `["blas"]`, `["cxx-stdlib"]`).
    /// If two active deps declare the same slot, freight errors before compilation.
    #[serde(default)]
    pub provides: Vec<String>,
}

// ── Language ──────────────────────────────────────────────────────────────────

/// Settings for one language entry under `[language.<key>]`.
/// The key (e.g. `cpp`, `fortran`) is the language identifier.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LanguageSettings {
    #[serde(default)]
    pub std: Option<String>,
    /// C++ standard library selection: `"libc++"` | `"libstdc++"` | `"none"`.
    /// Only meaningful for `[language.cpp]`. Defaults to the toolchain's built-in choice.
    #[serde(default)]
    pub stdlib: Option<String>,
}

// ── Targets ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LibTarget {
    #[serde(rename = "type", default)]
    pub lib_type: LibType,
    pub src: String,
    /// Public include directory. Accepts `inc` or `include` in TOML.
    #[serde(default, alias = "include")]
    pub inc: Option<String>,
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
    /// Features of this dep to activate (in addition to its defaults).
    #[serde(default)]
    pub features: Vec<String>,
    /// Whether to activate the dep's `default` features. Defaults to `true`.
    #[serde(default = "default_true", rename = "default-features")]
    pub default_features: bool,
    #[serde(default)]
    pub optional: bool,
    /// Target triples this prebuilt dep is compatible with (e.g. `["x86_64-linux-gnu"]`).
    /// `None` (absent) means compatible with all targets. Source deps ignore this field.
    /// Reserved for the cross-compilation phase.
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    /// Host OS requirement: dep is only included when the host OS (or a family
    /// alias) is in this list. Accepts a bare string or an array.
    /// Examples: `os = "linux"`, `os = ["linux", "macos"]`, `os = "unix"`.
    #[serde(default, deserialize_with = "string_or_vec")]
    pub os: Option<Vec<String>>,
    /// Host CPU architecture requirement: dep is only included when
    /// `std::env::consts::ARCH` matches one of the listed values.
    /// Examples: `arch = "x86_64"`, `arch = ["x86_64", "aarch64"]`.
    #[serde(default, deserialize_with = "string_or_vec")]
    pub arch: Option<Vec<String>>,
    /// Branch to check out for a git dependency. Mutually exclusive with `tag` and `rev`.
    #[serde(default)]
    pub branch: Option<String>,
    /// Tag to check out for a git dependency. Mutually exclusive with `branch` and `rev`.
    #[serde(default)]
    pub tag: Option<String>,
    /// Exact commit SHA or abbreviated ref to check out. Pins the dep to a
    /// specific commit and prevents `freight update` from moving it forward.
    #[serde(default)]
    pub rev: Option<String>,
    /// Delegate building this dep to an external build system rather than
    /// freight's own compiler. Values: `"cmake"`, `"make"`, `"meson"`, `"auto"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_system: Option<String>,
    /// Include directories to expose to code that depends on this dep,
    /// relative to the dep's source directory. Only used for foreign deps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Extra arguments forwarded verbatim to `cmake -S … -B …` during configure.
    /// Useful for silencing policy warnings on older CMakeLists.txt files, e.g.
    /// `cmake_args = ["-DCMAKE_POLICY_VERSION_MINIMUM=3.5"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cmake_args: Vec<String>,
    /// URL to a source archive (`.tar.gz`, `.tar.bz2`, `.tar.xz`, `.zip`).
    /// Any scheme that `curl` supports works: `https://`, `http://`, `ftp://`, etc.
    /// The archive is downloaded, optionally verified with `sha256`, extracted to
    /// `.deps/{name}/`, and then built by the auto-detected build system (or treated
    /// as header-only if no source files are found).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Expected SHA-256 checksum (lowercase hex) of the downloaded archive.
    /// Recommended for `url` deps; `freight fetch` rejects archives with a mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// pkg-config query string, e.g. `"libfoo >= 2.0"`. Freight runs
    /// `pkg-config --cflags --libs <query>` and injects the result into
    /// compilation and linking. No source build is performed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkg_config: Option<String>,
}

fn default_true() -> bool { true }

/// Deserialize a field that can be either a bare string or an array of strings.
fn string_or_vec<'de, D>(d: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(Option::<OneOrMany>::deserialize(d)?.map(|v| match v {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    }))
}

// ── Formatter / linter config ─────────────────────────────────────────────────

/// `[formatter]` — project code-style requirements.
///
/// ```toml
/// [formatter]
/// name  = "clang-format"   # which formatter (auto-detected when absent)
/// style = "Google"         # resolved through the template's settings map
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct FormatterConfig {
    /// Preferred formatter name (e.g. `"clang-format"`).
    #[serde(default)]
    pub name: Option<String>,
    /// Named settings resolved through the template's `settings` map.
    /// Written flat — `style = "Google"`, not `settings.style = "Google"`.
    #[serde(flatten)]
    pub settings: HashMap<String, String>,
}

/// `[linter]` — project code-quality requirements.
///
/// ```toml
/// [linter]
/// name   = "clang-tidy"
/// checks = "-*,modernize-*,bugprone-*"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LinterConfig {
    /// Preferred linter name (e.g. `"clang-tidy"`).
    #[serde(default)]
    pub name: Option<String>,
    /// Named settings resolved through the template's `settings` map.
    #[serde(flatten)]
    pub settings: HashMap<String, String>,
}

// ── Debugger config ───────────────────────────────────────────────────────────

/// `[debugger]` — per-project debugger configuration.
///
/// General settings here apply to every debugger. Debugger-specific settings
/// go under `[debugger.<name>]`. Which debugger to use is a user/machine
/// concern (CLI flag or toolchain selection), not a project setting.
///
/// ```toml
/// [debugger.gdb]        # GDB-specific settings
/// args  = ["--tui"]     # raw extra flags before the program separator
/// tui   = true          # resolved through gdb.rhai's settings map
/// quiet = true
///
/// [debugger.lldb]       # LLDB-specific settings
/// no_use_colors = true
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DebuggerConfig {
    /// Per-debugger configuration, keyed by debugger name.
    /// `[debugger.gdb]`, `[debugger.lldb]`, etc.
    #[serde(flatten)]
    pub debuggers: HashMap<String, DebuggerInstanceConfig>,
}

/// Configuration for a specific debugger, declared under `[debugger.<name>]`
/// in `~/.freight/config.toml`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DebuggerInstanceConfig {
    /// Raw extra flags inserted before the program separator.
    #[serde(default)]
    pub args: Vec<String>,
    /// Named settings resolved through the template's `settings` map.
    /// Written flat — `tui = true`, not `settings.tui = true`.
    #[serde(flatten)]
    pub settings: HashMap<String, bool>,
}

// ── Target / assembly config ──────────────────────────────────────────────────

/// `[target]` — CPU architecture and extension settings for assembly builds.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TargetConfig {
    /// Override the host CPU architecture used for assembler format selection
    /// (e.g. `arch = "x86_64"`). Defaults to the host arch at build time.
    #[serde(default)]
    pub arch: Option<String>,
    /// CPU extensions to enable (e.g. `["avx2", "fma"]`).
    /// Each entry produces a compiler flag via the template's `cpu_extension` pattern,
    /// e.g. `"-mavx2"` from `cpu_extension = "-m{name}"` in gcc.toml.
    #[serde(default)]
    pub cpu_extensions: Vec<String>,
}

// ── Compiler config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompilerConfig {
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
    /// Cross-compilation target triple — set via `freight --target` or `~/.freight/config.toml`,
    /// not in `freight.toml` (machine-local).
    #[serde(skip)]
    pub target: Option<String>,
    /// Path to the target sysroot — set via `~/.freight/config.toml`, not in `freight.toml`
    /// (machine-local absolute path).
    #[serde(skip)]
    pub sysroot: Option<String>,
    /// Path to a header to precompile (relative to the project root).
    /// E.g. `pch = "include/stdafx.h"`. The PCH is compiled once and
    /// injected into every source file of the matching language.
    #[serde(default)]
    pub pch: Option<String>,
}

impl Default for CompilerConfig {
    fn default() -> Self {
        Self {
            opt_level: default_opt_level(),
            debug: false,
            warnings: default_warnings(),
            defines: vec![],
            flags: vec![],
            includes: CompilerIncludes::default(),
            overrides: HashMap::default(),
            target: None,
            sysroot: None,
            pch: None,
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
    /// Any `[profile.<name>]` other than dev/release.
    #[serde(flatten, default)]
    pub custom: std::collections::HashMap<String, Profile>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Profile {
    /// Inherit settings from another named profile. The child overrides fields
    /// where specified; the parent fills in the rest. Up to 16 hops; cycles are
    /// silently broken at the first repeated name.
    #[serde(default)]
    pub inherits: Option<String>,
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
    /// Additional features active only in this profile (e.g. `features = ["mkl"]`).
    /// Merged with any features requested at the command line before resolution.
    #[serde(default)]
    pub features: Vec<String>,
}

// ── Platform overlays ─────────────────────────────────────────────────────────

/// Per-platform overlay applied on top of the base manifest when the host OS
/// matches. Only `dependencies` and a handful of compiler fields are
/// overlay-able — per-language stds, `[[bin]]` targets, profiles and
/// sanitizers are intentionally not platform-conditional in v1.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PlatformOverlay {
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
    #[serde(default)]
    pub compiler: PlatformCompilerOverlay,
    /// Per-language overrides — e.g. `[platform.linux.language.cpp]` can set `stdlib`.
    #[serde(default)]
    pub language: HashMap<String, LanguageSettings>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PlatformCompilerOverlay {
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub includes: CompilerIncludes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::load_manifest_str;

    fn host_overlay_block() -> String {
        // Build a manifest fragment with a platform section keyed on whichever
        // OS we're running the test under, so the test exercises the actual
        // host-detection path on every CI runner.
        let host = std::env::consts::OS;
        format!(
            r#"
[package]
name = "p"
version = "0.1.0"
[language.c]
[[bin]]
name = "p"
src  = "src/main.c"

[compiler]
defines = ["BASE"]

[platform.{host}.compiler]
defines = ["FROM_HOST"]
flags   = ["-DPLATFORM_FLAG"]

[platform.{host}.compiler.includes]
paths = ["platform-include/"]

[platform.{host}.dependencies]
hostlib = {{ system = "hostlib" }}
"#,
        )
    }

    #[test]
    fn platform_overlay_merges_into_build_settings() {
        let m = load_manifest_str(&host_overlay_block()).unwrap();
        let s = m.build_settings_for("dev");
        assert!(s.defines.contains(&"BASE".to_string()));
        assert!(s.defines.contains(&"FROM_HOST".to_string()));
        assert!(s.extra_flags.contains(&"-DPLATFORM_FLAG".to_string()));
        assert!(s.include_paths.iter().any(|p| p.ends_with("platform-include/")));
    }

    #[test]
    fn platform_overlay_adds_dependencies() {
        let m = load_manifest_str(&host_overlay_block()).unwrap();
        let deps = m.effective_dependencies();
        assert!(deps.contains_key("hostlib"));
    }

    #[test]
    fn non_matching_platform_overlay_is_ignored() {
        // Pick something that definitely isn't the host.
        let other = if std::env::consts::OS == "windows" { "linux" } else { "windows" };
        let s = format!(
            r#"
[package]
name = "p"
version = "0.1.0"
[language.c]
[[bin]]
name = "p"
src  = "src/main.c"

[platform.{other}.dependencies]
shouldnotbe = {{ system = "shouldnotbe" }}
"#
        );
        let m = load_manifest_str(&s).unwrap();
        let deps = m.effective_dependencies();
        assert!(!deps.contains_key("shouldnotbe"));
    }

    #[test]
    fn unix_alias_matches_unix_hosts() {
        let chain = host_platforms();
        let unix_hosts = ["linux", "macos", "freebsd", "openbsd", "netbsd"];
        if unix_hosts.contains(&std::env::consts::OS) {
            assert!(chain.contains(&"unix"), "expected `unix` in {chain:?}");
        }
    }

    #[test]
    fn host_platforms_specific_comes_after_family() {
        // Specific OS overlay should be applied last so it can override a
        // family-level overlay. Verify ordering: family aliases come before
        // the specific OS in the returned chain.
        let chain = host_platforms();
        let host = std::env::consts::OS;
        let specific = chain.iter().position(|p| *p == host).expect("host in chain");
        for (i, p) in chain.iter().enumerate() {
            if matches!(*p, "unix" | "bsd") {
                assert!(i < specific, "{p} should come before {host} in {chain:?}");
            }
        }
    }

    // ── dep os / arch filtering ───────────────────────────────────────────────

    fn manifest_with_dep_filter(os: Option<&str>, arch: Option<&str>) -> String {
        let os_line = os
            .map(|v| format!(", os = \"{v}\""))
            .unwrap_or_default();
        let arch_line = arch
            .map(|v| format!(", arch = \"{v}\""))
            .unwrap_or_default();
        format!(
            r#"
[package]
name = "p"
version = "0.1.0"
[[bin]]
name = "p"
src  = "src/main.cpp"
[dependencies]
mylib = {{ system = "mylib"{os_line}{arch_line} }}
"#
        )
    }

    #[test]
    fn dep_without_os_or_arch_always_included() {
        let m = load_manifest_str(&manifest_with_dep_filter(None, None)).unwrap();
        assert!(m.effective_dependencies().contains_key("mylib"));
    }

    #[test]
    fn dep_os_matching_host_is_included() {
        let host = std::env::consts::OS;
        let m = load_manifest_str(&manifest_with_dep_filter(Some(host), None)).unwrap();
        assert!(
            m.effective_dependencies().contains_key("mylib"),
            "dep with os = host OS should be included"
        );
    }

    #[test]
    fn dep_os_not_matching_host_is_excluded() {
        let other = if std::env::consts::OS == "windows" { "linux" } else { "windows" };
        let m = load_manifest_str(&manifest_with_dep_filter(Some(other), None)).unwrap();
        assert!(
            !m.effective_dependencies().contains_key("mylib"),
            "dep requiring a different OS should be excluded"
        );
    }

    #[test]
    fn dep_os_unix_alias_matches_unix_hosts() {
        let host = std::env::consts::OS;
        let is_unix = matches!(
            host,
            "linux" | "macos" | "freebsd" | "openbsd" | "netbsd" | "dragonfly"
                | "android" | "ios" | "solaris" | "illumos"
        );
        let m = load_manifest_str(&manifest_with_dep_filter(Some("unix"), None)).unwrap();
        let included = m.effective_dependencies().contains_key("mylib");
        assert_eq!(
            included, is_unix,
            "unix alias should match iff host is a unix OS; host={host}"
        );
    }

    #[test]
    fn dep_arch_matching_host_is_included() {
        let host_arch = std::env::consts::ARCH;
        let m = load_manifest_str(&manifest_with_dep_filter(None, Some(host_arch))).unwrap();
        assert!(
            m.effective_dependencies().contains_key("mylib"),
            "dep with arch = host arch should be included"
        );
    }

    #[test]
    fn dep_arch_not_matching_host_is_excluded() {
        let other = if std::env::consts::ARCH == "x86_64" { "s390x" } else { "x86_64" };
        let m = load_manifest_str(&manifest_with_dep_filter(None, Some(other))).unwrap();
        assert!(
            !m.effective_dependencies().contains_key("mylib"),
            "dep requiring a different arch should be excluded"
        );
    }

    #[test]
    fn dep_os_array_syntax_is_accepted() {
        let host = std::env::consts::OS;
        let s = format!(
            r#"
[package]
name = "p"
version = "0.1.0"
[[bin]]
name = "p"
src  = "src/main.cpp"
[dependencies]
mylib = {{ system = "mylib", os = ["{host}", "linux"] }}
"#
        );
        let m = load_manifest_str(&s).unwrap();
        assert!(m.effective_dependencies().contains_key("mylib"));
    }

    // ── cross-compilation: dep targets filtering ──────────────────────────────

    fn cross_manifest(dep_targets: Option<&[&str]>) -> String {
        let dep_targets_line = dep_targets
            .map(|ts| {
                let joined = ts.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ");
                format!(", targets = [{joined}]")
            })
            .unwrap_or_default();
        format!(
            r#"
[package]
name = "p"
version = "0.1.0"
[language.c]
[[bin]]
name = "p"
src  = "src/main.c"

[dependencies]
mylib = {{ path = "../mylib"{dep_targets_line} }}
"#
        )
    }

    #[test]
    fn dep_without_targets_always_included() {
        let m = load_manifest_str(&cross_manifest(None)).unwrap();
        assert!(m.effective_dependencies().contains_key("mylib"));
        let mut m2 = load_manifest_str(&cross_manifest(None)).unwrap();
        m2.compiler.target = Some("aarch64-linux-gnu".into());
        assert!(m2.effective_dependencies().contains_key("mylib"));
    }

    #[test]
    fn dep_with_targets_excluded_on_native_build() {
        let m = load_manifest_str(&cross_manifest(Some(&["aarch64-linux-gnu"]))).unwrap();
        assert!(
            !m.effective_dependencies().contains_key("mylib"),
            "target-specific dep should be excluded on native build"
        );
    }

    #[test]
    fn dep_with_matching_target_is_included() {
        let mut m = load_manifest_str(
            &cross_manifest(Some(&["aarch64-linux-gnu", "armv7-linux-gnu"]))
        ).unwrap();
        m.compiler.target = Some("aarch64-linux-gnu".into());
        assert!(
            m.effective_dependencies().contains_key("mylib"),
            "dep matching build target should be included"
        );
    }

    #[test]
    fn dep_with_non_matching_target_is_excluded() {
        let mut m = load_manifest_str(
            &cross_manifest(Some(&["aarch64-linux-gnu"]))
        ).unwrap();
        m.compiler.target = Some("x86_64-linux-gnu".into());
        assert!(
            !m.effective_dependencies().contains_key("mylib"),
            "dep for different target should be excluded"
        );
    }

    #[test]
    fn build_settings_propagates_target_triple_and_sysroot() {
        // target/sysroot are machine-local; set directly on the struct (not via TOML).
        let manifest_src = r#"
[package]
name = "p"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "p"
src  = "src/main.cpp"
"#;
        let mut m = load_manifest_str(manifest_src).unwrap();
        m.compiler.target  = Some("aarch64-linux-gnu".into());
        m.compiler.sysroot = Some("/opt/sysroot".into());
        let s = m.build_settings_for("dev");
        assert_eq!(s.target_triple.as_deref(), Some("aarch64-linux-gnu"));
        assert_eq!(s.sysroot.as_deref(), Some(std::path::Path::new("/opt/sysroot")));
    }
}
