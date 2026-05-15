use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use semver;

use super::cache::{freight_home, ToolchainCache};
use super::script::quick_kind;
use super::template::{from_rhai_file_cached, CompilerTemplate};
use crate::error::FreightError;

/// A compiler found on this machine.
#[derive(Debug, Clone)]
pub struct DetectedCompiler {
    pub template: CompilerTemplate,
    pub version: String,
    pub path: PathBuf,
    /// CPU extension names this compiler supports on the current target,
    /// e.g. `["avx2", "sse4.2", "bmi2"]`. Empty when the compiler does not
    /// support `-Q --help=target` (nvcc, nasm, msvc, …).
    pub cpu_extensions: Vec<String>,
}

/// A toolchain family: one or more detected compilers that share a `family` label
/// (e.g., `"gnu"` groups gcc + gfortran; `"llvm"` groups clang + flang).
/// Standalone compilers with `family = ""` and no host requirements each appear as
/// their own entry (e.g. `"nasm"`, `"msvc"`).
#[derive(Debug, Clone)]
pub struct DetectedToolchain {
    /// The family name (e.g., `"gnu"`) or the compiler name for standalone tools.
    pub name: String,
    /// All detected compilers belonging to this toolchain, sorted by template name.
    pub compilers: Vec<DetectedCompiler>,
    /// Sorted union of all language keys handled by any compiler in this toolchain.
    pub languages: Vec<String>,
}

/// Result of grouping detected compilers.
#[derive(Debug, Clone)]
pub struct ToolchainGroups {
    /// Primary toolchains: family groups (gnu, llvm, …) and standalone compilers
    /// that need no host toolchain (tcc, msvc, …).
    /// These are the names accepted by `freight toolchain use`.
    pub toolchains: Vec<DetectedToolchain>,
    /// Extension compilers that require a host toolchain to link
    /// (nvcc, hipcc, ispc, opencl, nasm, yasm…). They extend whichever primary
    /// toolchain is active — they are not selectable directly via `toolchain use`.
    pub guests: Vec<DetectedCompiler>,
}

/// Group a flat list of detected compilers into primary toolchains and extensions.
///
/// Compilers with `requires_toolchain` non-empty are **extensions** — they extend
/// the active toolchain and are collected in `guests`. Everything else is grouped
/// by `family` into `toolchains`, or kept as individual entries when `family` is `""`.
/// Both groups are sorted by name.
pub fn group_into_toolchains(detected: Vec<DetectedCompiler>) -> ToolchainGroups {
    let mut primaries: Vec<DetectedCompiler> = Vec::new();
    let mut guests: Vec<DetectedCompiler> = Vec::new();

    for compiler in detected {
        if !compiler.template.requires_toolchain.is_empty() {
            guests.push(compiler);
        } else {
            primaries.push(compiler);
        }
    }

    let mut map: std::collections::BTreeMap<String, Vec<DetectedCompiler>> =
        std::collections::BTreeMap::new();
    for compiler in primaries {
        let key = if compiler.template.family.is_empty() {
            compiler.template.name.clone()
        } else {
            compiler.template.family.clone()
        };
        map.entry(key).or_default().push(compiler);
    }

    let toolchains = map
        .into_iter()
        .map(|(name, mut compilers)| {
            // Primary sort: compiler name; secondary: newest version first.
            compilers.sort_by(|a, b| {
                a.template.name.cmp(&b.template.name)
                    .then_with(|| version_cmp_desc(&a.version, &b.version))
            });
            let mut languages: Vec<String> = compilers
                .iter()
                .flat_map(|c| c.template.linking.keys().cloned())
                .collect();
            languages.sort_unstable();
            languages.dedup();
            DetectedToolchain { name, compilers, languages }
        })
        .collect();

    guests.sort_by(|a, b| a.template.name.cmp(&b.template.name));

    ToolchainGroups { toolchains, guests }
}

/// Load every `.rhai` file from `templates_dir` and return parsed templates.
pub fn load_templates(templates_dir: &Path) -> Vec<CompilerTemplate> {
    let mut templates = Vec::new();
    for entry in walkdir::WalkDir::new(templates_dir)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rhai") {
            continue;
        }
        // Files starting with '_' are shared includes, not standalone templates.
        if path.file_name().and_then(|n| n.to_str())
            .map(|n| n.starts_with('_')).unwrap_or(false)
        {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        if quick_kind(&src) != "compiler" { continue; }
        match from_rhai_file_cached(path, &src) {
            Ok(t) => templates.push(t),
            Err(e) => eprintln!("warn: skipping {:?}: {e}", path.file_name().unwrap_or_default()),
        }
    }
    templates
}

