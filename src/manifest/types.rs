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
    /// Common keys: `c`, `cpp`, `fortran`, `ada`, `d`, `cuda`, `objc`, `objcpp`.
    #[serde(default)]
    pub language: HashMap<String, LanguageSettings>,
    #[serde(rename = "lib", default)]
    pub lib: Option<LibTarget>,
    #[serde(rename = "bin", default)]
    pub bins: Vec<BinTarget>,
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
    /// Build-time tool dependencies — fetched and built before regular deps.
    /// Executables found in their install `bin/` are prepended to PATH for all
    /// subsequent build steps.  Use this for tools like cmake, ninja, protoc, etc.
    #[serde(rename = "build-dependencies", default)]
    pub build_dependencies: HashMap<String, Dependency>,
    /// Debug / development-only dependencies — linked in debug builds and tests
    /// but excluded from release artifacts.
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
    /// Freight lints (`[lints]`), e.g. `undeclared-include`.
    #[serde(default)]
    pub lints: LintsConfig,
    /// OS-conditional sources and defines — `[os.linux]`, `[os.windows]`, etc.
    /// Files listed here are excluded from the unconditional `src/` walk on
    /// non-matching platforms and only compiled on the named OS.
    #[serde(default)]
    pub os: HashMap<String, ConditionalSources>,
    /// Arch-conditional sources and defines — `[arch.x86_64]`, `[arch.aarch64]`, etc.
    /// Same exclusion semantics as `[os.*]` but matched against the target CPU arch.
    #[serde(default)]
    pub arch: HashMap<String, ConditionalSources>,
}

