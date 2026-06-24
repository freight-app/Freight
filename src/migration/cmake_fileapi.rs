//! Extract a project's resolved build data from CMake's File API.
//!
//! Flow: drop a `codemodel-v2` query under a throwaway build dir, run `cmake`
//! configure (CMake's own evaluation — no script parsing), then read the JSON
//! "codemodel" reply: per-target sources, defines, include dirs, and the language
//! standard. See <https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html>.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::error::FreightError;

/// What kind of artifact a target produces (the subset freight can represent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    Executable,
    StaticLib,
    SharedLib,
    /// Anything else (utility, interface library, object library, …) — ignored.
    Other,
}

/// One CMake target's build data, distilled to what a freight manifest needs.
#[derive(Debug, Clone)]
pub struct CmakeTarget {
    pub name: String,
    pub kind: TargetKind,
    /// Compiled source files, project-relative (generated / external are dropped).
    pub sources: Vec<String>,
    /// Preprocessor defines (`NAME` or `NAME=value`), unioned over compile groups.
    pub defines: Vec<String>,
    /// Include directories under the project, project-relative.
    pub includes: Vec<String>,
    /// Language standard digits, e.g. `"17"`.
    pub std: Option<String>,
    /// `"C"` or `"CXX"` — the target's dominant compiled language.
    pub language: Option<String>,
}

/// The extracted model: every representable target in the project.
pub struct CmakeModel {
    pub targets: Vec<CmakeTarget>,
}

/// Configure `project_dir` with CMake's File API and return its build model.
/// Uses a throwaway build directory that is removed before returning.
pub fn extract(project_dir: &Path) -> Result<CmakeModel, FreightError> {
    let build = unique_build_dir();
    let query_dir = build.join(".cmake/api/v1/query");
    fs::create_dir_all(&query_dir)?;
    // An empty `codemodel-v2` file requests the codemodel object, version 2.
    fs::write(query_dir.join("codemodel-v2"), b"")?;

    let output = Command::new("cmake")
        .arg("-S")
        .arg(project_dir)
        .arg("-B")
        .arg(&build)
        // Courtesy for CTest-based projects: fewer test targets to filter out.
        .arg("-DBUILD_TESTING=OFF")
        .output()
        .map_err(|e| FreightError::OptionError(format!("could not run cmake: {e}")))?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&build);
        return Err(FreightError::OptionError(format!(
            "cmake configure failed during migration:\n{}",
            String::from_utf8_lossy(&output.stderr),
        )));
    }

    let result = parse_reply(&build.join(".cmake/api/v1/reply"), project_dir);
    let _ = fs::remove_dir_all(&build);
    result
}

fn unique_build_dir() -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("freight-migrate-{pid}-{nanos}"))
}

// ── File API JSON shapes (only the fields we read) ──────────────────────────

#[derive(Deserialize)]
struct CodeModel {
    #[serde(default)]
    configurations: Vec<Configuration>,
}

#[derive(Deserialize)]
struct Configuration {
    #[serde(default)]
    directories: Vec<DirectoryEntry>,
    #[serde(default)]
    targets: Vec<TargetRef>,
}

