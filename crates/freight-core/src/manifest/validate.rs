use std::collections::HashSet;
use std::path::Path;

use semver::Version;

use super::types::{known_arch_keys, known_platform_keys, Dependency, DetailedDep, Manifest, Profile};
use crate::toolchain::CompilerTemplate;

const VALID_WARNINGS: &[&str] = &["none", "default", "all", "error"];

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub context: String,
    pub message: String,
}

impl ValidationError {
    fn new(ctx: &str, msg: impl Into<String>) -> Self {
        Self { context: ctx.to_string(), message: msg.into() }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.context, self.message)
    }
}

/// Validate a parsed manifest and return every problem found.
///
/// Pass the loaded compiler templates so language keys and standards can be checked
/// against what the toolchain actually supports. An empty `templates` slice skips
/// template-dependent checks (language key validity, std validity) without error.
pub fn validate(manifest: &Manifest, templates: &[CompilerTemplate]) -> Vec<ValidationError> {
    let mut errors: Vec<ValidationError> = Vec::new();

    validate_package(manifest, &mut errors);
    validate_language(manifest, templates, &mut errors);
    validate_lang_std_consistency(manifest, &mut errors);
    validate_targets(manifest, &mut errors);
    validate_compiler(manifest, &mut errors);
    validate_profiles(manifest, &mut errors);
    validate_os_arch_keys(manifest, &mut errors);
    validate_dep_env_filters(manifest, &mut errors);
    validate_features(manifest, &mut errors);
    validate_foreign_deps(manifest, &mut errors);

    errors
}

fn validate_features(m: &Manifest, errors: &mut Vec<ValidationError>) {
    // Every name referenced in a feature list must itself be a key in [features].
    // "default" is the only allowed forward-reference to a pseudo-key.
    for (feat, deps) in &m.features {
        for dep in deps {
            if dep == "default" {
                errors.push(ValidationError::new(
                    &format!("[features.{feat}]"),
                    "'default' cannot be listed as a feature dependency",
                ));
                continue;
            }
            if !m.features.contains_key(dep.as_str()) {
                errors.push(ValidationError::new(
                    &format!("[features.{feat}]"),
                    format!("unknown feature '{dep}'"),
                ));
            }
        }
    }

    // Detect cycles via DFS.
    let keys: Vec<&String> = m.features.keys().filter(|k| k.as_str() != "default").collect();
    for start in &keys {
        let mut visited = HashSet::new();
        let mut stack = vec![start.as_str()];
        while let Some(cur) = stack.pop() {
            if cur == "default" { continue; }
            if !visited.insert(cur) {
                errors.push(ValidationError::new(
                    &format!("[features.{start}]"),
                    format!("feature cycle detected involving '{cur}'"),
                ));
                break;
            }
            if let Some(deps) = m.features.get(cur) {
                for d in deps { stack.push(d.as_str()); }
            }
        }
    }

    // Validate features requested on dep declarations.
    for (dep_name, dep) in &m.dependencies {
        let Dependency::Detailed(d) = dep else { continue };
        for feat in &d.features {
            // We can't validate features on foreign/registry deps (no local manifest to check),
            // but we can catch obviously wrong things if it's a path dep with a loaded manifest.
            // For now, just flag if `default-features = false` with no features listed.
            let _ = (dep_name, feat);  // reserved for future cross-manifest checks
        }
    }
}