impl Manifest {
    /// Produce `BuildSettings` for the named profile (`"dev"` or `"release"`),
    /// starting from the base `[compiler]` settings and applying profile and
    /// platform overrides.
    pub fn build_settings_for(&self, profile_name: &str) -> BuildSettings {
        let mut defines = self.compiler.defines.clone();
        let mut flags = self.compiler.flags.clone();
        let mut include_paths: Vec<PathBuf> =
            self.compiler.includes.iter().map(PathBuf::from).collect();

        // Apply matching [os.*] overlays — family-first so `[os.unix]` is
        // applied before `[os.linux]` and the specific key wins.
        for os_key in host_platforms() {
            if let Some(ov) = self
                .os
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(os_key))
                .map(|(_, v)| v)
            {
                merge_string_vec(&mut defines, &ov.defines);
                merge_string_vec(&mut flags, &ov.flags);
                for p in &ov.includes {
                    let buf = PathBuf::from(p);
                    if !include_paths.contains(&buf) {
                        include_paths.push(buf);
                    }
                }
            }
        }
        // Apply matching [arch.*] overlay.
        let current_arch = self
            .target
            .arch
            .as_deref()
            .unwrap_or(std::env::consts::ARCH);
        if let Some(ov) = self
            .arch
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(current_arch))
            .map(|(_, v)| v)
        {
            merge_string_vec(&mut defines, &ov.defines);
            merge_string_vec(&mut flags, &ov.flags);
            for p in &ov.includes {
                let buf = PathBuf::from(p);
                if !include_paths.contains(&buf) {
                    include_paths.push(buf);
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
            auto_cpu_tuning: self.compiler.auto_cpu_tuning,
            arch: self
                .target
                .arch
                .clone()
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
            sanitize: if p.sanitize.is_empty() {
                base.sanitize
            } else {
                p.sanitize
            },
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
            if visited.len() >= 16 {
                break;
            } // max-hop guard
            if visited.contains(&current_name) {
                break;
            } // cycle guard
            visited.push(current_name.clone());
            let p = match current_name.as_str() {
                // "debug" is an alias for "dev" (used by freight dap).
                "debug" | "dev" => self.profile.dev.clone().unwrap_or(Profile {
                    inherits: None,
                    opt_level: Some(0),
                    debug: Some(true),
                    lto: Some(false),
                    strip: Some(false),
                    sanitize: vec![],
                    features: vec![],
                }),
                "release" => self.profile.release.clone().unwrap_or(Profile {
                    inherits: None,
                    opt_level: Some(3),
                    debug: Some(false),
                    lto: Some(false),
                    strip: Some(false),
                    sanitize: vec![],
                    features: vec![],
                }),
                // Built-in bench default: release-speed + debug symbols, no strip.
                // Overridable with [profile.bench] in freight.toml.
                "bench" => self
                    .profile
                    .custom
                    .get("bench")
                    .cloned()
                    .unwrap_or(Profile {
                        inherits: None,
                        opt_level: Some(3),
                        debug: Some(true),
                        lto: Some(false),
                        strip: Some(false),
                        sanitize: vec![],
                        features: vec![],
                    }),
                _ => self.profile.custom.get(&current_name).cloned()?,
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
            if child.opt_level.is_some() {
                merged.opt_level = child.opt_level;
            }
            if child.debug.is_some() {
                merged.debug = child.debug;
            }
            if child.lto.is_some() {
                merged.lto = child.lto;
            }
            if child.strip.is_some() {
                merged.strip = child.strip;
            }
            if !child.sanitize.is_empty() {
                merged.sanitize = child.sanitize;
            }
            if !child.features.is_empty() {
                for f in child.features {
                    if !merged.features.contains(&f) {
                        merged.features.push(f);
                    }
                }
            }
        }
        Some(merged)
    }

    /// Iterate over `(name, dep)` pairs for the base `[dependencies]` plus any
    /// `[os.X.dependencies]` or `[arch.X.dependencies]` whose key matches the
    /// host. A conditional section can shadow a base dep with the same key —
    /// useful for linking a different system library on Windows vs Linux.
    ///
    /// Deps are also filtered by fields on the dep itself:
    /// - `targets`: cross-compilation triple allowlist
    /// - `os`: host OS allowlist (supports family aliases like `"unix"`)
    /// - `arch`: host CPU architecture allowlist
    pub fn effective_dependencies(&self) -> HashMap<String, Dependency> {
        let current_target = self.compiler.target.as_deref();
        let current_arch = self
            .target
            .arch
            .as_deref()
            .unwrap_or(std::env::consts::ARCH);
        let mut out: HashMap<String, Dependency> = self
            .dependencies
            .iter()
            .filter(|(_, dep)| dep_matches_env(dep, current_target))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for os_key in host_platforms() {
            if let Some(ov) = self
                .os
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(os_key))
                .map(|(_, v)| v)
            {
                for (name, dep) in &ov.dependencies {
                    if dep_matches_env(dep, current_target) {
                        out.insert(name.clone(), dep.clone());
                    }
                }
            }
        }
        if let Some(ov) = self
            .arch
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(current_arch))
            .map(|(_, v)| v)
        {
            for (name, dep) in &ov.dependencies {
                if dep_matches_env(dep, current_target) {
                    out.insert(name.clone(), dep.clone());
                }
            }
        }
        out
    }

    /// Return the effective `LanguageSettings` for `lang_key`, applying any
    /// matching `[os.*]` and `[arch.*]` language overlays on top of the base.
    pub fn effective_language_settings(&self, lang_key: &str) -> LanguageSettings {
        let mut s = self.language.get(lang_key).cloned().unwrap_or_default();
        let current_arch = self
            .target
            .arch
            .as_deref()
            .unwrap_or(std::env::consts::ARCH);
        for os_key in host_platforms() {
            if let Some(ov) = self
                .os
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(os_key))
                .map(|(_, v)| v)
            {
                if let Some(lang_ov) = ov.language.get(lang_key) {
                    if lang_ov.std.is_some() {
                        s.std = lang_ov.std.clone();
                    }
                    if lang_ov.stdlib.is_some() {
                        s.stdlib = lang_ov.stdlib.clone();
                    }
                }
            }
        }
        if let Some(ov) = self
            .arch
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(current_arch))
            .map(|(_, v)| v)
        {
            if let Some(lang_ov) = ov.language.get(lang_key) {
                if lang_ov.std.is_some() {
                    s.std = lang_ov.std.clone();
                }
                if lang_ov.stdlib.is_some() {
                    s.stdlib = lang_ov.stdlib.clone();
                }
            }
        }
        s
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
    let Dependency::Detailed(d) = dep else {
        return true;
    };

    if let Some(targets) = &d.targets {
        let ok = match current_target {
            Some(t) => targets.iter().any(|wanted| wanted == t),
            None => false,
        };
        if !ok {
            return false;
        }
    }

    if let Some(os_req) = &d.os {
        let host_plats = host_platforms();
        let ok = os_req.iter().any(|req| {
            host_plats
                .iter()
                .any(|p| p.eq_ignore_ascii_case(req.as_str()))
        });
        if !ok {
            return false;
        }
    }

    if let Some(arch_req) = &d.arch {
        let host_arch = std::env::consts::ARCH;
        let ok = arch_req
            .iter()
            .any(|req| req.eq_ignore_ascii_case(host_arch));
        if !ok {
            return false;
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
        "linux"
            | "macos"
            | "freebsd"
            | "openbsd"
            | "netbsd"
            | "dragonfly"
            | "android"
            | "ios"
            | "solaris"
            | "illumos"
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
        "unix",
        "bsd",
        "linux",
        "windows",
        "macos",
        "freebsd",
        "openbsd",
        "netbsd",
        "dragonfly",
        "android",
        "ios",
        "solaris",
        "illumos",
    ]
}