#[derive(Deserialize)]
struct DirectoryEntry {
    /// Source path of this directory relative to the top-level source dir (`.` for
    /// the top level, `test`, `examples`, … for subdirectories).
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct TargetRef {
    #[serde(rename = "jsonFile")]
    json_file: String,
    #[serde(default, rename = "directoryIndex")]
    directory_index: Option<usize>,
}

#[derive(Deserialize)]
struct TargetFile {
    name: String,
    #[serde(rename = "type")]
    type_: String,
    #[serde(default)]
    sources: Vec<SourceEntry>,
    #[serde(default, rename = "compileGroups")]
    compile_groups: Vec<CompileGroup>,
}

#[derive(Deserialize)]
struct SourceEntry {
    path: String,
    #[serde(default, rename = "compileGroupIndex")]
    compile_group_index: Option<usize>,
}

#[derive(Deserialize)]
struct CompileGroup {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    defines: Vec<DefineEntry>,
    #[serde(default)]
    includes: Vec<IncludeEntry>,
    #[serde(default, rename = "languageStandard")]
    language_standard: Option<LanguageStandard>,
}

#[derive(Deserialize)]
struct DefineEntry {
    define: String,
}

#[derive(Deserialize)]
struct IncludeEntry {
    path: String,
}

#[derive(Deserialize)]
struct LanguageStandard {
    #[serde(default)]
    standard: Option<String>,
}

// ── Parsing ─────────────────────────────────────────────────────────────────

/// Parse the codemodel reply directory into a model. The codemodel lists targets,
/// each pointing at its own `target-*.json`.
fn parse_reply(reply_dir: &Path, project_dir: &Path) -> Result<CmakeModel, FreightError> {
    let codemodel_file = find_codemodel(reply_dir).ok_or_else(|| {
        FreightError::OptionError("cmake File API produced no codemodel reply".into())
    })?;
    let cm: CodeModel = serde_json::from_str(&fs::read_to_string(&codemodel_file)?)
        .map_err(|e| FreightError::OptionError(format!("bad codemodel JSON: {e}")))?;

    let mut targets = Vec::new();
    let configuration = cm.configurations.into_iter().next();
    let (directories, trefs) = configuration
        .map(|c| (c.directories, c.targets))
        .unwrap_or_default();
    for tref in trefs {
        // Skip targets defined under test/example/vendor subdirectories — those are
        // the project's own tests/examples and bundled deps (gtest, …), not the
        // library being migrated.
        if let Some(di) = tref.directory_index {
            if directories.get(di).is_some_and(|d| is_excluded_dir(&d.source)) {
                continue;
            }
        }
        let path = reply_dir.join(&tref.json_file);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(tf) = serde_json::from_str::<TargetFile>(&text) else {
            continue;
        };
        if let Some(target) = build_target(tf, project_dir) {
            targets.push(target);
        }
    }
    Ok(CmakeModel { targets })
}

/// Directory path components whose targets are not part of the library deliverable.
const EXCLUDED_DIR_PARTS: &[&str] = &[
    "test",
    "tests",
    "testing",
    "example",
    "examples",
    "benchmark",
    "benchmarks",
    "bench",
    "doc",
    "docs",
    "third_party",
    "thirdparty",
    "3rdparty",
    "external",
    "extern",
    "vendor",
    "contrib",
];

/// True when a directory's source path lives under a test/example/vendor tree.
fn is_excluded_dir(source: &str) -> bool {
    source
        .split('/')
        .any(|c| EXCLUDED_DIR_PARTS.contains(&c.to_ascii_lowercase().as_str()))
}

/// The codemodel reply is the `codemodel-v2-*.json` file in the reply directory.
fn find_codemodel(reply_dir: &Path) -> Option<PathBuf> {
    let pattern = reply_dir.join("codemodel-v2-*.json");
    glob::glob(&pattern.to_string_lossy())
        .ok()?
        .flatten()
        .next()
}

fn target_kind(type_: &str) -> TargetKind {
    match type_ {
        "EXECUTABLE" => TargetKind::Executable,
        "STATIC_LIBRARY" | "OBJECT_LIBRARY" => TargetKind::StaticLib,
        "SHARED_LIBRARY" | "MODULE_LIBRARY" => TargetKind::SharedLib,
        _ => TargetKind::Other,
    }
}

/// Distil a target file into a [`CmakeTarget`]. Returns `None` for kinds freight
/// can't represent (utility/interface/…) or targets with no compiled sources.
fn build_target(tf: TargetFile, project_dir: &Path) -> Option<CmakeTarget> {
    let kind = target_kind(&tf.type_);
    if kind == TargetKind::Other {
        return None;
    }

    // Compiled sources: those with a compile-group index and a project-relative
    // path (an absolute path is generated or lives outside the source tree).
    let mut sources: Vec<String> = Vec::new();
    for s in &tf.sources {
        if s.compile_group_index.is_some() && !Path::new(&s.path).is_absolute() {
            let norm = s.path.replace('\\', "/");
            if !sources.contains(&norm) {
                sources.push(norm);
            }
        }
    }
    if sources.is_empty() {
        return None;
    }

    let mut defines: Vec<String> = Vec::new();
    let mut includes: Vec<String> = Vec::new();
    let mut std = None;
    let mut language = None;
    for cg in &tf.compile_groups {
        if language.is_none() {
            language = cg.language.clone();
        }
        if std.is_none() {
            std = cg
                .language_standard
                .as_ref()
                .and_then(|ls| ls.standard.clone());
        }
        for d in &cg.defines {
            if !defines.contains(&d.define) {
                defines.push(d.define.clone());
            }
        }
        for inc in &cg.includes {
            if let Some(rel) = project_relative(&inc.path, project_dir) {
                if !includes.contains(&rel) {
                    includes.push(rel);
                }
            }
        }
    }

    Some(CmakeTarget {
        name: tf.name,
        kind,
        sources,
        defines,
        includes,
        std,
        language,
    })
}

/// Return `path` made relative to `project_dir` when it lives inside the project;
/// otherwise `None` (external/system include dirs come from deps, not the manifest).
/// A non-absolute path is assumed already project-relative.
fn project_relative(path: &str, project_dir: &Path) -> Option<String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Some(path.replace('\\', "/"));
    }
    p.strip_prefix(project_dir)
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_target_collects_sources_defines_includes_std() {
        let json = r#"{
            "name": "fmt",
            "type": "STATIC_LIBRARY",
            "sources": [
                { "path": "src/format.cc", "compileGroupIndex": 0 },
                { "path": "src/os.cc", "compileGroupIndex": 0 },
                { "path": "include/fmt/format.h" },
                { "path": "/abs/generated.cc", "compileGroupIndex": 0 }
            ],
            "compileGroups": [
                {
                    "language": "CXX",
                    "languageStandard": { "standard": "17" },
                    "defines": [ { "define": "FMT_LOCALE" }, { "define": "NDEBUG" } ],
                    "includes": [ { "path": "include" } ]
                }
            ]
        }"#;
        let tf: TargetFile = serde_json::from_str(json).unwrap();
        let t = build_target(tf, Path::new("/proj")).unwrap();
        assert_eq!(t.kind, TargetKind::StaticLib);
        // Header (no compileGroupIndex) and the absolute generated path are dropped.
        assert_eq!(t.sources, vec!["src/format.cc", "src/os.cc"]);
        assert_eq!(t.defines, vec!["FMT_LOCALE", "NDEBUG"]);
        assert_eq!(t.includes, vec!["include"]);
        assert_eq!(t.std.as_deref(), Some("17"));
        assert_eq!(t.language.as_deref(), Some("CXX"));
    }

    #[test]
    fn excluded_dirs_cover_test_and_vendor_trees() {
        assert!(is_excluded_dir("test"));
        assert!(is_excluded_dir("third_party/googletest"));
        assert!(is_excluded_dir("examples"));
        assert!(!is_excluded_dir("."));
        assert!(!is_excluded_dir("src"));
    }

    #[test]
    fn utility_target_is_ignored() {
        let tf: TargetFile = serde_json::from_str(
            r#"{ "name": "docs", "type": "UTILITY", "sources": [], "compileGroups": [] }"#,
        )
        .unwrap();
        assert!(build_target(tf, Path::new("/proj")).is_none());
    }

    #[test]
    fn absolute_include_outside_project_is_dropped() {
        assert_eq!(
            project_relative("/proj/include", Path::new("/proj")),
            Some("include".to_string())
        );
        assert_eq!(project_relative("/usr/include", Path::new("/proj")), None);
        assert_eq!(
            project_relative("include", Path::new("/proj")),
            Some("include".to_string())
        );
    }
}
