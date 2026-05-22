use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::manifest::types::Manifest;
use crate::toolchain::CompilerTemplate;

/// A single compilable source file found under `src/`.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Path relative to the project root (e.g. `src/main.cpp`).
    pub path: PathBuf,
    /// The language key that owns this file (e.g. `"cpp"`, `"cuda"`).
    pub lang_key: String,
}

/// Everything the build engine needs to know about a project's source tree.
#[derive(Debug)]
pub struct DiscoveredSources {
    pub sources: Vec<SourceFile>,
    /// Directories to pass as `-I` flags (e.g. `inc/`).
    pub include_dirs: Vec<PathBuf>,
}

/// Walk `src/` and `inc/` relative to `project_dir`, classify each source file
/// by the active language keys declared in the manifest, and return the result.
///
/// Files whose extension is not claimed by any active language key are silently
/// skipped (headers, data files, etc.).
///
/// When multiple active languages claim the same extension (e.g. `sycl` and `cpp`
/// both handle `.cpp`), the language that appears first in `LANG_PRIORITY` wins.
pub fn discover(
    project_dir: &Path,
    manifest: &Manifest,
    templates: &[CompilerTemplate],
) -> DiscoveredSources {
    let ext_map = build_ext_map(manifest, templates);

    // Build the exclusion set: every file matched by any [os.*] or [arch.*]
    // sources glob. These files are opt-in to a specific platform — the
    // unconditional walk must never include them.
    let exclusion_set = build_exclusion_set(project_dir, manifest);

    let src_dir = project_dir.join("src");
    let mut sources: Vec<SourceFile> = Vec::new();

    if src_dir.is_dir() {
        for entry in WalkDir::new(&src_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            let rel = path.strip_prefix(project_dir).unwrap_or(path).to_path_buf();

            // Skip files claimed by a conditional section.
            if exclusion_set.contains(&rel) {
                continue;
            }

            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => format!(".{e}"),
                None => continue,
            };

            if let Some(lang_key) = ext_map.get(ext.as_str()) {
                sources.push(SourceFile { path: rel, lang_key: lang_key.clone() });
            }
        }
    }

    // Append sources from the matching [os.*] and [arch.*] sections.
    let current_os   = std::env::consts::OS;
    let current_arch = manifest.target.arch.as_deref().unwrap_or(std::env::consts::ARCH);

    for (key, entry) in &manifest.os {
        if key.eq_ignore_ascii_case(current_os) {
            sources.extend(expand_conditional_sources(project_dir, &entry.srcs, &ext_map));
        }
    }
    for (key, entry) in &manifest.arch {
        if key.eq_ignore_ascii_case(current_arch) {
            sources.extend(expand_conditional_sources(project_dir, &entry.srcs, &ext_map));
        }
    }

    // Stable sort + dedup in case globs from different sections overlap.
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    sources.dedup_by(|a, b| a.path == b.path);

    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let inc_dir = project_dir.join("inc");
    if inc_dir.is_dir() {
        include_dirs.push(PathBuf::from("inc"));
    }

    DiscoveredSources { sources, include_dirs }
}

/// Expand all globs from every `[os.*]` and `[arch.*]` sources list into a
/// set of relative paths. Used to exclude these files from the normal walk.
fn build_exclusion_set(project_dir: &Path, manifest: &Manifest) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    let all_globs = manifest.os.values().chain(manifest.arch.values())
        .flat_map(|e| e.srcs.iter());
    for pattern in all_globs {
        for path in glob_sources(project_dir, pattern) {
            set.insert(path);
        }
    }
    set
}