/// Set of CPU architecture names accepted in dep `arch` fields.
/// Values mirror `std::env::consts::ARCH` plus common aliases.
pub fn known_arch_keys() -> &'static [&'static str] {
    &[
        "x86_64",
        "x86",
        "aarch64",
        "arm",
        "mips",
        "mips64",
        "powerpc",
        "powerpc64",
        "riscv64",
        "s390x",
        "sparc64",
        "wasm32",
        "wasm64",
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
    /// Boolean platform expression that gates whether this package
    /// can be built on the current host/target. Examples:
    /// `"windows & x64"`, `"!windows"`, `"(windows & !uwp) | linux"`.
    #[serde(default)]
    pub supports: Option<String>,
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
    /// Freeform options forwarded to the active compiler template's `language_option` handlers.
    /// E.g. `[language.cpp] unity_build = "true"` if the template declares that option.
    #[serde(flatten, default)]
    pub extra: HashMap<String, String>,
    /// Flags injected at build time by `language_option` handlers. Not persisted to TOML.
    #[serde(skip)]
    pub injected_flags: Vec<String>,
}

impl LanguageSettings {
    /// Returns just the freeform `extra` options (excluding `std`/`stdlib`).
    pub fn extra_options(&self) -> &HashMap<String, String> {
        &self.extra
    }
}

// ── Targets ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LibTarget {
    #[serde(rename = "type", default)]
    pub lib_type: LibType,
    /// Source files for this library. Accepts a single string or a list.
    #[serde(deserialize_with = "deserialize_string_or_vec")]
    pub srcs: Vec<String>,
    /// Public header files that form the library's API, exposed to dependents.
    /// Include directories are inferred from the parent directories of listed headers.
    #[serde(default)]
    pub hdrs: Vec<String>,
    /// Prebuilt library name passed to the linker (e.g. `-l<link>`). When set,
    /// `srcs` must be empty — `link` and source compilation are mutually exclusive.
    /// For `type = "system"` this defaults to the package name when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LibType {
    #[default]
    Static,
    Shared,
    /// Header-only library: no sources, no link step, only `hdrs` are exposed.
    Header,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BinTarget {
    pub name: String,
    pub src: String,
}

// ── Dependencies ──────────────────────────────────────────────────────────────

/// Platform package names — deps whose `features` list OS-bundled libraries
/// that ship *with* the operating system and require no separate fetch or build.
///
/// Only use platform packages for libraries that are part of the OS itself:
/// - `linux`:   `pthread`, `dl`, `rt`, `m`, `resolv`  (glibc/musl)
/// - `windows`: `ws2_32`, `kernel32`, `user32`, `crypt32`, `ole32`, `ntdll`  (Windows SDK)
/// - `macos`:   `CoreFoundation`, `Security`, `AppKit`, `Foundation`  (Apple frameworks)
///
/// Third-party libraries — even if commonly available via `apt`/`pacman`/`brew`
/// (e.g. `openssl`, `openblas`, `zlib`) — are regular deps resolved via
/// pkg-config or the registry, not platform features.
///
/// Feature → link flag mapping:
/// - macOS, leading uppercase → `-framework <Name>`
/// - all others → toolchain `system_lib_flag` (`-l<name>` on GCC/Clang, `<name>.lib` on MSVC)
pub const PLATFORM_PACKAGES: &[&str] = &[
    "windows",
    "linux",
    "macos",
    "osx",
    "unix",
    "android",
    "ios",
    "freebsd",
    "openbsd",
    "netbsd",
    "dragonfly",
];

pub fn is_platform_dep(name: &str) -> bool {
    PLATFORM_PACKAGES.contains(&name)
}

/// Whether a version string places no constraint — empty or a bare `*`.
///
/// Rejected for declared deps by manifest validation (every dependency needs a
/// concrete version), but still recognised across the resolver/fetch/query paths
/// to stay robust against transitive or legacy manifests that carry one.
pub fn is_unpinned_version(version: &str) -> bool {
    let v = version.trim();
    v.is_empty() || v == "*"
}

/// A dependency can be either a bare version string or a detailed table.
///
/// ```toml
/// libfoo  = "0.3"                           # Simple
/// myutils = { path = "../myutils" }         # Detailed
/// windows = { features = ["ws2_32"], os = "windows" }  # Platform dep
/// linux   = { features = ["m", "pthread"], os = "linux" }
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
    /// How the dep content should be treated. Covers build systems and
    /// content kinds. Values: `"cmake"`, `"make"`, `"meson"`, `"autotools"`,
    /// `"scons"`, `"bazel"`, `"none"`. Omit to auto-detect from the dep
    /// directory's marker files (CMakeLists.txt, meson.build, etc.).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub dep_type: Option<String>,
    /// Include directories to expose to code that depends on this dep,
    /// relative to the dep's source directory. Only used for foreign deps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Extra arguments forwarded verbatim to `cmake -S … -B …` during configure.
    /// Useful for silencing policy warnings on older CMakeLists.txt files, e.g.
    /// `cmake_args = ["-DCMAKE_POLICY_VERSION_MINIMUM=3.5"]`.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "cmake-args",
        alias = "cmake_args"
    )]
    pub cmake_args: Vec<String>,
    /// URL to a source archive (`.tar.gz`, `.tar.bz2`, `.tar.xz`, `.zip`).
    /// Any scheme that `curl` supports works: `https://`, `http://`, `ftp://`, etc.
    /// The archive is downloaded, optionally verified with `sha256`, extracted to
    /// `target/deps/{name}/`, and then built by the auto-detected build system (or treated
    /// as header-only if no source files are found).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Expected SHA-256 checksum (lowercase hex) of the downloaded archive.
    /// Recommended for `url` deps; `freight fetch` rejects archives with a mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Patch files to apply to the dep source after fetching, in order.
    /// Paths are relative to the project root. Applied with `patch -p1`.
    /// Example: `patches = ["patches/fix-cmake.patch"]`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<String>,
    /// Explicit resolver to use for this version dep.
    /// Accepted values: `"system"`, `"pkg-config"`, or a named registry.
    /// When omitted, freight tries `pkg-config → system stubs → registry` in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Override the dep's own `[compiler] unity` setting.
    /// `unity = true` forces a unity build of this dep regardless of its manifest;
    /// `unity = false` disables unity even if the dep enables it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unity: Option<bool>,
    /// Registry channel to fetch this dep from (e.g. `"stable"`, `"experimental"`).
    /// When absent the registry uses its default channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