fn validate_foreign_deps(m: &Manifest, errors: &mut Vec<ValidationError>) {
    for (name, dep) in &m.dependencies {
        let Dependency::Detailed(d) = dep else { continue };
        let ctx = format!("[dependencies.{name}]");

        // pkg_config is for system-installed libraries. It can be used alone
        // (`{ pkg_config = "zlib" }`) or with system as a -l{name} fallback
        // (`{ system = "z", pkg_config = "zlib" }`). It must not be combined
        // with source dep kinds (path / git / url).
        if d.pkg_config.is_some() {
            let has_source = d.path.is_some() || d.git.is_some() || d.url.is_some();
            if has_source {
                errors.push(ValidationError::new(
                    &ctx,
                    "pkg_config cannot be combined with path, git, or url",
                ));
            }
        }

        if let Some(repo) = &d.repo {
            const KNOWN_REPOS: &[&str] = &["pkg-config", "conan", "vcpkg"];
            if !KNOWN_REPOS.contains(&repo.as_str()) {
                errors.push(ValidationError::new(
                    &ctx,
                    format!("unknown repo {:?}; accepted: {}", repo, KNOWN_REPOS.join(", ")),
                ));
            }
            let is_version_dep = d.version.is_some()
                && d.path.is_none()
                && d.system.is_none()
                && d.git.is_none()
                && d.url.is_none()
                && d.pkg_config.is_none();
            if !is_version_dep {
                errors.push(ValidationError::new(
                    &ctx,
                    "repo is only valid for version deps (no path, git, url, system, or pkg-config)",
                ));
            }
        }

    }
}

fn validate_dep_env_filters(m: &Manifest, errors: &mut Vec<ValidationError>) {
    let known_plats = known_platform_keys();
    let known_archs = known_arch_keys();

    let check_dep = |name: &str, dep: &DetailedDep, errors: &mut Vec<ValidationError>| {
        let ctx = format!("[dependencies.{name}]");
        if let Some(os_list) = &dep.os {
            for os in os_list {
                let lc = os.to_ascii_lowercase();
                if !known_plats.iter().any(|k| *k == lc) {
                    errors.push(ValidationError::new(
                        &ctx,
                        format!(
                            "unknown os value {:?}; expected one of: {}",
                            os,
                            known_plats.join(", "),
                        ),
                    ));
                }
            }
        }
        if let Some(arch_list) = &dep.arch {
            for arch in arch_list {
                let lc = arch.to_ascii_lowercase();
                if !known_archs.iter().any(|k| *k == lc) {
                    errors.push(ValidationError::new(
                        &ctx,
                        format!(
                            "unknown arch value {:?}; expected one of: {}",
                            arch,
                            known_archs.join(", "),
                        ),
                    ));
                }
            }
        }
    };

    for (name, dep) in &m.dependencies {
        if let Dependency::Detailed(d) = dep {
            check_dep(name, d, errors);
        }
    }
    for (name, dep) in &m.dev_dependencies {
        if let Dependency::Detailed(d) = dep {
            check_dep(name, d, errors);
        }
    }
}

fn validate_os_arch_keys(m: &Manifest, errors: &mut Vec<ValidationError>) {
    let known_os   = known_platform_keys();
    let known_arch = known_arch_keys();
    for key in m.os.keys() {
        let lc = key.to_ascii_lowercase();
        if !known_os.iter().any(|k| *k == lc) {
            errors.push(ValidationError::new(
                &format!("[os.{key}]"),
                format!(
                    "unknown OS key {:?}; expected one of: {}",
                    key, known_os.join(", "),
                ),
            ));
        }
    }
    for key in m.arch.keys() {
        let lc = key.to_ascii_lowercase();
        if !known_arch.iter().any(|k| *k == lc) {
            errors.push(ValidationError::new(
                &format!("[arch.{key}]"),
                format!(
                    "unknown arch key {:?}; expected one of: {}",
                    key, known_arch.join(", "),
                ),
            ));
        }
    }
}

