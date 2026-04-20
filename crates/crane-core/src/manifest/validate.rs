use std::collections::HashSet;
use std::path::Path;

use semver::Version;

use super::types::{Dependency, Manifest, Profile};

const VALID_WARNINGS: &[&str] = &["none", "default", "all", "error"];
const VALID_LANG_KEYS: &[&str] = &["c", "cpp", "fortran", "ada", "d", "cuda", "opencl", "hip", "sycl", "ispc"];

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
/// Returns an empty vec when the manifest is valid.
pub fn validate(manifest: &Manifest) -> Vec<ValidationError> {
    let mut errors: Vec<ValidationError> = Vec::new();

    validate_package(manifest, &mut errors);
    validate_language(manifest, &mut errors);
    validate_targets(manifest, &mut errors);
    validate_compiler(manifest, &mut errors);
    validate_profiles(manifest, &mut errors);

    errors
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
}

fn validate_language(m: &Manifest, errors: &mut Vec<ValidationError>) {
    for (key, settings) in &m.language {
        let ctx = format!("[language.{key}]");
        if !VALID_LANG_KEYS.contains(&key.as_str()) {
            errors.push(ValidationError::new(
                &ctx,
                format!("{key:?} is not a supported language key; choose one of: {}", VALID_LANG_KEYS.join(", ")),
            ));
            continue;
        }
        if let Some(std) = &settings.std {
            if !is_valid_std_for_lang(key, std) {
                errors.push(ValidationError::new(
                    &ctx,
                    format!("std {:?} is not valid for {key}; valid standards: {}", std, valid_stds_for(key)),
                ));
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
        if lib.src.is_empty() {
            errors.push(ValidationError::new("[lib]", "src must not be empty"));
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

/// Check language compatibility of path dependencies against the current project.
///
/// Only path deps are checked — system/registry deps expose a C ABI by convention and
/// are always safe to link. If a dep's `crane.toml` cannot be read it is silently skipped.
pub fn validate_dep_compat(manifest: &Manifest, base_dir: &Path) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    let project_langs: HashSet<&str> =
        manifest.language.keys().map(String::as_str).collect();

    if project_langs.is_empty() {
        return errors;
    }

    let allowed = allowed_dep_langs(&project_langs);

    for (dep_name, dep) in &manifest.dependencies {
        let rel_path = match dep {
            Dependency::Detailed(d) => match d.path.as_deref() {
                Some(p) => p,
                None => continue,
            },
            Dependency::Simple(_) => continue,
        };

        let dep_dir = base_dir.join(rel_path);
        let Ok(src) = std::fs::read_to_string(dep_dir.join("crane.toml")) else { continue };
        let Ok(dep_manifest) = toml_edit::de::from_str::<Manifest>(&src) else { continue };

        for dep_lang in dep_manifest.language.keys() {
            if !allowed.contains(dep_lang.as_str()) {
                errors.push(ValidationError::new(
                    &format!("[dependencies.{dep_name}]"),
                    format!(
                        "library language \"{dep_lang}\" is not compatible with project \
                         language(s) [{}] — languages cannot link across ABI boundaries \
                         without explicit C wrappers",
                        sorted_langs(&project_langs).join(", ")
                    ),
                ));
            }
        }
    }

    errors
}

/// Languages that a project may link against, given its own language set.
///
/// Fortran is linkable from C and C++ (via ABI conventions or `bind(C)` wrappers).
/// Fortran can link C via `iso_c_binding`. Ada and D stay within their own ecosystems
/// except for C interop.
fn allowed_dep_langs<'a>(project_langs: &HashSet<&'a str>) -> HashSet<&'static str> {
    let mut allowed = HashSet::new();
    for &lang in project_langs {
        match lang {
            "c"       => { allowed.extend(["c", "fortran"]); }
            "cpp"     => { allowed.extend(["c", "cpp", "fortran"]); }
            "fortran" => { allowed.extend(["fortran", "c"]); }
            "ada"     => { allowed.extend(["ada", "c"]); }
            "d"       => { allowed.extend(["d", "c", "fortran"]); }
            // GPU languages: host code is C/C++, so all three C-family ABIs are linkable.
            // OpenCL kernels don't directly link CUDA/HIP objects and vice-versa.
            "cuda"    => { allowed.extend(["cuda", "cpp", "c", "fortran"]); }
            "hip"     => { allowed.extend(["hip", "cpp", "c", "fortran"]); }
            "sycl"    => { allowed.extend(["sycl", "cpp", "c", "fortran"]); }
            "opencl"  => { allowed.extend(["opencl", "cpp", "c"]); }
            // ISPC outputs C-callable objects, so C and C++ hosts link it natively.
            "ispc"    => { allowed.extend(["ispc", "cpp", "c"]); }
            _ => {}
        }
    }
    allowed
}

fn sorted_langs<'a>(langs: &HashSet<&'a str>) -> Vec<&'a str> {
    let mut v: Vec<&'a str> = langs.iter().copied().collect();
    v.sort();
    v
}

fn is_valid_std_for_lang(lang: &str, std: &str) -> bool {
    let stds = valid_stds_for(lang);
    if stds == "(any)" { return true; }
    stds.split(", ").any(|s| s == std)
}

fn valid_stds_for(lang_key: &str) -> &'static str {
    match lang_key {
        "c"       => "c11, c17, c23",
        "cpp"     => "c++17, c++20, c++23",
        "fortran" => "f95, f2003, f2008, f2018",
        "ada"     => "ada95, ada2005, ada2012, ada2022",
        "d"       => "(any)",
        "cuda"    => "c++17, c++20",
        "opencl"  => "CL1.0, CL1.1, CL1.2, CL2.0, CL3.0",
        "hip"     => "c++14, c++17, c++20",
        "sycl"    => "c++17, c++20",
        "ispc"    => "(any)",
        _         => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::load_manifest_str;

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
backend   = "auto"
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
        validate(&load(s))
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

    // ── Language std edge cases ───────────────────────────────────────────────

    #[test]
    fn d_language_accepts_any_std() {
        let s = r#"
[package]
name    = "foo"
version = "0.1.0"
[language.d]
std = "2.106"
[[bin]]
name = "foo"
src  = "src/main.d"
"#;
        assert!(errors(s).is_empty(), "D language should accept any std string");
    }

    // ── Dependency language compatibility ─────────────────────────────────────

    fn write_dep_manifest(dir: &std::path::Path, lang_key: &str) {
        let content = format!(
            "[package]\nname = \"dep\"\nversion = \"0.1.0\"\n\
             [language.{lang_key}]\n\
             [[bin]]\nname = \"dep\"\nsrc = \"src/main.cpp\"\n"
        );
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("crane.toml"), content).unwrap();
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
        let errs = validate_dep_compat(&manifest, dir.path());
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
        let errs = validate_dep_compat(&manifest, dir.path());
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
        let errs = validate_dep_compat(&manifest, dir.path());
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
        assert!(validate_dep_compat(&manifest, dir.path()).is_empty());
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
        assert!(validate_dep_compat(&manifest, dir.path()).is_empty());
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
        assert!(validate_dep_compat(&manifest, dir.path()).is_empty());
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
        let errs = validate_dep_compat(&manifest, dir.path());
        assert!(errs.is_empty(), "missing dep dir should be silently skipped");
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
        let errs = validate_dep_compat(&manifest, dir.path());
        assert!(errs.is_empty(), "system deps should not trigger compat check");
    }
}