impl DetailedDep {
    /// True when this dep is a git source: has a `url` and at least one of
    /// `branch`, `tag`, `rev`, or the URL ends with `.git`.
    pub fn is_git(&self) -> bool {
        self.url.is_some()
            && (self.branch.is_some()
                || self.tag.is_some()
                || self.rev.is_some()
                || self.url.as_deref().is_some_and(|u| u.ends_with(".git")))
    }
}

fn default_true() -> bool {
    true
}

/// Deserialize a required field that accepts either a bare string or an array of strings.
fn deserialize_string_or_vec<'de, D>(d: D) -> Result<Vec<String>, D::Error>
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
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

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

// ── Lints config ──────────────────────────────────────────────────────────────

/// Severity of a Freight lint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LintLevel {
    /// Do not report.
    Allow,
    /// Report as a warning (LSP) — the default.
    #[default]
    Warn,
    /// Report as an error (LSP); a hard build failure once enforcement lands.
    Deny,
}

/// Freight lints, `[lints]`. See `docs/include-hygiene.md`.
///
/// ```toml
/// [lints]
/// undeclared-include = "warn"   # "allow" | "warn" | "deny"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LintsConfig {
    /// How to report an `#include` that resolves to a header provided by no
    /// declared package (and is not a language standard-library header).
    /// Defaults to `warn`.
    #[serde(rename = "undeclared-include", default)]
    pub undeclared_include: LintLevel,
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
    #[serde(default, rename = "cpu-extensions", alias = "cpu_extensions")]
    pub cpu_extensions: Vec<String>,
}