/// Probe PATH for every template's binary and return those that are present with their version.
/// Each template may yield multiple entries when versioned variants (e.g. `gcc-12`, `gcc-13`)
/// are installed alongside the unversioned name.
pub fn detect_all(templates: &[CompilerTemplate]) -> Vec<DetectedCompiler> {
    let mut found = Vec::new();
    for template in templates {
        found.extend(probe_all(template));
    }
    let mut found = filter_by_toolchain_deps(found);
    found.sort_by(|a, b| {
        a.template.name.cmp(&b.template.name)
            .then_with(|| version_cmp_desc(&a.version, &b.version))
    });
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
        found.extend(probe_all_cached(template, &mut cache, &mut dirty));
    }

    if dirty {
        cache.save();
    }

    let mut found = filter_by_toolchain_deps(found);
    found.sort_by(|a, b| {
        a.template.name.cmp(&b.template.name)
            .then_with(|| version_cmp_desc(&a.version, &b.version))
    });
    found
}

/// Return the major version component of a version string, or 0 for unknown.
fn version_major(v: &str) -> u64 {
    v.split('.').next().and_then(|p| p.parse().ok()).unwrap_or(0)
}

/// Ordering for two version strings: newest first (descending by major, then minor, …).
fn version_cmp_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let av = parse(a);
    let bv = parse(b);
    let len = av.len().max(bv.len());
    for i in 0..len {
        let x = av.get(i).copied().unwrap_or(0);
        let y = bv.get(i).copied().unwrap_or(0);
        match y.cmp(&x) { // reversed: b before a = descending
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

fn probe_all_cached(
    template: &CompilerTemplate,
    cache: &mut ToolchainCache,
    dirty: &mut bool,
) -> Vec<DetectedCompiler> {
    if !host_supported(template) {
        return vec![];
    }
    let paths = which_all(&template.binary);
    if paths.is_empty() {
        return vec![];
    }
    let mut result = Vec::new();
    for path in paths {
        if !requirements_met(template, &path) {
            continue;
        }
        let version = if let Some(v) = cache.get_version(&path) {
            v.to_string()
        } else {
            let v = query_version(template, &path).unwrap_or_else(|| "unknown".into());
            cache.set_version(&path, &v);
            *dirty = true;
            v
        };
        if !min_version_met(template, &version) {
            continue;
        }
        // Only query CPU extensions for primary compilers — guest compilers
        // (nvcc, nasm, hipcc, …) delegate machine code to the host toolchain.
        let cpu_extensions = if template.requires_toolchain.is_empty() {
            if let Some(exts) = cache.get_extensions(&path) {
                exts.to_vec()
            } else {
                let exts = query_cpu_extensions(&path);
                cache.set_extensions(&path, exts.clone());
                *dirty = true;
                exts
            }
        } else {
            vec![]
        };
        result.push(DetectedCompiler { template: template.clone(), version, path, cpu_extensions });
    }
    result
}

fn host_supported(template: &CompilerTemplate) -> bool {
    let arch_ok = template.supported_archs.is_empty()
        || template.supported_archs.iter().any(|a| a == std::env::consts::ARCH);
    let os_ok = template.supported_os.is_empty()
        || template.supported_os.iter().any(|o| o == std::env::consts::OS);
    arch_ok && os_ok
}

/// Check required co-tools and env vars. Emits a warning for each missing item
/// so the user knows why the toolchain was skipped.
fn requirements_met(template: &CompilerTemplate, compiler_path: &Path) -> bool {
    let mut ok = true;
    for tool in &template.required_tools {
        if find_required_tool(tool, compiler_path).is_none() {
            eprintln!(
                "warn: {} found but required tool '{}' is not in PATH \
                 or next to '{}' — installation may be incomplete",
                template.name, tool, template.binary,
            );
            ok = false;
        }
    }
    for var in &template.required_env {
        if std::env::var(var).is_err() {
            eprintln!(
                "warn: {} found but ${} is not set \
                 — run the SDK environment setup script first",
                template.name, var
            );
            ok = false;
        }
    }
    ok
}

fn find_required_tool(tool: &str, compiler_path: &Path) -> Option<PathBuf> {
    which(tool).or_else(|| {
        let sibling = compiler_path.parent()?.join(executable_name(tool));
        sibling.is_file().then_some(sibling)
    })
}

fn executable_name(binary: &str) -> String {
    if cfg!(windows) && !binary.ends_with(".exe") {
        format!("{binary}.exe")
    } else {
        binary.to_string()
    }
}

/// Check `set_min_version`. Emits a warning when the detected version is older.
fn min_version_met(template: &CompilerTemplate, detected: &str) -> bool {
    let Some(min) = &template.min_version else { return true };
    if !version_ge(detected, min) {
        eprintln!(
            "warn: {} {} is older than the required minimum {} \
             — upgrade to use this toolchain",
            template.name, detected, min
        );
        return false;
    }
    true
}

/// Compare two dotted version strings component-by-component.
/// Returns `true` when `a >= b`. Unknown/non-numeric components are treated as 0.
fn version_ge(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let av = parse(a);
    let bv = parse(b);
    let len = av.len().max(bv.len());
    for i in 0..len {
        let x = av.get(i).copied().unwrap_or(0);
        let y = bv.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less    => return false,
            std::cmp::Ordering::Equal   => {}
        }
    }
    true // equal counts as satisfied
}

/// Check the `version` semver requirement declared under `[compiler.<name>]`
/// against the detected compiler version.  Returns an error whose message can
/// be forwarded directly to the user.
///
/// `version_req` is the raw requirement string from `freight.toml`, e.g.
/// `">=14.0"` or `">=14, <16"` (semver range syntax). `detected` is the
/// version string reported by the compiler (e.g. `"14.2.0"` or `"17.0.6-r1"`).
pub fn check_manifest_version_bounds(
    tool_name: &str,
    detected: &str,
    version_req: Option<&str>,
) -> Result<(), FreightError> {
    let Some(req_str) = version_req else { return Ok(()) };

    let req = semver::VersionReq::parse(req_str).map_err(|e| {
        FreightError::OptionError(format!(
            "[compiler.{tool_name}] version = {req_str:?} is not valid semver: {e}"
        ))
    })?;

    // Coerce the detected version string to a semver-compatible form.
    // Compilers often report "14", "14.2", "17.0.6-r1", etc.
    let coerced = coerce_semver(detected);
    let ver = semver::Version::parse(&coerced).map_err(|e| {
        FreightError::OptionError(format!(
            "{tool_name}: cannot parse detected version {detected:?} as semver: {e}"
        ))
    })?;

    if !req.matches(&ver) {
        return Err(FreightError::OptionError(format!(
            "{tool_name} {detected} does not satisfy version requirement {req_str}"
        )));
    }
    Ok(())
}

/// Coerce a compiler-reported version string (e.g. `"14"`, `"17.0.6-r1"`,
/// `"12.3"`) into a clean `MAJOR.MINOR.PATCH` string that `semver` can parse.
fn coerce_semver(s: &str) -> String {
    // Strip any build metadata suffix (anything after a non-numeric, non-dot char
    // following the numeric part — e.g. "-r1", "+debian").
    let numeric: String = s.chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let parts: Vec<&str> = numeric.split('.').collect();
    match parts.as_slice() {
        []           => "0.0.0".into(),
        [major]      => format!("{major}.0.0"),
        [major, minor] => format!("{major}.{minor}.0"),
        [major, minor, patch, ..] => format!("{major}.{minor}.{patch}"),
    }
}

/// Second-pass filter: remove toolchains whose `requires_toolchain` ABI keys are
/// not provided by any other compiler in the detected set.
///
/// This catches guest compilers (nvcc, hipcc, ispc, opencl) whose output
/// cannot be linked into a final binary without a host C/C++ toolchain.
fn filter_by_toolchain_deps(detected: Vec<DetectedCompiler>) -> Vec<DetectedCompiler> {
    let provided: std::collections::HashSet<String> = detected
        .iter()
        .flat_map(|d| d.template.linking.keys().cloned())
        .collect();

    detected
        .into_iter()
        .filter(|d| {
            let unmet: Vec<&str> = d
                .template
                .requires_toolchain
                .iter()
                .filter(|req| !provided.contains(*req))
                .map(String::as_str)
                .collect();
            if !unmet.is_empty() {
                for req in &unmet {
                    eprintln!(
                        "warn: {} requires a '{}' toolchain but none was detected \
                         — install a compatible compiler to use {}",
                        d.template.name, req, d.template.name
                    );
                }
                return false;
            }
            true
        })
        .collect()
}

fn probe_all(template: &CompilerTemplate) -> Vec<DetectedCompiler> {
    if !host_supported(template) {
        return vec![];
    }
    let paths = which_all(&template.binary);
    if paths.is_empty() {
        return vec![];
    }
    let mut result = Vec::new();
    for path in paths {
        if !requirements_met(template, &path) {
            continue;
        }
        let version = query_version(template, &path).unwrap_or_else(|| "unknown".into());
        if !min_version_met(template, &version) {
            continue;
        }
        let cpu_extensions = if template.requires_toolchain.is_empty() {
            query_cpu_extensions(&path)
        } else {
            vec![]
        };
        result.push(DetectedCompiler { template: template.clone(), version, path, cpu_extensions });
    }
    result
}

/// Query the compiler for supported CPU extensions via `-Q --help=target`.
///
/// Parses `-m<name>` flag lines from the output, stripping the `-m` prefix.
/// Value-taking flags (`-march=`, `-mtune=`) are skipped. Returns an empty
/// vec for compilers that don't support this flag (nvcc, nasm, msvc, …).
fn query_cpu_extensions(path: &Path) -> Vec<String> {
    let Ok(output) = Command::new(path)
        .args(["-Q", "--help=target"])
        .output()
    else { return vec![] };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut exts = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("-m") else { continue };
        // First token is the flag name; anything after is the [enabled]/[disabled] annotation.
        let name = rest.split_whitespace().next().unwrap_or("");
        // Skip empty, value-taking (-march=, -mtune=), and non-extension flags.
        if name.is_empty() || name.ends_with('=') { continue; }
        exts.push(name.to_string());
    }

    exts.sort_unstable();
    exts.dedup();
    exts
}