fn validate_package(m: &Manifest, errors: &mut Vec<ValidationError>) {
    let pkg = &m.package;

    if pkg.name.is_empty() {
        errors.push(ValidationError::new("[package]", "name must not be empty"));
    } else if !is_valid_package_name(&pkg.name) {
        errors.push(ValidationError::new(
            "[package]",
            format!("name {:?} contains invalid characters (use letters, digits, - or _)", pkg.name),
        ));
    }

    if Version::parse(&pkg.version).is_err() {
        errors.push(ValidationError::new(
            "[package]",
            format!("version {:?} is not valid semver (expected major.minor.patch)", pkg.version),
        ));
    }

    if let Some(supports) = &pkg.supports {
        if supports.trim().is_empty() {
            errors.push(ValidationError::new(
                "[package]",
                "supports must not be empty when present",
            ));
        } else {
            match m.supports_current_platform() {
                Ok(true) => {}
                Ok(false) => errors.push(ValidationError::new(
                    "[package]",
                    format!(
                        "current platform is not supported by supports expression {:?}",
                        supports,
                    ),
                )),
                Err(msg) => errors.push(ValidationError::new(
                    "[package]",
                    format!("invalid supports expression {:?}: {msg}", supports),
                )),
            }
        }
    }
}

fn validate_language(m: &Manifest, templates: &[CompilerTemplate], errors: &mut Vec<ValidationError>) {
    for (key, settings) in &m.language {
        let ctx = format!("[language.{key}]");

        let handling: Vec<&CompilerTemplate> = templates.iter()
            .filter(|t| t.linking.contains_key(key.as_str()))
            .collect();

        if !templates.is_empty() && handling.is_empty() {
            let mut known: Vec<String> = templates.iter()
                .flat_map(|t| t.linking.keys().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            known.sort();
            errors.push(ValidationError::new(
                &ctx,
                format!("{key:?} is not a supported language key; known keys: {}", known.join(", ")),
            ));
            continue;
        }

        if let Some(std) = &settings.std {
            let valid_stds: HashSet<&str> = handling.iter()
                .flat_map(|t| t.standards.keys().map(String::as_str))
                .collect();
            // Empty valid_stds means the language uses no -std= flag; treat any value as docs.
            if !valid_stds.is_empty() && !valid_stds.contains(std.as_str()) {
                let mut sorted: Vec<&str> = valid_stds.into_iter().collect();
                sorted.sort();
                errors.push(ValidationError::new(
                    &ctx,
                    format!("std {:?} is not valid for {key}; valid standards: {}", std, sorted.join(", ")),
                ));
            }
        }

        // Validate stdlib (C++) — checked against the union of flags_stdlib keys.
        if key == "cpp" {
            if let Some(stdlib) = &settings.stdlib {
                let valid: HashSet<&str> = handling.iter()
                    .flat_map(|t| t.flags_stdlib.keys().map(String::as_str))
                    .collect();
                if !valid.is_empty() && !valid.contains(stdlib.as_str()) {
                    let mut sorted: Vec<&str> = valid.into_iter().collect();
                    sorted.sort();
                    errors.push(ValidationError::new(
                        &ctx,
                        format!("stdlib {:?} is not supported; valid values: {}", stdlib, sorted.join(", ")),
                    ));
                }
            }
        }

    }
}

fn validate_targets(m: &Manifest, errors: &mut Vec<ValidationError>) {
    if m.bins.is_empty() && m.lib.is_none() {
        errors.push(ValidationError::new(
            "targets",
            "at least one [[bin]] or [lib] target must be defined",
        ));
    }

    for (i, bin) in m.bins.iter().enumerate() {
        let ctx = format!("[[bin]][{i}]");
        if bin.name.is_empty() {
            errors.push(ValidationError::new(&ctx, "name must not be empty"));
        }
        if bin.src.is_empty() {
            errors.push(ValidationError::new(&ctx, "src must not be empty"));
        }
    }

    if let Some(lib) = &m.lib {
        if lib.srcs.is_empty() {
            errors.push(ValidationError::new("[lib]", "srcs must not be empty"));
        }
    }
}

fn validate_compiler(m: &Manifest, errors: &mut Vec<ValidationError>) {
    let cc = &m.compiler;

    if cc.opt_level > 3 {
        errors.push(ValidationError::new(
            "[compiler]",
            format!("opt-level {} is out of range; must be 0–3", cc.opt_level),
        ));
    }

    if !VALID_WARNINGS.contains(&cc.warnings.as_str()) {
        errors.push(ValidationError::new(
            "[compiler]",
            format!("warnings {:?} is not valid; choose one of: {}", cc.warnings, VALID_WARNINGS.join(", ")),
        ));
    }

}

fn validate_profiles(m: &Manifest, errors: &mut Vec<ValidationError>) {
    if let Some(dev) = &m.profile.dev {
        validate_profile(dev, "[profile.dev]", errors);
    }
    if let Some(rel) = &m.profile.release {
        validate_profile(rel, "[profile.release]", errors);
    }
}

fn validate_profile(p: &Profile, ctx: &str, errors: &mut Vec<ValidationError>) {
    if let Some(opt) = p.opt_level {
        if opt > 3 {
            errors.push(ValidationError::new(
                ctx,
                format!("opt-level {opt} is out of range; must be 0–3"),
            ));
        }
    }
}

fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Check ABI compatibility of path dependencies against the current project.
///
/// Compatibility is determined by the `[compiler.linking]` sections of the loaded
/// compiler templates — no rules are hardcoded in Rust. Only path deps are checked;
/// system/registry deps expose a C ABI by convention and are always safe to link.
/// If a dep's `freight.toml` cannot be read it is silently skipped.
/// An empty `templates` slice skips all compatibility checks.
pub fn validate_dep_compat(
    manifest: &Manifest,
    base_dir: &Path,
    templates: &[CompilerTemplate],
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    let project_langs: HashSet<&str> = manifest.language.keys().map(String::as_str).collect();
    if project_langs.is_empty() || templates.is_empty() {
        return errors;
    }

    // Collect all ABIs the project's languages are compatible with,
    // always including each language's own ABI (a language links with itself).
    let allowed_abis: HashSet<&str> = project_langs.iter()
        .flat_map(|&lang| {
            templates.iter()
                .filter_map(move |t| t.linking.get(lang))
                .flat_map(|l| {
                    std::iter::once(l.abi.as_str())
                        .chain(l.compatible.iter().map(String::as_str))
                })
        })
        .collect();

    for (dep_name, dep) in &manifest.dependencies {
        let rel_path = match dep {
            Dependency::Detailed(d) => match d.path.as_deref() {
                Some(p) => p,
                None => continue,
            },
            Dependency::Simple(_) => continue,
        };

        let dep_dir = base_dir.join(rel_path);
        let Ok(src) = std::fs::read_to_string(dep_dir.join("freight.toml")) else { continue };
        let Ok(dep_manifest) = toml_edit::de::from_str::<Manifest>(&src) else { continue };

        match dep_manifest.supports_current_platform() {
            Ok(true) => {}
            Ok(false) => errors.push(ValidationError::new(
                &format!("[dependencies.{dep_name}]"),
                format!(
                    "dependency package {} does not support the current platform ({})",
                    dep_manifest.package.name,
                    dep_manifest.package.supports.as_deref().unwrap_or_default(),
                ),
            )),
            Err(msg) => errors.push(ValidationError::new(
                &format!("[dependencies.{dep_name}]"),
                format!(
                    "dependency package {} has an invalid supports expression: {msg}",
                    dep_manifest.package.name,
                ),
            )),
        }

        for dep_lang in dep_manifest.language.keys() {
            let dep_abi = templates.iter()
                .filter_map(|t| t.linking.get(dep_lang.as_str()))
                .map(|l| l.abi.as_str())
                .next();

            if let Some(abi) = dep_abi {
                if !allowed_abis.contains(abi) {
                    errors.push(ValidationError::new(
                        &format!("[dependencies.{dep_name}]"),
                        format!(
                            "library language \"{dep_lang}\" (ABI: \"{abi}\") is not compatible \
                             with project language(s) [{}]",
                            sorted_langs(&project_langs).join(", ")
                        ),
                    ));
                }
            }

            // For C and C++, the dep's standard must not be newer than the project's.
            // A library compiled with a newer std may use features unavailable to the caller.
            if matches!(dep_lang.as_str(), "c" | "cpp") {
                let proj_std = manifest.language.get(dep_lang.as_str())
                    .and_then(|l| l.std.as_deref());
                let dep_std = dep_manifest.language.get(dep_lang.as_str())
                    .and_then(|l| l.std.as_deref());

                if let (Some(ps), Some(ds)) = (proj_std, dep_std) {
                    if std_year(ds) > std_year(ps) {
                        errors.push(ValidationError::new(
                            &format!("[dependencies.{dep_name}]"),
                            format!(
                                "{dep_lang} library uses std \"{ds}\" which is newer than the \
                                 project's \"{ps}\" — the project must use at least \"{ds}\""
                            ),
                        ));
                    }
                }
            }
        }
    }

    errors
}

/// Check that C and C++ standards are mutually consistent within one project.
///
/// When both languages are active, they must either both declare a std or neither
/// should. If both declare one, the C std must not be newer than the C++ std —
/// mixing c23 headers with a c++17 translation unit causes hard-to-diagnose
/// symbol resolution failures at link time.
fn validate_lang_std_consistency(m: &Manifest, errors: &mut Vec<ValidationError>) {
    if !m.language.contains_key("c") || !m.language.contains_key("cpp") {
        return;
    }

    let c_std   = m.language.get("c")  .and_then(|l| l.std.as_deref());
    let cpp_std = m.language.get("cpp").and_then(|l| l.std.as_deref());

    match (c_std, cpp_std) {
        (Some(_), None) | (None, Some(_)) => {
            errors.push(ValidationError::new(
                "[language]",
                "when mixing [language.c] and [language.cpp] both must declare std or neither should",
            ));
        }
        (Some(cs), Some(cpps)) => {
            if std_year(cs) > std_year(cpps) {
                errors.push(ValidationError::new(
                    "[language]",
                    format!(
                        "C standard \"{cs}\" is newer than C++ standard \"{cpps}\"; \
                         use a matching or older C std to avoid link-time symbol conflicts"
                    ),
                ));
            }
        }
        (None, None) => {}
    }
}

/// Return a numeric year for C and C++ standard strings for ordering comparisons.
/// Returns 0 for unknown or non-versioned standards (treated as "no constraint").
fn std_year(std: &str) -> u32 {
    match std {
        "c11"   => 11, "c17" => 17, "c23" => 23,
        "c++11" => 11, "c++14" => 14, "c++17" => 17, "c++20" => 20, "c++23" => 23,
        _       => 0,
    }
}

fn sorted_langs<'a>(langs: &HashSet<&'a str>) -> Vec<&'a str> {
    let mut v: Vec<&'a str> = langs.iter().copied().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::load_manifest_str;

    const TEMPLATES_DIR: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../toolchains");

    fn test_templates() -> Vec<CompilerTemplate> {
        crate::toolchain::load_templates(std::path::Path::new(TEMPLATES_DIR))
    }

    const FULL_MANIFEST: &str = r#"
