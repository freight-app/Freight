use std::collections::HashMap;
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
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => format!(".{e}"),
                None => continue,
            };

            if let Some(lang_key) = ext_map.get(ext.as_str()) {
                let rel = path.strip_prefix(project_dir)
                    .unwrap_or(path)
                    .to_path_buf();
                sources.push(SourceFile { path: rel, lang_key: lang_key.clone() });
            }
        }
    }

    // Sort for deterministic build order within each language group.
    sources.sort_by(|a, b| a.path.cmp(&b.path));

    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let inc_dir = project_dir.join("inc");
    if inc_dir.is_dir() {
        include_dirs.push(PathBuf::from("inc"));
    }

    DiscoveredSources { sources, include_dirs }
}

/// Build a map from file extension (e.g. `".cpp"`) to language key (e.g. `"cpp"`).
///
/// Only extensions claimed by languages declared in the manifest are included.
/// When multiple active languages claim the same extension, `LANG_PRIORITY` decides
/// which one wins — more specialised languages (GPU, domain-specific) beat generic ones.
pub(crate) fn build_ext_map(manifest: &Manifest, templates: &[CompilerTemplate]) -> HashMap<String, String> {
    // Specialised languages take priority over generic C/C++ so that e.g. a SYCL
    // project that only declares [language.sycl] gets its .cpp files compiled by icpx.
    const LANG_PRIORITY: &[&str] = &[
        "cuda", "hip", "sycl", "opencl", "ispc", "d", "ada", "fortran", "cpp", "c",
    ];

    let active: std::collections::HashSet<&str> =
        manifest.language.keys().map(String::as_str).collect();

    let mut ext_map: HashMap<String, String> = HashMap::new();

    // Insert in reverse priority order so higher-priority entries overwrite lower ones.
    let ordered: Vec<&&str> = LANG_PRIORITY.iter().rev()
        .filter(|&&k| active.contains(k))
        .collect();

    for &&lang_key in &ordered {
        for template in templates {
            if let Some(linking) = template.linking.get(lang_key) {
                for ext in &linking.extensions {
                    ext_map.insert(ext.clone(), lang_key.to_string());
                }
            }
        }
    }

    // Also handle any active language keys not in LANG_PRIORITY (user-added via custom template).
    for lang_key in active.iter().filter(|k| !LANG_PRIORITY.contains(k)) {
        for template in templates {
            if let Some(linking) = template.linking.get(*lang_key) {
                for ext in &linking.extensions {
                    ext_map.entry(ext.clone()).or_insert_with(|| lang_key.to_string());
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
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../compiler-templates");

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
}