/// Resolve a binary name to its full path by searching PATH (first match only).
/// Used for required-tool lookups; for compiler discovery use [`which_all`].
fn which(binary: &str) -> Option<PathBuf> {
    which_all(binary).into_iter().next()
}

/// Find all installed variants of `binary` in PATH:
///   • the unversioned name (`gcc`)
///   • major-versioned suffixes (`gcc-12`, `gcc-13`, `gcc-14`)
///
/// Results are deduplicated by canonical path so symlinks (e.g. `gcc → gcc-14`)
/// are not reported twice. Within each PATH directory, the unversioned binary is
/// considered before versioned ones; versioned names are sorted descending so
/// the newest appears first.
fn which_all(binary: &str) -> Vec<PathBuf> {
    let Some(path_var) = std::env::var_os("PATH") else { return vec![] };
    let base_name = executable_name(binary);
    let versioned_prefix = format!("{binary}-");

    let mut result: Vec<PathBuf> = Vec::new();
    let mut canonical_seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for dir in std::env::split_paths(&path_var) {
        // Check the unversioned binary first.
        let base = dir.join(&base_name);
        try_add_executable(&base, &mut result, &mut canonical_seen);

        // Scan directory for versioned variants: `{binary}-{N}` where N is a
        // non-empty sequence of ASCII digits (e.g. gcc-12, clang-17).
        let mut versioned: Vec<(u64, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let name = fname.to_string_lossy();

                let Some(suffix) = name.strip_prefix(&versioned_prefix) else { continue };
                // On Windows the suffix may include the .exe extension.
                #[cfg(windows)]
                let suffix = suffix.strip_suffix(".exe").unwrap_or(suffix);

                if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let major: u64 = suffix.parse().unwrap_or(0);
                let path = entry.path();
                if is_executable_file(&path) {
                    versioned.push((major, path));
                }
            }
        }
        // Add versioned variants newest-first.
        versioned.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in versioned {
            try_add_executable(&path, &mut result, &mut canonical_seen);
        }
    }

    result
}