[package]
name        = "myproject"
version     = "0.1.0"
description = "A test project"
license     = "MIT"

[language.cpp]
std = "c++20"

[[bin]]
name = "myproject"
src  = "src/main.cpp"

[compiler]
opt-level = 2
debug     = false
warnings  = "all"

[profile.dev]
opt-level = 0
debug     = true

[profile.release]
opt-level = 3
lto       = true
strip     = true
debug     = false
"#;

    fn load(s: &str) -> crate::manifest::types::Manifest {
        load_manifest_str(s).unwrap()
    }

    fn errors(s: &str) -> Vec<ValidationError> {
        validate(&load(s), &test_templates())
    }

    fn field_errors(s: &str, ctx: &str) -> Vec<ValidationError> {
        errors(s).into_iter().filter(|e| e.context.contains(ctx)).collect()
    }

    #[test]
    fn full_manifest_is_valid() {
        assert!(errors(FULL_MANIFEST).is_empty(), "full manifest should be valid");
    }

    #[test]
    fn minimal_valid_manifest() {
        let s = r#"
[package]
name    = "foo"
version = "1.0.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        assert!(errors(s).is_empty());
    }

    #[test]
    fn empty_package_name() {
        let s = r#"
[package]
name    = ""
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        assert!(!field_errors(s, "[package]").is_empty());
    }

    #[test]
    fn invalid_semver_version() {
        let s = r#"
[package]
name    = "foo"
version = "0.1"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        let errs = field_errors(s, "[package]");
        assert!(!errs.is_empty());
        assert!(errs.iter().any(|e| e.message.contains("semver")));
    }

    #[test]
    fn package_supports_matching_platform_is_valid() {
        let platform = if std::env::consts::OS == "macos" {
            "osx"
        } else {
            std::env::consts::OS
        };
        let s = format!(r#"
[package]
name    = "foo"
version = "0.1.0"
supports = "{platform}"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#);

        assert!(field_errors(&s, "[package]").is_empty());
    }

    #[test]
    fn package_supports_rejects_non_matching_platform() {
        let unsupported = if std::env::consts::OS == "windows" {
            "linux"
        } else {
            "windows"
        };
        let s = format!(r#"
[package]
name    = "foo"
version = "0.1.0"
supports = "{unsupported}"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#);

        let errs = field_errors(&s, "[package]");
        assert!(
            errs.iter().any(|e| e.message.contains("current platform is not supported")),
            "expected unsupported-platform error, got {errs:?}"
        );
    }

    #[test]
    fn package_supports_rejects_invalid_expression() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
supports = "windows &"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;

        let errs = field_errors(s, "[package]");
        assert!(
            errs.iter().any(|e| e.message.contains("invalid supports expression")),
            "expected invalid-expression error, got {errs:?}"
        );
    }

    #[test]
    fn unsupported_language() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cobol]
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        assert!(!field_errors(s, "[language.cobol]").is_empty());
    }

    #[test]
    fn invalid_std_for_language() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