/// Expand a single glob pattern and return matching files as paths relative
/// to `project_dir`, classified by extension via `ext_map`.
fn expand_conditional_sources(
    project_dir: &Path,
    patterns: &[String],
    ext_map: &HashMap<String, String>,
) -> Vec<SourceFile> {
    let mut out = Vec::new();
    for pattern in patterns {
        for rel in glob_sources(project_dir, pattern) {
            let ext = rel.extension().and_then(|e| e.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_default();
            if let Some(lang_key) = ext_map.get(ext.as_str()) {
                out.push(SourceFile { path: rel, lang_key: lang_key.clone() });
            }
        }
    }
    out
}

/// Expand a glob pattern rooted at `project_dir` and return matching paths
/// relative to `project_dir`. Non-matching or invalid patterns yield nothing.
fn glob_sources(project_dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let abs_pattern = project_dir.join(pattern);
    let pattern_str = match abs_pattern.to_str() {
        Some(s) => s.to_owned(),
        None => return vec![],
    };
    let Ok(paths) = glob::glob(&pattern_str) else { return vec![] };
    paths
        .filter_map(|r| r.ok())
        .filter(|p| p.is_file())
        .filter_map(|p| p.strip_prefix(project_dir).ok().map(PathBuf::from))
        .collect()
}

/// Build a map from file extension (e.g. `".cpp"`) to language key (e.g. `"cpp"`).
///
/// Build a map from file extension → language key from all loaded templates.
///
/// Language detection is automatic — `[language.<key>]` sections provide optional
/// configuration but do not gate source discovery. Extensions that are unique to a
/// language (`.asm`, `.cu`, `.hip`, `.f90`, …) are always included so those files
/// are compiled without requiring a manifest declaration.
///
/// Specialised lang_keys that share extensions with generic C/C++ (e.g. `sycl`
/// and `cpp` both handle `.cpp`) require an explicit `[language.<key>]` declaration
/// in the manifest to activate, preventing accidental misclassification.
pub(crate) fn build_ext_map(manifest: &Manifest, templates: &[CompilerTemplate]) -> HashMap<String, String> {
    // Specialised lang_keys that share extensions with generic C/C++ (e.g. sycl/hip use .cpp).
    // These only override the default mapping when explicitly declared in the manifest.
    // cuda/ispc are NOT listed here because .cu/.ispc are unique extensions with no conflict.
    const REQUIRES_DECLARATION: &[&str] = &["sycl", "hip", "opencl"];

    // Priority order for resolving conflicts among always-active languages.
    // Higher index = higher priority (last write wins).
    const LANG_PRIORITY: &[&str] = &[
        "c", "asm", "fortran", "ada", "d", "cpp",
    ];

    let declared: HashSet<&str> = manifest.language.keys().map(String::as_str).collect();

    let mut ext_map: HashMap<String, String> = HashMap::new();

    // Phase 1: insert all always-active languages in priority order.
    // Non-LANG_PRIORITY keys first (low priority, first-write wins).
    let all_always_active: Vec<&str> = templates
        .iter()
        .flat_map(|t| t.linking.keys().map(String::as_str))
        .filter(|k| !REQUIRES_DECLARATION.contains(k))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    for &lang_key in all_always_active.iter().filter(|k| !LANG_PRIORITY.contains(k)) {
        for template in templates {
            if let Some(linking) = template.linking.get(lang_key) {
                for ext in &linking.extensions {
                    ext_map.entry(ext.clone()).or_insert_with(|| lang_key.to_string());
                }
            }
        }
    }

    // LANG_PRIORITY languages in ascending order (higher priority overwrites).
    for &lang_key in LANG_PRIORITY {
        for template in templates {
            if let Some(linking) = template.linking.get(lang_key) {
                for ext in &linking.extensions {
                    ext_map.insert(ext.clone(), lang_key.to_string());
                }
            }
        }
    }

    // Phase 2: apply declared specialised languages on top (they override cpp/c defaults).
    for &lang_key in REQUIRES_DECLARATION {
        if !declared.contains(lang_key) { continue; }
        for template in templates {
            if let Some(linking) = template.linking.get(lang_key) {
                for ext in &linking.extensions {
                    ext_map.insert(ext.clone(), lang_key.to_string());
                }
            }
        }
    }

    ext_map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const TEMPLATES_DIR: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../toolchains");

    fn templates() -> Vec<CompilerTemplate> {
        crate::toolchain::load_templates(std::path::Path::new(TEMPLATES_DIR))
    }

    fn minimal_manifest(lang_key: &str) -> Manifest {
        let src = format!(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n\
             [language.{lang_key}]\n\
             [[bin]]\nname=\"p\"\nsrc=\"src/main.cpp\"\n"
        );
        crate::manifest::load_manifest_str(&src).unwrap()
    }

    #[test]
    fn finds_cpp_sources_in_src() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("main.cpp"), "").unwrap();
        fs::write(src.join("util.cc"), "").unwrap();
        fs::write(src.join("README.md"), "").unwrap();

        let m = minimal_manifest("cpp");
        let found = discover(dir.path(), &m, &templates());

        let paths: Vec<&str> = found.sources.iter()
            .map(|s| s.path.to_str().unwrap())
            .collect();
        assert!(paths.contains(&"src/main.cpp"), "main.cpp not found");
        assert!(paths.contains(&"src/util.cc"), "util.cc not found");
        assert!(!paths.iter().any(|p| p.ends_with(".md")), "md should be skipped");
        assert!(found.sources.iter().all(|s| s.lang_key == "cpp"));
    }

    #[test]
    fn detects_inc_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("inc")).unwrap();

        let m = minimal_manifest("cpp");
        let found = discover(dir.path(), &m, &templates());

        assert!(found.include_dirs.iter().any(|d| d == std::path::Path::new("inc")));
    }

    #[test]
    fn no_inc_dir_means_empty_include_dirs() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();

        let m = minimal_manifest("cpp");
        let found = discover(dir.path(), &m, &templates());
        assert!(found.include_dirs.is_empty());
    }

    #[test]
    fn mixed_c_and_cpp_sources() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("main.cpp"), "").unwrap();
        fs::write(src.join("helper.c"), "").unwrap();

        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.cpp]