// ── Compiler config ───────────────────────────────────────────────────────────

/// Per-compiler-tool options declared under `[compiler.<name>]` in `freight.toml`.
/// E.g. `[compiler.clang++] lto_mode = "thin"` → forwarded to the template's
/// `compiler_option` handlers when that tool is the active compiler.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CompilerToolOptions {
    /// Semver requirement for the compiler version, e.g. `">=14.0"` or `">=14, <16"`.
    #[serde(default)]
    pub version: Option<String>,
    #[serde(flatten)]
    pub options: HashMap<String, String>,
}

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
    /// Extra include directories added to every compilation (`-I` flags).
    #[serde(default)]
    pub includes: Vec<String>,
    /// Cross-compilation target triple — set via `freight --target` or `~/.freight/config.toml`,
    /// not in `freight.toml` (machine-local).
    #[serde(skip)]
    pub target: Option<String>,
    /// Path to the target sysroot — set via `~/.freight/config.toml`, not in `freight.toml`
    /// (machine-local absolute path).
    #[serde(skip)]
    pub sysroot: Option<String>,
    /// Whether compiler templates may derive CPU tuning flags from target/sysroot.
    #[serde(skip)]
    pub auto_cpu_tuning: bool,
    /// Path to a header to precompile (relative to the project root).
    /// E.g. `pch = "include/stdafx.h"`. The PCH is compiled once and
    /// injected into every source file of the matching language.
    #[serde(default)]
    pub pch: Option<String>,
    /// Enable unity (jumbo) builds: all sources of the same language are concatenated
    /// into a single translation unit via `#include` and compiled together.
    /// Trades incremental speed for faster full builds and better cross-TU inlining.
    /// Only applies to C, C++, CUDA, HIP, and OpenCL; other languages compile individually.
    #[serde(default)]
    pub unity: bool,
    /// Per-compiler-tool option sub-tables: `[compiler.<name>]`.
    /// Options here are forwarded to the matching template's `compiler_option` handlers.
    #[serde(flatten, default)]
    pub per_tool: HashMap<String, CompilerToolOptions>,
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
            includes: vec![],
            target: None,
            sysroot: None,
            auto_cpu_tuning: true,
            pch: None,
            unity: false,
            per_tool: HashMap::default(),
        }
    }
}

/// The compiler backend name from `[compiler] backend = "..."`.
/// Stored as a plain string so user-added templates are supported without a Rust change.
/// Special value `"auto"` (the default) picks the first available compiler for each language.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Backend(pub String);

impl Default for Backend {
    fn default() -> Self {
        Self("auto".into())
    }
}

impl Backend {
    pub fn is_auto(&self) -> bool {
        self.0.eq_ignore_ascii_case("auto")
    }
    pub fn name(&self) -> &str {
        &self.0
    }
}

fn default_opt_level() -> u8 {
    2
}
fn default_warnings() -> String {
    "all".to_string()
}

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

// ── Conditional sections (os / arch) ─────────────────────────────────────────