fn is_executable_file(path: &Path) -> bool {
    if path.is_file() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = path.metadata() {
            return meta.permissions().mode() & 0o111 != 0;
        }
    }
    false
}

fn try_add_executable(
    path: &Path,
    result: &mut Vec<PathBuf>,
    canonical_seen: &mut std::collections::HashSet<PathBuf>,
) {
    if !is_executable_file(path) {
        return;
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if canonical_seen.insert(canon) {
        result.push(path.to_path_buf());
    }
}

fn query_version(template: &CompilerTemplate, path: &Path) -> Option<String> {
    let mut cmd = Command::new(path);
    // An empty version_arg means "invoke with no arguments" (e.g. cl.exe prints version on stderr).
    if !template.version_arg.is_empty() {
        cmd.arg(&template.version_arg);
    }
    let output = cmd.output().ok()?;

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
    fn required_tools_can_live_next_to_compiler() {
        let dir = tempfile::tempdir().expect("temp dir");
        let compiler = dir.path().join(executable_name("nvcc"));
        let helper = dir.path().join(executable_name("ptxas"));
        std::fs::write(&compiler, "").expect("compiler marker");
        std::fs::write(&helper, "").expect("helper marker");

        assert_eq!(
            find_required_tool("ptxas", &compiler).as_deref(),
            Some(helper.as_path())
        );
    }

    // ── version_ge ────────────────────────────────────────────────────────────

    #[test]
    fn version_ge_equal() {
        assert!(version_ge("14.2.0", "14.2.0"));
        assert!(version_ge("12.0", "12.0"));
    }

    #[test]
    fn version_ge_newer() {
        assert!(version_ge("14.2.0", "14.1.0"));
        assert!(version_ge("15.0.0", "14.2.0"));
        assert!(version_ge("14.2.1", "14.2.0"));
        assert!(version_ge("12.1", "12.0"));
    }

    #[test]
    fn version_ge_older() {
        assert!(!version_ge("13.0.0", "14.0.0"));
        assert!(!version_ge("14.1.9", "14.2.0"));
        assert!(!version_ge("11.8", "12.0"));
    }

    #[test]
    fn version_ge_padding() {
        // "12" treated as "12.0.0"
        assert!(version_ge("12", "12.0"));
        assert!(version_ge("12.0", "12"));
        assert!(!version_ge("11", "12.0"));
    }

    // ── load / detect ─────────────────────────────────────────────────────────

    #[test]
    fn load_templates_finds_all() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        assert_eq!(templates.len(), 23,
            "expected g++, gcc, gfortran, clang++, clang, flang, \
             gdc, icpx, ifx, ispc, hipcc, nvcc, nvc++, nvc, nvfortran, \
             gas, nasm, yasm, dmd, ldc2, msvc, opencl, tcc");
    }

    #[test]
    fn all_templates_have_required_fields() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        for t in &templates {
            assert!(!t.name.is_empty(), "empty name");
            assert!(!t.binary.is_empty(), "{}: empty binary", t.name);
            assert!(!t.extensions.is_empty(), "{}: no extensions", t.name);
            assert!(!t.version_regex.is_empty(), "{}: empty version_regex", t.name);
            // version_arg may be empty — means "invoke the binary with no arguments"
            // (e.g. cl.exe prints its version on stderr when called with no args).
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
    fn guest_compilers_declare_requires_toolchain() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        for name in &["nvcc", "hipcc", "opencl", "ispc"] {
            let t = templates.iter().find(|t| t.name == *name)
                .unwrap_or_else(|| panic!("{name} template not found"));
            assert!(
                t.requires_toolchain.contains(&"cpp".to_string()),
                "{name} should declare requires_toolchain [\"cpp\"]"
            );
        }
    }

    #[test]
    fn detected_compilers_have_non_empty_version() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        for d in detect_all(&templates) {
            assert!(!d.version.is_empty(), "{} reported empty version", d.template.name);
            assert!(d.path.exists(), "{} path does not exist: {:?}", d.template.name, d.path);
        }
    }

    // ── group_into_toolchains ─────────────────────────────────────────────────

    fn fake_detected_from_templates(templates: &[CompilerTemplate]) -> Vec<DetectedCompiler> {
        templates.iter().map(|t| DetectedCompiler {
            template: t.clone(),
            version: "0.0.0".into(),
            path: std::path::PathBuf::from(format!("/usr/bin/{}", t.name)),
            cpu_extensions: vec![],
        }).collect()
    }

    #[test]
    fn group_into_toolchains_merges_gnu_family() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let detected = fake_detected_from_templates(&templates);
        let groups = group_into_toolchains(detected);

        let gnu = groups.toolchains.iter().find(|tc| tc.name == "gnu")
            .expect("gnu toolchain should exist");
        let names: Vec<&str> = gnu.compilers.iter().map(|c| c.template.name.as_str()).collect();
        assert!(names.contains(&"g++"),  "gnu should contain g++");
        assert!(names.contains(&"gcc"),  "gnu should contain gcc");
        assert!(names.contains(&"gfortran"), "gnu should contain gfortran");
        assert!(gnu.languages.contains(&"cpp".to_string()), "gnu covers cpp");
        assert!(gnu.languages.contains(&"c".to_string()), "gnu covers c");
        assert!(gnu.languages.contains(&"fortran".to_string()), "gnu covers fortran");
    }

    #[test]
    fn group_into_toolchains_merges_llvm_family() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let detected = fake_detected_from_templates(&templates);
        let groups = group_into_toolchains(detected);

        let llvm = groups.toolchains.iter().find(|tc| tc.name == "llvm")
            .expect("llvm toolchain should exist");
        let names: Vec<&str> = llvm.compilers.iter().map(|c| c.template.name.as_str()).collect();
        assert!(names.contains(&"clang++"), "llvm should contain clang++");
        assert!(names.contains(&"clang"),   "llvm should contain clang");
        assert!(names.contains(&"flang"),     "llvm should contain flang");
        assert!(llvm.languages.contains(&"cpp".to_string()),     "llvm covers cpp");
        assert!(llvm.languages.contains(&"c".to_string()),       "llvm covers c");
        assert!(llvm.languages.contains(&"fortran".to_string()), "llvm covers fortran");
    }

    #[test]
    fn group_into_toolchains_guests_are_separated() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let detected = fake_detected_from_templates(&templates);
        let groups = group_into_toolchains(detected);

        // Guest compilers (requires_toolchain non-empty) must not appear in toolchains.
        for name in &["nvcc", "hipcc", "opencl", "ispc"] {
            assert!(
                !groups.toolchains.iter().any(|tc| {
                    tc.compilers.iter().any(|c| c.template.name == *name)
                }),
                "{name} (guest) should not appear in a primary toolchain"
            );
            assert!(
                groups.guests.iter().any(|g| g.template.name == *name),
                "{name} should appear in guests list"
            );
        }
    }

    #[test]
    fn group_into_toolchains_standalone_primaries_own_entry() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let detected = fake_detected_from_templates(&templates);
        let groups = group_into_toolchains(detected);

        // Standalone primaries (family = "", requires_toolchain = []) get their own entry.
        for name in &["msvc", "tcc"] {
            assert!(
                groups.toolchains.iter().any(|tc| tc.name == *name),
                "standalone primary {name} should have its own toolchain entry"
            );
        }
        // Extensions (requires_toolchain non-empty) must not appear as toolchains.
        for name in &["opencl", "nasm", "yasm", "nvcc", "hipcc", "ispc"] {
            assert!(
                !groups.toolchains.iter().any(|tc| tc.name == *name),
                "{name} (extension) must not appear as a primary toolchain"
            );
        }
    }

    #[test]
    fn group_into_toolchains_assemblers_are_extensions() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let detected = fake_detected_from_templates(&templates);
        let groups = group_into_toolchains(detected);

        for name in &["nasm", "yasm"] {
            assert!(
                groups.guests.iter().any(|g| g.template.name == *name),
                "{name} should appear in the extensions list"
            );
            let t = templates.iter().find(|t| t.name == *name).unwrap();
            assert!(
                !t.requires_toolchain.is_empty(),
                "{name} should declare requires_toolchain"
            );
        }
    }

    #[test]
    fn group_into_toolchains_sorted_by_name() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let detected = fake_detected_from_templates(&templates);
        let groups = group_into_toolchains(detected);
        let names: Vec<&str> = groups.toolchains.iter().map(|tc| tc.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "toolchains should be sorted by name");
    }

    // ── toolchain_use ─────────────────────────────────────────────────────────

    #[test]
    fn toolchain_use_accepts_family_names() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        // Family names should be accepted (they reference a group, not a binary).
        // We can't actually persist without a real home dir, but we can check validation.
        // toolchain_use returns Ok only if name is valid, then tries to save — the save
        // may fail without a home dir, but validation alone is what we're testing here.
        // Just confirm no TemplateError is returned for a known family.
        let err = super::super::super::toolchain::toolchain_use("gnu", &templates);
        // Either Ok or a non-TemplateError (e.g. Io error saving config) is acceptable.
        if let Err(e) = err {
            assert!(
                !format!("{e}").contains("unknown toolchain"),
                "family name 'gnu' should be accepted, got: {e}"
            );
        }
    }

    #[test]
    fn toolchain_use_rejects_individual_compiler_with_family() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        // "g++" has family "gnu", so it should be rejected — use "gnu" instead.
        let result = super::super::super::toolchain::toolchain_use("g++", &templates);
        assert!(result.is_err(), "'g++' (has family 'gnu') should not be a valid toolchain name");
    }

    #[test]
    fn toolchain_use_accepts_standalone_primary() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        // "tcc" has family = "", requires_toolchain = [], role = Toolchain → valid.
        let err = super::super::super::toolchain::toolchain_use("tcc", &templates);
        if let Err(e) = err {
            assert!(
                !format!("{e}").contains("unknown toolchain"),
                "standalone 'tcc' should be accepted, got: {e}"
            );
        }
    }

    #[test]
    fn toolchain_use_rejects_assembler() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        // Assemblers are auto-selected, not user-selectable.
        let result = super::super::super::toolchain::toolchain_use("nasm", &templates);
        assert!(result.is_err(), "'nasm' (assembler) should not be a valid toolchain use target");
        let result2 = super::super::super::toolchain::toolchain_use("yasm", &templates);
        assert!(result2.is_err(), "'yasm' (assembler) should not be a valid toolchain use target");
    }

    #[test]
    fn toolchain_use_rejects_unknown_name() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let result = super::super::super::toolchain::toolchain_use("badname", &templates);
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("unknown toolchain"));
    }

    // ── which_all ─────────────────────────────────────────────────────────────

    #[test]
    fn which_all_finds_versioned_binaries() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let make_exec = |name: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, "#!/bin/sh").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };

        let _base = make_exec("mycc");
        let _v12  = make_exec("mycc-12");
        let _v13  = make_exec("mycc-13");
        let _v14  = make_exec("mycc-14");
        // Should not be picked up (not a plain integer suffix).
        let _skip = make_exec("mycc-old");

        let orig_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf())
                .chain(std::env::split_paths(&orig_path))
        ).unwrap();
        std::env::set_var("PATH", &new_path);

        let found = which_all("mycc");
        std::env::set_var("PATH", &orig_path);

        let names: Vec<String> = found.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"mycc".to_string()),    "base binary must be found");
        assert!(names.contains(&"mycc-12".to_string()), "mycc-12 must be found");
        assert!(names.contains(&"mycc-13".to_string()), "mycc-13 must be found");
        assert!(names.contains(&"mycc-14".to_string()), "mycc-14 must be found");
        assert!(!names.contains(&"mycc-old".to_string()), "non-numeric suffix must be ignored");
    }

    #[test]
    fn which_all_deduplicates_symlinks() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let real = dir.path().join("mycc-14");
        std::fs::write(&real, "#!/bin/sh").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Create a symlink: mycc → mycc-14
        std::os::unix::fs::symlink(&real, dir.path().join("mycc")).unwrap();

        let orig_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf())
                .chain(std::env::split_paths(&orig_path))
        ).unwrap();
        std::env::set_var("PATH", &new_path);

        let found = which_all("mycc");
        std::env::set_var("PATH", &orig_path);

        // The symlink and real binary resolve to the same canonical path,
        // so we should get exactly one entry.
        assert_eq!(found.len(), 1, "symlink and target must be deduplicated; got: {found:?}");
    }

    // ── parse_versioned_name / backend_matches ────────────────────────────────

    #[test]
    fn parse_versioned_name_parses_correctly() {
        assert_eq!(parse_versioned_name("gnu-14"), Some(("gnu", 14)));
        assert_eq!(parse_versioned_name("llvm-18"), Some(("llvm", 18)));
        assert_eq!(parse_versioned_name("gnu"),     None);
        assert_eq!(parse_versioned_name("gnu-old"), None);
        assert_eq!(parse_versioned_name("-14"),     None);
        assert_eq!(parse_versioned_name(""),        None);
    }

    #[test]
    fn backend_matches_family_and_versioned() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let gcc = templates.iter().find(|t| t.name == "gcc").expect("gcc template");
        let detected = DetectedCompiler {
            template: gcc.clone(),
            version: "14.2.0".into(),
            path: PathBuf::from("/usr/bin/gcc-14"),
            cpu_extensions: vec![],
        };

        assert!(backend_matches(&detected, "gcc"),    "exact name match");
        assert!(backend_matches(&detected, "gnu"),    "family match");
        assert!(backend_matches(&detected, "gnu-14"), "version-pinned match");
        assert!(!backend_matches(&detected, "gnu-13"), "wrong major version");
        assert!(!backend_matches(&detected, "llvm"),   "wrong family");
    }

    #[test]
    fn toolchain_use_accepts_versioned_family_names() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        // "gnu-14" means family "gnu" with major version 14 — valid because "gnu" is a known family.
        let result = super::super::super::toolchain::toolchain_use("gnu-14", &templates);
        if let Err(e) = result {
            assert!(
                !format!("{e}").contains("unknown toolchain"),
                "versioned family 'gnu-14' should be accepted, got: {e}"
            );
        }
    }

    #[test]
    fn toolchain_use_rejects_versioned_unknown_family() {
        let templates = load_templates(Path::new(TEMPLATES_DIR));
        let result = super::super::super::toolchain::toolchain_use("nofamily-14", &templates);
        assert!(result.is_err(), "'nofamily-14' (unknown family) should be rejected");
    }
}