std = "c99"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        assert!(!field_errors(s, "[language.cpp]").is_empty());
    }

    #[test]
    fn no_targets_is_error() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
"#;
        assert!(!field_errors(s, "targets").is_empty());
    }

    #[test]
    fn invalid_warnings() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[compiler]
warnings = "verbose"
"#;
        assert!(!field_errors(s, "[compiler]").is_empty());
    }

    #[test]
    fn opt_level_out_of_range() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[compiler]
opt-level = 5
"#;
        assert!(!field_errors(s, "[compiler]").is_empty());
    }

    #[test]
    fn profile_opt_level_out_of_range() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[profile.dev]
opt-level = 9
"#;
        assert!(!field_errors(s, "[profile.dev]").is_empty());
    }

    #[test]
    fn multiple_bins_valid() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "server"
src  = "src/server.cpp"
[[bin]]
name = "client"
src  = "src/client.cpp"
"#;
        assert!(errors(s).is_empty());
    }

    #[test]
    fn package_name_with_invalid_chars() {
        let s = r#"
[package]
name    = "my project"
version = "0.1.0"
[[bin]]
name = "x"
src  = "src/x.cpp"
"#;
        assert!(!field_errors(s, "[package]").is_empty());
    }

    #[test]
    fn all_errors_reported_at_once() {
        // multiple problems → all should surface, not just the first
        let s = r#"
[package]
name    = ""
version = "bad"
"#;
        let errs = errors(s);
        assert!(errs.len() >= 3, "expected at least 3 errors (name, version, no targets), got {}", errs.len());
    }

    // ── Dependency language compatibility ─────────────────────────────────────

    fn write_dep_manifest(dir: &std::path::Path, lang_key: &str) {
        let content = format!(
            "[package]\nname = \"dep\"\nversion = \"0.1.0\"\n\
             [language.{lang_key}]\n\
             [[bin]]\nname = \"dep\"\nsrc = \"src/main.cpp\"\n"
        );
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("freight.toml"), content).unwrap();
    }

    #[test]
    fn cpp_project_can_use_c_dep() {
        let dir = tempfile::tempdir().unwrap();
        let dep_dir = dir.path().join("mylib");
        write_dep_manifest(&dep_dir, "c");

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
mylib = { path = "mylib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(errs.is_empty(), "cpp project should be able to use a C dep");
    }

    #[test]
    fn c_project_cannot_use_cpp_dep() {
        let dir = tempfile::tempdir().unwrap();
        let dep_dir = dir.path().join("cpplib");
        write_dep_manifest(&dep_dir, "cpp");

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.c]
[[bin]]
name = "foo"
src  = "src/main.c"
[dependencies]
cpplib = { path = "cpplib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(!errs.is_empty(), "c project should not be able to use a C++ dep");
        assert!(errs[0].context.contains("cpplib"));
    }

    #[test]
    fn fortran_project_cannot_use_cpp_dep() {
        let dir = tempfile::tempdir().unwrap();
        let dep_dir = dir.path().join("cpplib");
        write_dep_manifest(&dep_dir, "cpp");

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.fortran]
[[bin]]
name = "foo"
src  = "src/main.f90"
[dependencies]
cpplib = { path = "cpplib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(!errs.is_empty(), "fortran project should not be able to use a C++ dep");
    }

    #[test]
    fn c_project_can_use_fortran_dep() {
        let dir = tempfile::tempdir().unwrap();
        write_dep_manifest(&dir.path().join("numlib"), "fortran");

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.c]
[[bin]]
name = "foo"
src  = "src/main.c"
[dependencies]
numlib = { path = "numlib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        assert!(validate_dep_compat(&manifest, dir.path(), &test_templates()).is_empty());
    }

    #[test]
    fn cpp_project_can_use_fortran_dep() {
        let dir = tempfile::tempdir().unwrap();
        write_dep_manifest(&dir.path().join("numlib"), "fortran");

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
numlib = { path = "numlib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        assert!(validate_dep_compat(&manifest, dir.path(), &test_templates()).is_empty());
    }

    #[test]
    fn fortran_project_can_use_c_dep() {
        let dir = tempfile::tempdir().unwrap();
        write_dep_manifest(&dir.path().join("clib"), "c");

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.fortran]
[[bin]]
name = "foo"
src  = "src/main.f90"
[dependencies]
clib = { path = "clib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        assert!(validate_dep_compat(&manifest, dir.path(), &test_templates()).is_empty());
    }

    #[test]
    fn missing_dep_dir_is_skipped() {
        let dir = tempfile::tempdir().unwrap();

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.c]
[[bin]]
name = "foo"
src  = "src/main.c"
[dependencies]
ghost = { path = "does-not-exist" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(errs.is_empty(), "missing dep dir should be silently skipped");
    }

    #[test]
    fn path_dep_with_non_matching_supports_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let dep_dir = dir.path().join("unsupported");
        std::fs::create_dir_all(&dep_dir).unwrap();
        let unsupported = if std::env::consts::OS == "windows" {
            "linux"
        } else {
            "windows"
        };
        std::fs::write(
            dep_dir.join("freight.toml"),
            format!(r#"
[package]
name = "unsupported"
version = "0.1.0"
supports = "{unsupported}"
[language.c]
[[bin]]
name = "unsupported"
src = "src/main.c"
"#),
        )
        .unwrap();

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.c]
[[bin]]
name = "foo"
src  = "src/main.c"
[dependencies]
unsupported = { path = "unsupported" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(
            errs.iter().any(|e| {
                e.context.contains("unsupported")
                    && e.message.contains("does not support the current platform")
            }),
            "expected dependency supports error, got {errs:?}"
        );
    }

    // ── C/C++ std consistency ─────────────────────────────────────────────────

    #[test]
    fn mixed_c_cpp_with_matching_stds_is_valid() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
std = "c++20"
[language.c]
std = "c17"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        assert!(errors(s).is_empty(), "c17 with c++20 should be valid");
    }

    #[test]
    fn mixed_c_cpp_c_newer_than_cpp_is_error() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
std = "c++17"
[language.c]
std = "c23"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        assert!(!field_errors(s, "[language]").is_empty(), "c23 with c++17 should error");
    }

    #[test]
    fn mixed_c_cpp_one_std_missing_is_error() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
