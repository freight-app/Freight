use std::collections::HashSet;
use std::path::Path;

use semver::Version;

use super::types::{
    is_platform_dep, is_unpinned_version, known_arch_keys, known_platform_keys, Dependency,
    DetailedDep, Manifest, Profile,
};
use crate::toolchain::CompilerTemplate;

const VALID_WARNINGS: &[&str] = &["none", "default", "all", "error"];

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub context: String,
    pub message: String,
}

impl ValidationError {
    fn new(ctx: &str, msg: impl Into<String>) -> Self {
        Self {
            context: ctx.to_string(),
            message: msg.into(),
        }
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
    validate_dep_versions(manifest, &mut errors);
    validate_no_platform_deps(manifest, &mut errors);
    validate_cpu_features(manifest, &mut errors);
    validate_patch(manifest, &mut errors);

    errors
}

/// `[patch]` entries override a dependency's source with a local checkout, so
/// each entry must be a `path` dep. Version/git/archive overrides are not (yet)
/// supported — reject them rather than silently ignoring the override.
fn validate_patch(m: &Manifest, errors: &mut Vec<ValidationError>) {
    for (name, dep) in &m.patch {
        let is_path = matches!(dep, Dependency::Detailed(d) if d.path.is_some());
        if !is_path {
            errors.push(ValidationError::new(
                &format!("[patch.{name}]"),
                "a `[patch]` entry must be a path override (e.g. \
                 `{name} = {{ path = \"../{name}\" }}`); version, git, and archive \
                 overrides are not supported",
            ));
        }
    }
}

/// Reject a known CPU feature declared under an `[arch.*]` section it does not
/// belong to (e.g. `[arch.aarch64] features = ["avx2"]` would emit `-mavx2` on
/// ARM). Unknown names fall back to `-m<name>` and are not validated here.
fn validate_cpu_features(m: &Manifest, errors: &mut Vec<ValidationError>) {
    use crate::toolchain::cpu_features::{
        feature_allows_arch, find_cpu_feature, load_cpu_features,
    };
    let table = load_cpu_features();
    for (arch_key, sec) in &m.arch {
        for feat in &sec.features {
            if let Some(cf) = find_cpu_feature(feat, &table) {
                if !feature_allows_arch(cf, arch_key) {
                    errors.push(ValidationError::new(
                        &format!("[arch.{arch_key}]"),
                        format!(
                            "CPU feature '{feat}' is not valid for arch '{arch_key}' (belongs to: {})",
                            cf.arch.as_deref().unwrap_or("any")
                        ),
                    ));
                }
            }
        }
    }
}

/// Reject dependency keys named after an OS/family (`unix`, `windows`, …). These
/// used to link versionless system libraries (`unix = { features = [...] }`); that
/// form is gone — system libraries are now declared with `[os.<name>] features = [...]`.
fn validate_no_platform_deps(m: &Manifest, errors: &mut Vec<ValidationError>) {
    let mut flag = |section: &str, name: &str| {
        if known_platform_keys().contains(&name) || name.eq_ignore_ascii_case("osx") {
            errors.push(ValidationError::new(
                &format!("[{section}.{name}]"),
                format!(
                    "'{name}' is an OS/family name, not a dependency — declare system \
                     libraries with `[os.{name}]` + `features = [...]` instead"
                ),
            ));
        }
    };
    for name in m.dependencies.keys() {
        flag("dependencies", name);
    }
    for name in m.build_dependencies.keys() {
        flag("build-dependencies", name);
    }
    for name in m.dev_dependencies.keys() {
        flag("dev-dependencies", name);
    }
}

/// A version-resolved dependency must carry a concrete version or range — a bare
/// `*` (or empty/omitted version) is rejected, because C/C++ libraries make no
/// SemVer/ABI promise and an unpinned dep would build against an arbitrary
/// version. The version is the same whether the package is already installed
/// (resolved from the system via pkg-config) or downloaded from the registry —
/// "installed" just means freight skips the download. Exempt: `path`/`url`
/// (git/archive) deps, which name an explicit source, and platform pseudo-deps
/// (`windows = { features = … }`).
fn validate_dep_versions(m: &Manifest, errors: &mut Vec<ValidationError>) {
    fn check(section: &str, name: &str, dep: &Dependency, errors: &mut Vec<ValidationError>) {
        if is_platform_dep(name) {
            return;
        }
        let version = match dep {
            Dependency::Simple(v) => Some(v.as_str()),
            Dependency::Detailed(d) => {
                // path / git / archive deps name an explicit source, not a version.
                if d.path.is_some() || d.url.is_some() {
                    return;
                }
                d.version.as_deref()
            }
        };
        let unpinned = version.map(is_unpinned_version).unwrap_or(true);
        if unpinned {
            errors.push(ValidationError::new(
                &format!("[{section}.{name}]"),
                "needs a concrete version or range (e.g. \"1.3\" or \">=1.2\"); a bare `*` is \
                 not allowed because C/C++ libraries change their API between versions. freight \
                 uses the version installed on the system if present, and downloads it from the \
                 registry otherwise.",
            ));
        }
    }
    for (n, d) in &m.dependencies {
        check("dependencies", n, d, errors);
    }
    for (n, d) in &m.build_dependencies {
        check("build-dependencies", n, d, errors);
    }
    for (n, d) in &m.dev_dependencies {
        check("dev-dependencies", n, d, errors);
    }
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
            // `dep:name` activates an optional dependency and `define:NAME[=value]`
            // injects a preprocessor define — neither is a reference to another
            // feature, so skip the known-feature check. `<dep>/define:NAME` (and
            // the weak `<dep>?/define:NAME`) forwards a define into a dependency's
            // build, so it isn't a feature reference either.
            if dep.starts_with("dep:")
                || dep.starts_with("define:")
                || dep
                    .split_once('/')
                    .is_some_and(|(_, rhs)| rhs.trim_start().starts_with("define:"))
            {
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
    let keys: Vec<&String> = m
        .features
        .keys()
        .filter(|k| k.as_str() != "default")
        .collect();
    for start in &keys {
        let mut visited = HashSet::new();
        let mut stack = vec![start.as_str()];
        while let Some(cur) = stack.pop() {
            if cur == "default" {
                continue;
            }
            if !visited.insert(cur) {
                errors.push(ValidationError::new(
                    &format!("[features.{start}]"),
                    format!("feature cycle detected involving '{cur}'"),
                ));
                break;
            }
            if let Some(deps) = m.features.get(cur) {
                for d in deps {
                    stack.push(d.as_str());
                }
            }
        }
    }

    // Validate features requested on dep declarations.
    for (dep_name, dep) in &m.dependencies {
        let Dependency::Detailed(d) = dep else {
            continue;
        };
        for feat in &d.features {
            // We can't validate features on foreign/registry deps (no local manifest to check),
            // but we can catch obviously wrong things if it's a path dep with a loaded manifest.
            // For now, just flag if `default-features = false` with no features listed.
            let _ = (dep_name, feat); // reserved for future cross-manifest checks
        }
    }
}

fn validate_foreign_deps(m: &Manifest, errors: &mut Vec<ValidationError>) {
    for (name, dep) in &m.dependencies {
        let Dependency::Detailed(d) = dep else {
            continue;
        };
        let ctx = format!("[dependencies.{name}]");

        if let Some(repo) = &d.registry {
            if repo.is_empty() {
                errors.push(ValidationError::new(&ctx, "repo must not be empty"));
            }
            let is_version_dep =
                d.version.is_some() && d.path.is_none() && !d.is_git() && d.url.is_none();
            if !is_version_dep {
                errors.push(ValidationError::new(
                    &ctx,
                    "repo is only valid for version deps (no path, git, or url)",
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
    let known_os = known_platform_keys();
    let known_arch = known_arch_keys();
    for key in m.os.keys() {
        let lc = key.to_ascii_lowercase();
        if !known_os.iter().any(|k| *k == lc) {
            errors.push(ValidationError::new(
                &format!("[os.{key}]"),
                format!(
                    "unknown OS key {:?}; expected one of: {}",
                    key,
                    known_os.join(", "),
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
                    key,
                    known_arch.join(", "),
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
            format!(
                "name {:?} contains invalid characters (use letters, digits, - or _)",
                pkg.name
            ),
        ));
    }

    if Version::parse(&pkg.version).is_err() {
        errors.push(ValidationError::new(
            "[package]",
            format!(
                "version {:?} is not valid semver (expected major.minor.patch)",
                pkg.version
            ),
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

fn validate_language(
    m: &Manifest,
    templates: &[CompilerTemplate],
    errors: &mut Vec<ValidationError>,
) {
    for (key, settings) in &m.language {
        let ctx = format!("[language.{key}]");

        let handling: Vec<&CompilerTemplate> = templates
            .iter()
            .filter(|t| t.linking.contains_key(key.as_str()))
            .collect();

        if !templates.is_empty() && handling.is_empty() {
            let mut known: Vec<String> = templates
                .iter()
                .flat_map(|t| t.linking.keys().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            known.sort();
            errors.push(ValidationError::new(
                &ctx,
                format!(
                    "{key:?} is not a supported language key; known keys: {}",
                    known.join(", ")
                ),
            ));
            continue;
        }

        if let Some(std) = &settings.std {
            let valid_stds: HashSet<&str> = handling
                .iter()
                .flat_map(|t| t.standards.keys().map(String::as_str))
                .collect();
            // Empty valid_stds means the language uses no -std= flag; treat any value as docs.
            if !valid_stds.is_empty() && !valid_stds.contains(std.as_str()) {
                let mut sorted: Vec<&str> = valid_stds.into_iter().collect();
                sorted.sort();
                errors.push(ValidationError::new(
                    &ctx,
                    format!(
                        "std {:?} is not valid for {key}; valid standards: {}",
                        std,
                        sorted.join(", ")
                    ),
                ));
            }
        }

        // Validate stdlib (C++) — checked against the union of flags_stdlib keys.
        if key == "cpp" {
            if let Some(stdlib) = &settings.stdlib {
                let valid: HashSet<&str> = handling
                    .iter()
                    .flat_map(|t| t.flags_stdlib.keys().map(String::as_str))
                    .collect();
                if !valid.is_empty() && !valid.contains(stdlib.as_str()) {
                    let mut sorted: Vec<&str> = valid.into_iter().collect();
                    sorted.sort();
                    errors.push(ValidationError::new(
                        &ctx,
                        format!(
                            "stdlib {:?} is not supported; valid values: {}",
                            stdlib,
                            sorted.join(", ")
                        ),
                    ));
                }
            }
        }
    }
}

fn validate_targets(m: &Manifest, errors: &mut Vec<ValidationError>) {
    // A foreign package (`[package].build` set, e.g. a vcpkg-scraper port) has no
    // native targets — it's fetched and built with its own build system.
    let is_foreign = m.package.build.is_some();
    // A project adopted from a foreign build system delegates to a build-system
    // plugin (e.g. `freight init` on a CMake project writes `[build-dependencies]
    // cmake` + `[cmake] build = "<self>"`): the plugin produces the artifacts, so
    // there are no freight-native `[[bin]]`/`[lib]` targets.
    const BUILD_PLUGINS: &[&str] = &["cmake", "make", "meson", "autotools", "scons", "bazel"];
    let delegates_to_plugin = BUILD_PLUGINS.iter().any(|p| {
        m.build_dependencies.contains_key(*p) || m.dependencies.contains_key(*p)
    });
    if m.bins.is_empty() && m.lib.is_none() && !is_foreign && !delegates_to_plugin {
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
        for feat in &bin.required_features {
            if !m.features.contains_key(feat) {
                errors.push(ValidationError::new(
                    &ctx,
                    format!("required-features references unknown feature '{feat}'"),
                ));
            }
        }
    }

    for (i, ex) in m.examples.iter().enumerate() {
        let ctx = format!("[[example]][{i}]");
        if ex.name.is_empty() {
            errors.push(ValidationError::new(&ctx, "name must not be empty"));
        }
        if ex.src.is_empty() {
            errors.push(ValidationError::new(&ctx, "src must not be empty"));
        }
        for feat in &ex.required_features {
            if !m.features.contains_key(feat) {
                errors.push(ValidationError::new(
                    &ctx,
                    format!("required-features references unknown feature '{feat}'"),
                ));
            }
        }
    }

    // `default-run` must name a declared [[bin]].
    if let Some(name) = &m.package.default_run {
        if !m.bins.iter().any(|b| &b.name == name) {
            errors.push(ValidationError::new(
                "[package].default-run",
                format!("'{name}' does not match any [[bin]] target"),
            ));
        }
    }

    if let Some(lib) = &m.lib {
        // An empty `srcs` is allowed: the library is compiled from the
        // auto-discovered `src/` tree. A genuinely source-less build is caught at
        // build time ("no source files found under src/").
        if lib.link.is_some() && !lib.srcs.is_empty() {
            errors.push(ValidationError::new(
                "[lib]",
                "link and srcs are mutually exclusive: prebuilt libraries have no source files",
            ));
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
            format!(
                "warnings {:?} is not valid; choose one of: {}",
                cc.warnings,
                VALID_WARNINGS.join(", ")
            ),
        ));
    }
}

fn validate_profiles(m: &Manifest, errors: &mut Vec<ValidationError>) {
    if let Some(debug) = &m.profile.debug {
        validate_profile(debug, "[profile.debug]", errors);
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
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
    let allowed_abis: HashSet<&str> = project_langs
        .iter()
        .flat_map(|&lang| {
            templates
                .iter()
                .filter_map(move |t| t.linking.get(lang))
                .flat_map(|l| {
                    std::iter::once(l.abi.as_str()).chain(l.compatible.iter().map(String::as_str))
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
        let Ok(src) = std::fs::read_to_string(dep_dir.join("freight.toml")) else {
            continue;
        };
        let Ok(dep_manifest) = toml_edit::de::from_str::<Manifest>(&src) else {
            continue;
        };

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
            let dep_abi = templates
                .iter()
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
                let proj_std = manifest
                    .language
                    .get(dep_lang.as_str())
                    .and_then(|l| l.std.as_deref());
                let dep_std = dep_manifest
                    .language
                    .get(dep_lang.as_str())
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

    let c_std = m.language.get("c").and_then(|l| l.std.as_deref());
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
        "c11" => 11,
        "c17" => 17,
        "c23" => 23,
        "c++11" => 11,
        "c++14" => 14,
        "c++17" => 17,
        "c++20" => 20,
        "c++23" => 23,
        _ => 0,
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

    fn test_templates() -> Vec<CompilerTemplate> {
        crate::toolchain::load_all_templates()
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

[profile.debug]
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
        errors(s)
            .into_iter()
            .filter(|e| e.context.contains(ctx))
            .collect()
    }

    #[test]
    fn required_features_must_be_known() {
        let errs = field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.c]\n\
             [[bin]]\nname=\"x\"\nsrc=\"src/x.c\"\nrequired-features=[\"ghost\"]\n",
            "[[bin]][0]",
        );
        assert!(
            errs.iter().any(|e| e.message.contains("ghost")),
            "expected unknown-feature error, got {errs:?}"
        );
    }

    #[test]
    fn required_features_resolve_when_declared() {
        let errs = field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.c]\n[features]\nextras=[]\n\
             [[bin]]\nname=\"x\"\nsrc=\"src/x.c\"\nrequired-features=[\"extras\"]\n",
            "[[bin]][0]",
        );
        assert!(
            errs.is_empty(),
            "declared feature should be valid: {errs:?}"
        );
    }

    #[test]
    fn example_required_features_must_be_known() {
        let errs = field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.c]\n\
             [lib]\ntype=\"static\"\nsrcs=[\"src/lib.c\"]\n\
             [[example]]\nname=\"e\"\nsrc=\"examples/e.c\"\nrequired-features=[\"ghost\"]\n",
            "[[example]][0]",
        );
        assert!(
            errs.iter().any(|e| e.message.contains("ghost")),
            "expected unknown-feature error, got {errs:?}"
        );
    }

    #[test]
    fn default_run_must_match_a_bin() {
        let errs = field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\ndefault-run=\"nope\"\n[language.c]\n\
             [[bin]]\nname=\"main\"\nsrc=\"src/main.c\"\n",
            "[package].default-run",
        );
        assert!(!errs.is_empty(), "bad default-run should be rejected");
    }

    #[test]
    fn patch_path_override_is_accepted() {
        let errs = field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n\
             [patch]\nfoo = { path = \"../foo\" }\n",
            "[patch.foo]",
        );
        assert!(errs.is_empty(), "path patch should be valid: {errs:?}");
    }

    #[test]
    fn patch_version_override_is_rejected() {
        let errs = field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n\
             [patch]\nfoo = \"1.2\"\n",
            "[patch.foo]",
        );
        assert!(!errs.is_empty(), "version patch should be rejected");
    }

    #[test]
    fn wildcard_dep_version_is_rejected() {
        // Bare `*` (and empty) version is not allowed for a version-resolved dep.
        let errs = field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n[dependencies]\nzlib=\"*\"\n",
            "[dependencies.zlib]",
        );
        assert!(
            errs.iter().any(|e| e.message.contains("concrete version")),
            "expected a concrete-version error, got: {errs:?}"
        );
        // Empty version too.
        assert!(!field_errors(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n[dependencies]\nzlib=\"\"\n",
            "[dependencies.zlib]",
        )
        .is_empty());
    }

    #[test]
    fn concrete_and_sourced_dep_versions_are_accepted() {
        // Concrete version, range, and path/git/url deps are all fine.
        let ok = "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n\
                  [dependencies]\nzlib=\"1.3\"\nfmt=\">=10\"\n\
                  mylib={ path=\"../mylib\" }\n\
                  dep2={ url=\"https://example.com/x.tar.gz\" }\n";
        assert!(
            field_errors(ok, "dependencies").is_empty(),
            "concrete/range/path/url deps must validate: {:?}",
            field_errors(ok, "dependencies")
        );
    }

    #[test]
    fn lints_undeclared_include_defaults_to_warn() {
        let m = load("[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n");
        assert_eq!(
            m.lints.undeclared_include,
            crate::manifest::types::LintLevel::Warn
        );
    }

    #[test]
    fn lints_undeclared_include_parses_levels() {
        use crate::manifest::types::LintLevel;
        let base = "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n";
        let deny = load(&format!("{base}[lints]\nundeclared-include=\"deny\"\n"));
        assert_eq!(deny.lints.undeclared_include, LintLevel::Deny);
        let allow = load(&format!("{base}[lints]\nundeclared-include=\"allow\"\n"));
        assert_eq!(allow.lints.undeclared_include, LintLevel::Allow);
    }

    #[test]
    fn full_manifest_is_valid() {
        assert!(
            errors(FULL_MANIFEST).is_empty(),
            "full manifest should be valid"
        );
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
        let s = format!(
            r#"
[package]
name    = "foo"
version = "0.1.0"
supports = "{platform}"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#
        );

        assert!(field_errors(&s, "[package]").is_empty());
    }

    #[test]
    fn package_supports_rejects_non_matching_platform() {
        let unsupported = if std::env::consts::OS == "windows" {
            "linux"
        } else {
            "windows"
        };
        let s = format!(
            r#"
[package]
name    = "foo"
version = "0.1.0"
supports = "{unsupported}"
[[bin]]
name = "foo"
src  = "src/main.cpp"
"#
        );

        let errs = field_errors(&s, "[package]");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("current platform is not supported")),
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
            errs.iter()
                .any(|e| e.message.contains("invalid supports expression")),
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
[profile.debug]
opt-level = 9
"#;
        assert!(!field_errors(s, "[profile.debug]").is_empty());
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
        assert!(
            errs.len() >= 3,
            "expected at least 3 errors (name, version, no targets), got {}",
            errs.len()
        );
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
        assert!(
            !errs.is_empty(),
            "c project should not be able to use a C++ dep"
        );
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
        assert!(
            !errs.is_empty(),
            "fortran project should not be able to use a C++ dep"
        );
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
        assert!(
            errs.is_empty(),
            "missing dep dir should be silently skipped"
        );
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
            format!(
                r#"
[package]
name = "unsupported"
version = "0.1.0"
supports = "{unsupported}"
[language.c]
[[bin]]
name = "unsupported"
src = "src/main.c"
"#
            ),
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
        assert!(
            !field_errors(s, "[language]").is_empty(),
            "c23 with c++17 should error"
        );
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
        assert!(
            !field_errors(s, "[language]").is_empty(),
            "one std missing should error"
        );
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
libpng = ">=1.6"
"#;
        let manifest = load_manifest_str(s).unwrap();
        let errs = validate_dep_compat(&manifest, dir.path(), &test_templates());
        assert!(
            errs.is_empty(),
            "version deps should not trigger compat check"
        );
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
alsa    = { version = "1", os = "linux" }
winhttp = { version = "1", os = "windows" }
sse_lib = { version = "1", arch = "x86_64" }
"#;
        assert!(
            field_errors(s, "[dependencies.").is_empty(),
            "known os/arch dep filters should validate clean"
        );
    }

    #[test]
    fn cpu_feature_under_wrong_arch_is_rejected() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[arch.aarch64]
features = ["avx2"]
"#;
        let errs = field_errors(s, "[arch.aarch64]");
        assert!(!errs.is_empty(), "avx2 under aarch64 must be rejected");
        assert!(errs.iter().any(|e| e.message.contains("avx2")));
    }

    #[test]
    fn cpu_feature_under_correct_arch_validates() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[arch.x86_64]
features = ["avx2", "fma"]
"#;
        assert!(field_errors(s, "[arch.x86_64]").is_empty());
    }

    #[test]
    fn os_family_dep_key_is_rejected() {
        // The old `unix = { features = [...] }` dep form is gone — it must error
        // and point at `[os.*] features`.
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[[bin]]
name = "foo"
src  = "src/main.cpp"
[dependencies]
unix = { features = ["pthread"] }
"#;
        let errs = field_errors(s, "[dependencies.unix]");
        assert!(!errs.is_empty(), "OS-family dep key must be rejected");
        assert!(errs.iter().any(|e| e.message.contains("[os.unix]")));
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
mylib = { version = "1", os = "beos" }
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
mylib = { version = "1", arch = "pdp11" }
"#;
        let errs = field_errors(s, "[dependencies.mylib]");
        assert!(!errs.is_empty());
        assert!(errs
            .iter()
            .any(|e| e.message.contains("unknown arch value")));
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
[os.linux]
features = ["dl"]
[os.windows]
features = ["ws2_32"]
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
foo = "1"
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