/// Resolve the compiler templates directory.
/// The user-local templates directory: `~/.freight/templates/`.
///
/// Returns `None` when the freight home directory cannot be determined. The
/// directory does not need to exist — it is created by [`toolchain_add`].
pub fn user_templates_dir() -> Option<PathBuf> {
    Some(freight_home()?.join("templates"))
}

/// Load templates from both the system templates directory and the user's
/// `~/.freight/templates/` directory. User templates override system templates
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

/// Install a compiler template from a local `.rhai` file into `~/.freight/templates/`.
///
/// The script is validated before copying. If a template with the same name
/// already exists it is overwritten. Returns the path the template was written to.
pub fn toolchain_add(rhai_path: &Path) -> Result<PathBuf, FreightError> {
    if rhai_path.extension().and_then(|e| e.to_str()) != Some("rhai") {
        return Err(FreightError::TemplateError(
            "toolchain file must have a .rhai extension".into(),
        ));
    }

    let src = std::fs::read_to_string(rhai_path).map_err(FreightError::Io)?;
    let template = CompilerTemplate::from_rhai_file(rhai_path)?;

    let user_dir = user_templates_dir()
        .ok_or_else(|| FreightError::TemplateError("cannot determine ~/.freight directory".into()))?;

    std::fs::create_dir_all(&user_dir).map_err(FreightError::Io)?;

    let dest = user_dir.join(format!("{}.rhai", template.name));
    std::fs::write(&dest, &src).map_err(FreightError::Io)?;

    Ok(dest)
}