std = "c++20"
[language.c]
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#;
        assert!(!field_errors(s, "[language]").is_empty(), "one std missing should error");
    }

    #[test]
    fn dep_with_newer_cpp_std_than_project_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let dep_content = r#"
[package]
name    = "dep"
version = "0.1.0"
[language.cpp]
std = "c++23"
[[bin]]
name = "dep"
src  = "src/main.cpp"
"#;
        std::fs::create_dir_all(dir.path().join("mylib")).unwrap();
        std::fs::write(dir.path().join("mylib/freight.toml"), dep_content).unwrap();

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
std = "c++17"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
mylib = { path = "mylib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(!errs.is_empty(), "dep with newer std should error");
        assert!(errs[0].message.contains("c++23"));
    }

    #[test]
    fn dep_with_same_or_older_cpp_std_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let dep_content = r#"
[package]
name    = "dep"
version = "0.1.0"
[language.cpp]
std = "c++17"
[[bin]]
name = "dep"
src  = "src/main.cpp"
"#;
        std::fs::create_dir_all(dir.path().join("mylib")).unwrap();
        std::fs::write(dir.path().join("mylib/freight.toml"), dep_content).unwrap();

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.cpp]
std = "c++20"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
mylib = { path = "mylib" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(errs.is_empty(), "dep with older std should be fine");
    }

    #[test]
    fn system_dep_skipped_in_compat_check() {
        let dir = tempfile::tempdir().unwrap();

        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.c]
