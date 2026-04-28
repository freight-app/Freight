use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

use super::cache::{crane_home, ToolchainCache};
use super::template::CompilerTemplate;
use crate::error::CraneError;

/// A compiler found on this machine.
#[derive(Debug, Clone)]
pub struct DetectedCompiler {
    pub template: CompilerTemplate,
    pub version: String,
    pub path: PathBuf,
}

/// Load every `.rhai` file from `templates_dir` and return parsed templates.
pub fn load_templates(templates_dir: &Path) -> Vec<CompilerTemplate> {
    let Ok(entries) = std::fs::read_dir(templates_dir) else {
        return vec![];
    };

    let mut templates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rhai") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        match CompilerTemplate::from_rhai(&src) {
            Ok(t) => templates.push(t),
            Err(e) => eprintln!("warn: skipping {:?}: {e}", path.file_name().unwrap_or_default()),
        }
    }
    templates
}

/// Probe PATH for every template's binary and return those that are present with their version.
pub fn detect_all(templates: &[CompilerTemplate]) -> Vec<DetectedCompiler> {
    let mut found = Vec::new();
    for template in templates {
        if let Some(detected) = probe(template) {
            found.push(detected);
        }
    }
    found.sort_by(|a, b| a.template.name.cmp(&b.template.name));
    found
}

/// Like [`detect_all`] but reads and writes a persistent version cache so that
/// `--version` is only invoked when a compiler binary has changed on disk.
pub fn detect_all_cached(templates: &[CompilerTemplate]) -> Vec<DetectedCompiler> {
    let mut cache = ToolchainCache::load();
    cache.evict_missing();
    let mut dirty = false;
    let mut found = Vec::new();

    for template in templates {
        if let Some(detected) = probe_cached(template, &mut cache, &mut dirty) {
            found.push(detected);
        }
    }

    if dirty {
        cache.save();
    }

    found.sort_by(|a, b| a.template.name.cmp(&b.template.name));
    found
}

fn probe_cached(
    template: &CompilerTemplate,
    cache: &mut ToolchainCache,
    dirty: &mut bool,
) -> Option<DetectedCompiler> {
    if !arch_supported(template) {
        return None;
    }
    let path = which(&template.binary)?;
    let version = if let Some(v) = cache.get_version(&path) {
        v.to_string()
    } else {
        let v = query_version(template, &path).unwrap_or_else(|| "unknown".into());
        cache.set_version(&path, &v);
        *dirty = true;
        v
    };
    Some(DetectedCompiler { template: template.clone(), version, path })
}

fn arch_supported(template: &CompilerTemplate) -> bool {
    template.supported_archs.is_empty()
        || template.supported_archs.iter().any(|a| a == std::env::consts::ARCH)
}

fn probe(template: &CompilerTemplate) -> Option<DetectedCompiler> {
    if !arch_supported(template) {
        return None;
    }
    let path = which(&template.binary)?;
    let version = query_version(template, &path).unwrap_or_else(|| "unknown".into());
    Some(DetectedCompiler {
        template: template.clone(),
        version,
        path,
    })
}