/// Set the global default compiler backend, stored in `~/.freight/config.toml`.
///
/// `name` accepts:
/// - a family name (e.g. `"gnu"`, `"llvm"`)
/// - a version-pinned family name (e.g. `"gnu-14"`, `"llvm-18"`)
/// - a standalone compiler name (e.g. `"tcc"`, `"msvc"`)
///
/// Version-pinned names activate the specific installed version of that family.
/// Prints a warning when the requested toolchain is not currently on PATH, but
/// still saves the preference so it applies once the compiler is installed.
pub fn toolchain_use(name: &str, templates: &[CompilerTemplate]) -> Result<(), FreightError> {
    // Build the set of valid toolchain names: distinct non-empty family names +
    // individual names for templates that have no family.
    // Only primary compilers (requires_toolchain empty) are selectable.
    let mut families: Vec<&str> = templates
        .iter()
        .filter(|t| t.requires_toolchain.is_empty() && !t.family.is_empty())
        .map(|t| t.family.as_str())
        .collect();
    families.sort_unstable();
    families.dedup();

    let standalone: Vec<&str> = templates
        .iter()
        .filter(|t| t.requires_toolchain.is_empty() && t.family.is_empty())
        .map(|t| t.name.as_str())
        .collect();

    // Accept plain family/standalone names and version-pinned variants "{family}-{N}".
    let valid = families.contains(&name)
        || standalone.contains(&name)
        || parse_versioned_name(name)
            .map(|(family, _)| families.contains(&family))
            .unwrap_or(false);

    if !valid {
        let mut known: Vec<String> = families.iter().map(|s| s.to_string()).collect();
        known.extend(standalone.iter().map(|s| s.to_string()));
        known.sort_unstable();
        return Err(FreightError::TemplateError(format!(
            "unknown toolchain {:?}; known toolchains: {}",
            name,
            known.join(", "),
        )));
    }

    let mut config = super::cache::GlobalConfig::load();
    config.default_backend = Some(name.to_string());
    config.save()?;
    Ok(())
}