/// Everything that can vary by OS or CPU architecture.
/// Used by `[os.<name>]` and `[arch.<name>]` manifest sections.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ConditionalSources {
    /// Glob patterns (relative to project root) of source files to include
    /// only on this platform. Files matching any pattern across *any* os/arch
    /// section are excluded from the unconditional `src/` walk.
    #[serde(default)]
    pub srcs: Vec<String>,
    /// Preprocessor defines injected when this platform is active.
    #[serde(default)]
    pub defines: Vec<String>,
    /// Extra compiler flags injected when this platform is active.
    #[serde(default)]
    pub flags: Vec<String>,
    /// Extra include paths injected when this platform is active.
    #[serde(default)]
    pub includes: Vec<String>,
    /// Dependencies active only on this platform. Shadow base deps with the
    /// same name — useful for linking a different system library per OS.
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
    /// Per-language overrides active only on this platform (e.g. `[os.linux.language.cpp]`).
    #[serde(default)]
    pub language: HashMap<String, LanguageSettings>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::load_manifest_str;

    fn host_overlay_block() -> String {
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

[os.{host}]
defines  = ["FROM_HOST"]
flags    = ["-DPLATFORM_FLAG"]
includes = ["platform-include/"]

[os.{host}.dependencies]
hostlib = {{ version = "1" }}
"#,
        )
    }

    #[test]
    fn os_overlay_merges_into_build_settings() {
        let m = load_manifest_str(&host_overlay_block()).unwrap();
        let s = m.build_settings_for("dev");
        assert!(s.defines.contains(&"BASE".to_string()));
        assert!(s.defines.contains(&"FROM_HOST".to_string()));
        assert!(s.extra_flags.contains(&"-DPLATFORM_FLAG".to_string()));
        assert!(s
            .include_paths
            .iter()
            .any(|p| p.ends_with("platform-include/")));
    }

    #[test]
    fn os_overlay_adds_dependencies() {
        let m = load_manifest_str(&host_overlay_block()).unwrap();
        let deps = m.effective_dependencies();
        assert!(deps.contains_key("hostlib"));
    }

    #[test]
    fn non_matching_os_overlay_is_ignored() {
        let other = if std::env::consts::OS == "windows" {
            "linux"
        } else {
            "windows"
        };
        let s = format!(
            r#"
[package]
name = "p"
version = "0.1.0"
[language.c]
[[bin]]
name = "p"
src  = "src/main.c"

[os.{other}.dependencies]
shouldnotbe = {{ version = "1" }}
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
        let specific = chain
            .iter()
            .position(|p| *p == host)
            .expect("host in chain");
        for (i, p) in chain.iter().enumerate() {
            if matches!(*p, "unix" | "bsd") {
                assert!(i < specific, "{p} should come before {host} in {chain:?}");
            }
        }
    }

    // ── dep os / arch filtering ───────────────────────────────────────────────

    fn manifest_with_dep_filter(os: Option<&str>, arch: Option<&str>) -> String {
        let os_line = os.map(|v| format!(", os = \"{v}\"")).unwrap_or_default();
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
mylib = {{ version = "1"{os_line}{arch_line} }}
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
        let other = if std::env::consts::OS == "windows" {
            "linux"
        } else {
            "windows"
        };
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
            "linux"
                | "macos"
                | "freebsd"
                | "openbsd"
                | "netbsd"
                | "dragonfly"
                | "android"
                | "ios"
                | "solaris"
                | "illumos"
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
        let other = if std::env::consts::ARCH == "x86_64" {
            "s390x"
        } else {
            "x86_64"
        };
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
mylib = {{ version = "1", os = ["{host}", "linux"] }}
"#
        );
        let m = load_manifest_str(&s).unwrap();
        assert!(m.effective_dependencies().contains_key("mylib"));
    }

    // ── cross-compilation: dep targets filtering ──────────────────────────────

    fn cross_manifest(dep_targets: Option<&[&str]>) -> String {
        let dep_targets_line = dep_targets
            .map(|ts| {
                let joined = ts
                    .iter()
                    .map(|t| format!("\"{t}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
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
        let mut m = load_manifest_str(&cross_manifest(Some(&[
            "aarch64-linux-gnu",
            "armv7-linux-gnu",
        ])))
        .unwrap();
        m.compiler.target = Some("aarch64-linux-gnu".into());
        assert!(
            m.effective_dependencies().contains_key("mylib"),
            "dep matching build target should be included"
        );
    }

    #[test]
    fn dep_with_non_matching_target_is_excluded() {
        let mut m = load_manifest_str(&cross_manifest(Some(&["aarch64-linux-gnu"]))).unwrap();
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
        m.compiler.target = Some("aarch64-linux-gnu".into());
        m.compiler.sysroot = Some("/opt/sysroot".into());
        let s = m.build_settings_for("dev");
        assert_eq!(s.target_triple.as_deref(), Some("aarch64-linux-gnu"));
        assert_eq!(
            s.sysroot.as_deref(),
            Some(std::path::Path::new("/opt/sysroot"))
        );
    }
}