/// Resolve a binary name to its full path by searching PATH.
fn which(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        // On some systems the binary might not have an extension check needed
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if candidate.exists() {
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    None
}

fn query_version(template: &CompilerTemplate, path: &Path) -> Option<String> {
    let output = Command::new(path)
        .arg(&template.version_arg)
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Some compilers (gfortran) print version to stderr
    let combined = format!("{stdout}{stderr}");

    let re = Regex::new(&template.version_regex).ok()?;
    re.captures(&combined)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATES_DIR: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../toolchains");

    #[test]
    fn load_templates_finds_all() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        assert_eq!(templates.len(), 18,
            "expected gcc, clang, gfortran, gnat, nvcc, dmd, opencl, hipcc, icpx, ispc, nasm, \
             tcc, nvhpc, ifx, flang, ldc2, yasm, circle");
    }

    #[test]
    fn all_templates_have_required_fields() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        for t in &templates {
            assert!(!t.name.is_empty(), "empty name");
            assert!(!t.binary.is_empty(), "{}: empty binary", t.name);
            assert!(!t.extensions.is_empty(), "{}: no extensions", t.name);
            assert!(!t.version_regex.is_empty(), "{}: empty version_regex", t.name);
            assert!(!t.version_arg.is_empty(), "{}: empty version_arg", t.name);
        }
    }

    #[test]
    fn load_templates_missing_dir_returns_empty() {
        let templates = load_templates(Path::new("/nonexistent/path/toolchains"));
        assert!(templates.is_empty());
    }

    #[test]
    fn detect_all_result_is_sorted_by_name() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let detected = detect_all(&templates);
        let names: Vec<&str> = detected.iter().map(|d| d.template.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "detect_all should return compilers in alphabetical order");
    }

    #[test]
    fn detected_compilers_have_non_empty_version() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        for d in detect_all(&templates) {
            assert!(!d.version.is_empty(), "{} reported empty version", d.template.name);
            assert!(d.path.exists(), "{} path does not exist: {:?}", d.template.name, d.path);
        }
    }
}

/// Resolve the compiler templates directory.
/// The user-local templates directory: `~/.crane/templates/`.
///
/// Returns `None` when the crane home directory cannot be determined. The
/// directory does not need to exist — it is created by [`toolchain_add`].
pub fn user_templates_dir() -> Option<PathBuf> {
    Some(crane_home()?.join("templates"))
}

/// Load templates from both the system templates directory and the user's
/// `~/.crane/templates/` directory. User templates override system templates
/// with the same `name` field, enabling local customisation without touching
/// the system installation.
pub fn load_all_templates() -> Vec<CompilerTemplate> {
    let mut templates: Vec<CompilerTemplate> = Vec::new();

    if let Some(system_dir) = templates_dir() {
        templates.extend(load_templates(&system_dir));
    }

    if let Some(user_dir) = user_templates_dir() {
        for t in load_templates(&user_dir) {
            if let Some(pos) = templates.iter().position(|s| s.name == t.name) {
                templates[pos] = t; // user template overrides system template
            } else {
                templates.push(t);
            }
        }
    }

    templates
}

/// Install a compiler template from a local `.rhai` file into `~/.crane/templates/`.
///
/// The script is validated before copying. If a template with the same name
/// already exists it is overwritten. Returns the path the template was written to.
pub fn toolchain_add(rhai_path: &Path) -> Result<PathBuf, CraneError> {
    if rhai_path.extension().and_then(|e| e.to_str()) != Some("rhai") {
        return Err(CraneError::TemplateError(
            "toolchain file must have a .rhai extension".into(),
        ));
    }

    let src = std::fs::read_to_string(rhai_path).map_err(CraneError::Io)?;
    let template = CompilerTemplate::from_rhai(&src)?;

    let user_dir = user_templates_dir()
        .ok_or_else(|| CraneError::TemplateError("cannot determine ~/.crane directory".into()))?;

    std::fs::create_dir_all(&user_dir).map_err(CraneError::Io)?;

    let dest = user_dir.join(format!("{}.rhai", template.name));
    std::fs::write(&dest, &src).map_err(CraneError::Io)?;

    Ok(dest)
}

/// Checks (in order):
///   1. `CRANE_TEMPLATES_DIR` env var
///   2. `{binary_dir}/toolchains/`
///   3. `{binary_dir}/../../toolchains/`  (cargo dev layout)
pub fn templates_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CRANE_TEMPLATES_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }

    let exe = std::env::current_exe().ok()?;
    let bin_dir = exe.parent()?;

    let candidate1 = bin_dir.join("toolchains");
    if candidate1.is_dir() {
        return Some(candidate1);
    }

    // cargo dev layout: target/debug/crane → workspace root is two levels up
    let candidate2 = bin_dir.join("..").join("..").join("toolchains");
    let candidate2 = candidate2.canonicalize().ok()?;
    if candidate2.is_dir() {
        return Some(candidate2);
    }

    None
}