/// Parse a version-pinned toolchain name like `"gnu-14"` into `("gnu", 14)`.
/// Returns `None` when the name doesn't match the `{family}-{major}` pattern.
pub fn parse_versioned_name(name: &str) -> Option<(&str, u64)> {
    let (family, major_str) = name.rsplit_once('-')?;
    if family.is_empty() || major_str.is_empty() {
        return None;
    }
    if !major_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let major: u64 = major_str.parse().ok()?;
    Some((family, major))
}

/// Returns true when `detected` matches the backend `name`.
///
/// Matches on:
/// - exact template name (`"gcc"`)
/// - family name (`"gnu"`)
/// - version-pinned family name (`"gnu-14"` ↔ a gnu-family compiler at major version 14)
pub fn backend_matches(detected: &DetectedCompiler, name: &str) -> bool {
    if detected.template.name == name {
        return true;
    }
    let effective_family = if detected.template.family.is_empty() {
        detected.template.name.as_str()
    } else {
        detected.template.family.as_str()
    };
    if effective_family == name {
        return true;
    }
    if let Some((family, major)) = parse_versioned_name(name) {
        if effective_family == family && version_major(&detected.version) == major {
            return true;
        }
    }
    false
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

    // cargo dev layout: target/debug/freight → workspace root is two levels up
    let candidate2 = bin_dir.join("..").join("..").join("toolchains");
    let candidate2 = candidate2.canonicalize().ok()?;
    if candidate2.is_dir() {
        return Some(candidate2);
    }

    None
}
