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

    /// Iterate over `(name, dep)` pairs for the base `[dependencies]` plus any
    /// `[platform.X.dependencies]` whose `X` matches the host. A platform
    /// overlay can shadow a base dep with the same key — common when a package
    /// links a different system library on Windows vs Linux.
    ///
    /// Deps with a `targets = [...]` field are only included when
    /// `[compiler] target` matches one of the listed triples. This is intended
    /// for prebuilt path deps that have been compiled for a specific target; set
    /// no `targets` field to include a dep unconditionally.
    pub fn effective_dependencies(&self) -> HashMap<String, Dependency> {
        let current_target = self.compiler.target.as_deref();
        let mut out: HashMap<String, Dependency> = self.dependencies.iter()
            .filter(|(_, dep)| dep_matches_target(dep, current_target))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for plat in host_platforms() {
            if let Some(ov) = self.platform_overlay(plat) {
                for (name, dep) in &ov.dependencies {
                    if dep_matches_target(dep, current_target) {
                        out.insert(name.clone(), dep.clone());
                    }
                }
            }
        }
        out
    }

    /// Case-insensitive lookup of a platform overlay.
    fn platform_overlay(&self, name: &str) -> Option<&PlatformOverlay> {
        self.platform
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }
}

/// Returns `true` if the dependency should be included for the given build target.
///
/// When `targets` is absent the dep is unconditional. When `targets` is present the
/// dep is only included when `current_target` is in the list — a `None` current target
/// (native build) never matches target-specific deps.
fn dep_matches_target(dep: &Dependency, current_target: Option<&str>) -> bool {
    if let Dependency::Detailed(d) = dep {
        if let Some(targets) = &d.targets {
            return match current_target {
                Some(t) => targets.iter().any(|wanted| wanted == t),
                None => false,
            };
        }
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
        // Map a few rust os names back to the friendlier crane keys.
        other => other,
    });
    chain
}

/// Set of platform / family names crane recognizes in `[platform.X]` keys.
/// Used by validation; everything outside this set still parses but emits a
/// warning so typos don't silently no-op.
pub fn known_platform_keys() -> &'static [&'static str] {
    &[
        "unix", "bsd",
        "linux", "windows", "macos",
        "freebsd", "openbsd", "netbsd", "dragonfly",
        "android", "ios", "solaris", "illumos",
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

    // ── cross-compilation: dep targets filtering ──────────────────────────────

    fn cross_manifest(target: Option<&str>, dep_targets: Option<&[&str]>) -> String {
        let target_line = target.map(|t| format!("target = \"{t}\"")).unwrap_or_default();
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

[compiler]
{target_line}

[dependencies]
mylib = {{ path = "../mylib"{dep_targets_line} }}
"#
        )
    }

    #[test]
    fn dep_without_targets_always_included() {
        let m = load_manifest_str(&cross_manifest(None, None)).unwrap();
        assert!(m.effective_dependencies().contains_key("mylib"));
        let m2 = load_manifest_str(&cross_manifest(Some("aarch64-linux-gnu"), None)).unwrap();
        assert!(m2.effective_dependencies().contains_key("mylib"));
    }

    #[test]
    fn dep_with_targets_excluded_on_native_build() {
        let m = load_manifest_str(
            &cross_manifest(None, Some(&["aarch64-linux-gnu"]))
        ).unwrap();
        assert!(
            !m.effective_dependencies().contains_key("mylib"),
            "target-specific dep should be excluded on native build"
        );
    }

    #[test]
    fn dep_with_matching_target_is_included() {
        let m = load_manifest_str(
            &cross_manifest(Some("aarch64-linux-gnu"), Some(&["aarch64-linux-gnu", "armv7-linux-gnu"]))
        ).unwrap();
        assert!(
            m.effective_dependencies().contains_key("mylib"),
            "dep matching build target should be included"
        );
    }

    #[test]
    fn dep_with_non_matching_target_is_excluded() {
        let m = load_manifest_str(
            &cross_manifest(Some("x86_64-linux-gnu"), Some(&["aarch64-linux-gnu"]))
        ).unwrap();
        assert!(
            !m.effective_dependencies().contains_key("mylib"),
            "dep for different target should be excluded"
        );
    }

    #[test]
    fn build_settings_propagates_target_triple_and_sysroot() {
        let manifest_src = r#"
[package]
name = "p"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "p"
src  = "src/main.cpp"
[compiler]
target  = "aarch64-linux-gnu"
sysroot = "/opt/sysroot"
"#;
        let m = load_manifest_str(manifest_src).unwrap();
        let s = m.build_settings_for("dev");
        assert_eq!(s.target_triple.as_deref(), Some("aarch64-linux-gnu"));
        assert_eq!(s.sysroot.as_deref(), Some(std::path::Path::new("/opt/sysroot")));
    }
}