[[bin]]
name = "foo"
src  = "src/main.c"
[dependencies]
libpng = { system = "libpng", version = ">=1.6" }
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(errs.is_empty(), "system deps should not trigger compat check");
    }

    #[test]
    fn dep_known_os_and_arch_validate_clean() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
pthread = { system = "pthread", os = "linux" }
ws2_32  = { system = "ws2_32",  os = "windows" }
sse_lib = { system = "sse_lib", arch = "x86_64" }
"#;
        assert!(field_errors(s, "[dependencies.").is_empty(), "known os/arch should validate clean");
    }

    #[test]
    fn dep_unknown_os_is_error() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
mylib = { system = "mylib", os = "beos" }
"#;
        let errs = field_errors(s, "[dependencies.mylib]");
        assert!(!errs.is_empty());
        assert!(errs.iter().any(|e| e.message.contains("unknown os value")));
    }

    #[test]
    fn dep_unknown_arch_is_error() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
mylib = { system = "mylib", arch = "pdp11" }
"#;
        let errs = field_errors(s, "[dependencies.mylib]");
        assert!(!errs.is_empty());
        assert!(errs.iter().any(|e| e.message.contains("unknown arch value")));
    }

    #[test]
    fn known_os_arch_keys_validate_clean() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[os.linux.dependencies]
dl = { system = "dl" }
[os.windows.dependencies]
ws2_32 = { system = "ws2_32" }
[os.unix]
defines = ["UNIX_BUILD"]
[arch.x86_64]
defines = ["HAVE_SSE2"]
"#;
        assert!(field_errors(s, "[os.").is_empty());
        assert!(field_errors(s, "[arch.").is_empty());
    }

    #[test]
    fn unknown_os_key_errors() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[os.beos.dependencies]
foo = { system = "foo" }
"#;
        let errs = field_errors(s, "[os.beos]");
        assert!(!errs.is_empty());
        assert!(errs.iter().any(|e| e.message.contains("unknown OS key")));
    }

    #[test]
    fn unknown_arch_key_errors() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[arch.pdp11]
defines = ["RETRO"]
"#;
        let errs = field_errors(s, "[arch.pdp11]");
        assert!(!errs.is_empty());
        assert!(errs.iter().any(|e| e.message.contains("unknown arch key")));
    }
}