[language.c]
[[bin]]
name = "p"
src  = "src/main.cpp"
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let found = discover(dir.path(), &m, &templates());

        let cpp_files: Vec<_> = found.sources.iter().filter(|s| s.lang_key == "cpp").collect();
        let c_files: Vec<_> = found.sources.iter().filter(|s| s.lang_key == "c").collect();
        assert_eq!(cpp_files.len(), 1);
        assert_eq!(c_files.len(), 1);
    }

    #[test]
    fn sources_are_sorted() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("z.cpp"), "").unwrap();
        fs::write(src.join("a.cpp"), "").unwrap();
        fs::write(src.join("m.cpp"), "").unwrap();

        let m = minimal_manifest("cpp");
        let found = discover(dir.path(), &m, &templates());

        let paths: Vec<_> = found.sources.iter().map(|s| s.path.file_name().unwrap()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "sources should be sorted");
    }

    #[test]
    fn missing_src_dir_returns_empty() {
        let dir = tempdir().unwrap();
        let m = minimal_manifest("cpp");
        let found = discover(dir.path(), &m, &templates());
        assert!(found.sources.is_empty());
    }

    #[test]
    fn subdirectories_are_walked() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("src").join("core");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("engine.cpp"), "").unwrap();

        let m = minimal_manifest("cpp");
        let found = discover(dir.path(), &m, &templates());
        assert!(found.sources.iter().any(|s| s.path.ends_with("core/engine.cpp")));
    }

    #[test]
    fn cpp_maps_to_cpp_not_sycl_without_declaration() {
        let manifest = crate::manifest::load_manifest_str(r#"
[package]
name = "p"
version = "0.1.0"
[language.cpp]
[language.c]
[[bin]]
name = "p"
src = "src/main.cpp"
"#).unwrap();
        let ext_map = build_ext_map(&manifest, &templates());
        assert_eq!(ext_map.get(".cpp").map(String::as_str), Some("cpp"),
            ".cpp should map to cpp, got {:?}", ext_map.get(".cpp"));
        assert!(!ext_map.values().any(|v| v == "sycl"),
            "sycl should not appear without [language.sycl]; ext_map: {:?}", ext_map);
    }
}
