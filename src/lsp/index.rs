//! Shared indexer trait and header index.
//!
//! `LanguageIndexer` is the contract every per-language indexer in
//! `indexers/` must implement so the LSP server can drive them uniformly.
//!
//! `HeaderIndex` is the freight-specific lookup structure used for `#include`
//! hover and inlay hints.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

// ---------------------------------------------------------------------------
// LanguageIndexer trait
// ---------------------------------------------------------------------------

/// Common interface for per-language LSP indexers.
///
/// Each implementation owns its parse state (AST cache, compile flags, etc.)
/// and answers LSP requests for the file types it handles.
pub trait LanguageIndexer: Send {
    /// Return true if this indexer handles the given source file.
    fn handles(&self, path: &Path) -> bool;

    /// Called when the manifest or build context changes so the indexer can
    /// reload compile flags and evict stale cached state.
    fn refresh_flags(&mut self, manifest_dir: &Path, profile: &str);

    /// Evict any cached state for `path` (called on `textDocument/didClose`).
    fn evict(&mut self, path: &Path);

    /// Serve `textDocument/hover`. Returns the LSP result value or `None`.
    fn hover(&mut self, uri: &str, msg: &Value) -> Option<Value>;

    /// Serve `textDocument/definition` / `textDocument/declaration`.
    /// Returns an LSP `Location` or `None`.
    fn goto_definition(&mut self, uri: &str, msg: &Value) -> Option<Value>;

    /// Serve `textDocument/completion`. Returns an LSP `CompletionList` or `None`.
    fn completion(&mut self, uri: &str, msg: &Value) -> Option<Value>;

    /// Reparse the file with `content` as the unsaved buffer so subsequent
    /// hover/goto/completion calls reflect the live editor state.
    /// A no-op for indexers that do not cache ASTs.
    fn reparse(&mut self, _uri: &str, _content: &str) {}

    /// Return LSP `Diagnostic` objects for the given URI based on the last
    /// parsed TU. Called after `didOpen`, `didChange`, and `didSave`.
    fn diagnostics(&mut self, _uri: &str) -> Vec<Value> { vec![] }

    /// Return compile flags for `path`, used by external tools (e.g. clang-tidy).
    fn flags_for(&self, _path: &Path) -> Vec<String> { vec![] }

    /// Serve `textDocument/inlayHint` for source-code hints (parameter names,
    /// deduced types). Returns LSP `InlayHint[]` or `None` if this indexer
    /// does not handle the file. The default returns `None` so existing
    /// indexers that do not implement this are unaffected.
    fn inlay_hints(&mut self, _uri: &str, _msg: &Value) -> Option<Vec<Value>> { None }

    /// Serve `textDocument/documentSymbol`. Returns a hierarchical LSP
    /// `DocumentSymbol[]` or `None` if this indexer does not handle the file.
    fn document_symbols(&mut self, _uri: &str) -> Option<Vec<Value>> { None }

    /// Serve `textDocument/foldingRange`. Returns LSP `FoldingRange[]` or `None`.
    fn folding_ranges(&mut self, _uri: &str) -> Option<Vec<Value>> { None }
}

use crate::manifest::load_manifest;

// ---------------------------------------------------------------------------
// HeaderIndex
// ---------------------------------------------------------------------------

/// Where a package's headers came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderOrigin {
    /// The current project itself (headers in its `include/`, `src/`, or `[compiler].includes`).
    Own,
    /// Workspace member (sibling crate in the same `freight.toml` workspace).
    Workspace,
    /// Path dependency declared in the current project's `[dependencies]`.
    PathDep,
    /// Installed by `freight fetch` into the project's `.pkgs/` cache.
    Fetched,
    /// System-installed (found on the compiler's default include path).
    System,
}

#[derive(Clone)]
pub struct HeaderEntry {
    pub package_name: String,
    pub package_version: Option<String>,
    pub full_path: PathBuf,
    pub origin: HeaderOrigin,
    /// The key used in `[dependencies]` for this entry (e.g. `"mylib"`).
    pub dep_key: Option<String>,
}

/// Maps include-path variants → package origin.
///
/// Both the basename (`zlib.h`) and the relative include path (`zlib/zlib.h`)
/// are indexed so lookups work for both `<zlib.h>` and `<zlib/zlib.h>`.
pub struct HeaderIndex {
    by_path: HashMap<String, HeaderEntry>,
    /// Compiler system include directories (probed once at build time).
    system_dirs: Vec<PathBuf>,
}

/// A package directory together with where it came from.
pub struct HeaderDirSpec<'a> {
    pub path: &'a Path,
    pub origin: HeaderOrigin,
    /// The dep key from `[dependencies]` — `None` for the project itself.
    pub dep_key: Option<String>,
}

impl HeaderIndex {
    /// Build the index from:
    /// - `package_dirs`: workspace members, path deps, and the project itself
    /// - `pkgs_dir`: the `.pkgs/` directory (freight-fetched packages → `Fetched`)
    pub fn build(package_dirs: &[HeaderDirSpec<'_>], pkgs_dir: Option<&Path>) -> Self {
        let mut by_path = HashMap::new();

        for spec in package_dirs {
            let dir = spec.path;
            let manifest = load_manifest(dir).ok();
            let pkg_name = manifest
                .as_ref()
                .map(|m| m.package.name.clone())
                .unwrap_or_else(|| {
                    dir.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string()
                });
            let pkg_version = manifest.as_ref().map(|m| m.package.version.clone());
            let dep_key = spec.dep_key.clone();

            // For non-Own packages: [lib].hdrs is the authoritative public API.
            // Only fall back to walking include/ when no hdrs are declared.
            let lib_hdrs = manifest
                .as_ref()
                .and_then(|m| m.lib.as_ref())
                .map(|l| l.hdrs.as_slice())
                .unwrap_or(&[]);

            if spec.origin != HeaderOrigin::Own && !lib_hdrs.is_empty() {
                for hdr in lib_hdrs {
                    let full = dir.join(hdr);
                    if full.is_file() {
                        insert_header(
                            &mut by_path,
                            hdr,
                            HeaderEntry {
                                package_name: pkg_name.clone(),
                                package_version: pkg_version.clone(),
                                full_path: full,
                                origin: spec.origin.clone(),
                                dep_key: dep_key.clone(),
                            },
                        );
                    }
                }
            } else if spec.origin != HeaderOrigin::Own {
                // No lib.hdrs declared — walk include/ as a best-effort fallback.
                let include_dir = dir.join("include");
                if include_dir.is_dir() {
                    walk_headers(
                        &include_dir,
                        &include_dir,
                        &pkg_name,
                        &pkg_version,
                        spec.origin.clone(),
                        &dep_key,
                        &mut by_path,
                    );
                }
            }

            // For the project itself: also walk [compiler].includes dirs and src/.
            // These are where relative #include "..." paths live.
            if spec.origin == HeaderOrigin::Own {
                // [compiler].includes — authoritative per-project extra include dirs
                if let Some(ref m) = manifest {
                    for inc in &m.compiler.includes {
                        let inc_dir = dir.join(inc);
                        if inc_dir.is_dir() {
                            walk_headers(
                                &inc_dir,
                                &inc_dir,
                                &pkg_name,
                                &pkg_version,
                                HeaderOrigin::Own,
                                &dep_key,
                                &mut by_path,
                            );
                        }
                    }
                }
                // src/ — relative includes like #include "utils.h"
                let src_dir = dir.join("src");
                if src_dir.is_dir() {
                    walk_headers(
                        &src_dir,
                        &src_dir,
                        &pkg_name,
                        &pkg_version,
                        HeaderOrigin::Own,
                        &dep_key,
                        &mut by_path,
                    );
                }
            }
        }

        // Installed packages in .pkgs/
        if let Some(pkgs) = pkgs_dir.filter(|p| p.is_dir()) {
            for entry in std::fs::read_dir(pkgs).into_iter().flatten().flatten() {
                let pkg_dir = entry.path();
                if !pkg_dir.is_dir() {
                    continue;
                }
                let dir_name = pkg_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let (pkg_name, pkg_version) = split_name_version(&dir_name);
                let include_dir = pkg_dir.join("include");
                if include_dir.is_dir() {
                    walk_headers(
                        &include_dir,
                        &include_dir,
                        pkg_name,
                        &Some(pkg_version.to_string()),
                        HeaderOrigin::Fetched,
                        &None,
                        &mut by_path,
                    );
                }
            }
        }

        let system_dirs = probe_system_include_dirs();
        Self {
            by_path,
            system_dirs,
        }
    }

    /// Look up a header by its include path (e.g. `"zlib.h"` or `"zlib/zlib.h"`).
    pub fn lookup(&self, header: &str) -> Option<&HeaderEntry> {
        // Exact match first (covers `zlib/zlib.h` style)
        if let Some(e) = self.by_path.get(header) {
            return Some(e);
        }
        // Basename fallback
        let basename = Path::new(header).file_name()?.to_str()?;
        self.by_path.get(basename)
    }

    /// Look up a header in the compiler's system include directories.
    /// Returns a synthetic `System` entry if found, `None` otherwise.
    pub fn lookup_system(&self, header: &str) -> Option<HeaderEntry> {
        for dir in &self.system_dirs {
            let candidate = dir.join(header);
            if candidate.exists() {
                return Some(HeaderEntry {
                    package_name: "stdlib".to_string(),
                    package_version: None,
                    full_path: candidate,
                    origin: HeaderOrigin::System,
                    dep_key: None,
                });
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

impl Default for HeaderIndex {
    fn default() -> Self {
        Self {
            by_path: HashMap::new(),
            system_dirs: probe_system_include_dirs(),
        }
    }
}

/// Probe the default C++ compiler for its system include search paths.
/// Used by `HeaderIndex` for include-hover; for per-file LSP parsing use
/// `probe_for_file` instead so the stdlib/sysroot/target are respected.
pub(crate) fn probe_system_include_dirs() -> Vec<PathBuf> {
    let compilers = ["c++", "g++", "clang++", "cc", "gcc", "clang"];
    for compiler in compilers {
        if let Some(dirs) = run_compiler_probe(compiler, &[]) {
            if !dirs.is_empty() {
                return dirs;
            }
        }
    }
    Vec::new()
}

/// Find the Clang resource directory by running `clang -print-resource-dir`.
/// Returns `None` if clang is not found or returns a nonexistent path.
///
/// Used to pass an explicit `-resource-dir` to libclang (via clang-bridge)
/// so that built-in headers like `stddef.h` are found regardless of where the
/// freight binary lives on disk.
pub(crate) fn probe_clang_resource_dir() -> Option<PathBuf> {
    for compiler in ["clang++", "clang"] {
        if let Ok(out) = std::process::Command::new(compiler)
            .arg("-print-resource-dir")
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            let s = s.trim();
            if !s.is_empty() {
                let p = PathBuf::from(s);
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Probe `compiler` with `env_flags` (e.g. `-stdlib=libc++`, `--sysroot=...`,
/// `--target=...`) to find the system C++ include dirs that match the actual
/// build configuration for a specific source file.
///
/// Falls back to `probe_system_include_dirs()` if the compiler is not found or
/// returns an empty list (e.g. a bare compiler name that isn't on PATH).
pub(crate) fn probe_for_file(compiler: &str, env_flags: &[&str]) -> Vec<PathBuf> {
    if let Some(dirs) = run_compiler_probe(compiler, env_flags) {
        if !dirs.is_empty() {
            return dirs;
        }
    }
    probe_system_include_dirs()
}

fn run_compiler_probe(compiler: &str, extra_flags: &[&str]) -> Option<Vec<PathBuf>> {
    let out = std::process::Command::new(compiler)
        .arg("-xc++")
        .arg("-E")
        .arg("-v")
        .args(extra_flags)
        .arg("/dev/null")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .ok()?;

    let stderr = String::from_utf8_lossy(&out.stderr);
    Some(parse_include_search_dirs(&stderr))
}

fn parse_include_search_dirs(stderr: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut in_block = false;
    for line in stderr.lines() {
        if line.contains("#include <...> search starts here") {
            in_block = true;
            continue;
        }
        if in_block {
            if line.starts_with("End of search list") {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                // Strip trailing " (framework directory)" annotation (macOS clang)
                let path_str = trimmed.split_once(" (").map(|(p, _)| p).unwrap_or(trimmed);
                let p = PathBuf::from(path_str);
                if p.is_dir() {
                    dirs.push(p);
                }
            }
        }
    }
    dirs
}

fn walk_headers(
    root: &Path,
    dir: &Path,
    pkg_name: &str,
    pkg_version: &Option<String>,
    origin: HeaderOrigin,
    dep_key: &Option<String>,
    out: &mut HashMap<String, HeaderEntry>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_headers(
                root,
                &path,
                pkg_name,
                pkg_version,
                origin.clone(),
                dep_key,
                out,
            );
        } else if is_header(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let entry = HeaderEntry {
                package_name: pkg_name.to_string(),
                package_version: pkg_version.clone(),
                full_path: path.clone(),
                origin: origin.clone(),
                dep_key: dep_key.clone(),
            };
            insert_header(out, &rel.to_string_lossy(), entry);
        }
    }
}

fn insert_header(map: &mut HashMap<String, HeaderEntry>, rel_path: &str, entry: HeaderEntry) {
    let normalized = rel_path.replace('\\', "/");
    let basename = Path::new(rel_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    map.entry(normalized).or_insert_with(|| HeaderEntry {
        package_name: entry.package_name.clone(),
        package_version: entry.package_version.clone(),
        full_path: entry.full_path.clone(),
        origin: entry.origin.clone(),
        dep_key: entry.dep_key.clone(),
    });
    if !basename.is_empty() {
        map.entry(basename).or_insert(entry);
    }
}

fn is_header(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e, "h" | "hh" | "hpp" | "hxx" | "h++" | "cuh" | "inc"))
        .unwrap_or(false)
}

fn split_name_version(s: &str) -> (&str, &str) {
    // "zlib-1.3.2" → ("zlib", "1.3.2")
    if let Some(pos) = s.rfind('-') {
        let after = &s[pos + 1..];
        if after.chars().next().map_or(false, |c| c.is_ascii_digit()) {
            return (&s[..pos], after);
        }
    }
    (s, "")
}

// ---------------------------------------------------------------------------
// Include hover rendering
// ---------------------------------------------------------------------------

pub fn include_hover_markdown(header: &str, entry: &HeaderEntry) -> String {
    // Build the "$origin/name-version" label
    let name_ver = if let Some(ref ver) = entry.package_version {
        if ver.is_empty() {
            entry.package_name.clone()
        } else {
            format!("{}-{ver}", entry.package_name)
        }
    } else {
        entry.package_name.clone()
    };

    let origin_label = match &entry.origin {
        HeaderOrigin::Own => format!("[this project]/{name_ver}"),
        HeaderOrigin::Workspace => format!("[workspace]/{name_ver}"),
        HeaderOrigin::PathDep => {
            if let Some(key) = &entry.dep_key {
                format!("[dep: {key}]/{name_ver}")
            } else {
                format!("[path dep]/{name_ver}")
            }
        }
        HeaderOrigin::Fetched => format!("[fetched]/{name_ver}"),
        HeaderOrigin::System => "[system]".to_string(),
    };

    let mut out = String::new();
    out.push_str(&format!("**`{origin_label}::{header}`**\n\n"));
    let path_str = display_include_path(&entry.full_path);
    out.push_str(&format!("`{path_str}`"));
    out
}

fn display_include_path(path: &Path) -> String {
    if let Some(rel) = relative_to_freight_package(path) {
        return rel;
    }
    if let Some(rel) = relative_to_pkgs_package(path) {
        return rel;
    }
    if let Some(rel) = relative_to_include_root(path) {
        return rel;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn relative_to_freight_package(path: &Path) -> Option<String> {
    for ancestor in path.ancestors() {
        if ancestor.join("freight.toml").is_file() {
            return path
                .strip_prefix(ancestor)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"));
        }
    }
    None
}

fn relative_to_pkgs_package(path: &Path) -> Option<String> {
    let parts: Vec<_> = path.components().collect();
    let pkgs_idx = parts
        .iter()
        .position(|component| component.as_os_str() == ".pkgs")?;
    let pkg_idx = pkgs_idx + 1;
    if pkg_idx >= parts.len() {
        return None;
    }

    let pkg_root: PathBuf = parts[..=pkg_idx].iter().collect();
    let pkg_name = parts[pkg_idx].as_os_str().to_string_lossy();
    let rel = path.strip_prefix(&pkg_root).ok()?;
    Some(format!(
        "{}/{}",
        pkg_name,
        rel.to_string_lossy().replace('\\', "/")
    ))
}

fn relative_to_include_root(path: &Path) -> Option<String> {
    let parts: Vec<_> = path.components().collect();
    let include_idx = parts
        .iter()
        .rposition(|component| component.as_os_str() == "include")?;
    let rel_start = include_idx + 1;
    if rel_start >= parts.len() {
        return None;
    }
    let rel: PathBuf = parts[rel_start..].iter().collect();
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Parse a header or module import directive from a line.
/// Returns `(header_path, is_system)` where `is_system` is true for `<…>` forms.
///
/// Handles:
/// - `#include <header>` / `#include "header"` — C/C++ includes
/// - `#import <header>` / `#import "header"` — ObjC / Clang module imports
/// - `import <header>;` / `import "header";` — C++20 header units
/// - `import module.name;` — C++20 named module imports (treated as system)
pub fn parse_include_header(line: &str) -> Option<(String, bool)> {
    let line = line.trim();

    // #include / #import — preprocessor directives
    let rest = if let Some(r) = line
        .strip_prefix("#include")
        .or_else(|| line.strip_prefix("#import"))
    {
        r.trim()
    } else if let Some(r) = line.strip_prefix("import") {
        // C++20: `import <header>;` / `import "header";` / `import module.name;`
        let r = r.trim().trim_end_matches(';').trim();
        if r.starts_with('<') || r.starts_with('"') {
            r
        } else {
            // Named module: `import std.core` → treat as system, no file path
            let name = r.split_whitespace().next()?.trim_end_matches(';');
            if name.is_empty() || name.contains('{') {
                return None;
            }
            return Some((name.to_string(), true));
        }
    } else {
        return None;
    };

    if rest.starts_with('<') {
        let header = rest.strip_prefix('<')?.split('>').next()?.to_string();
        Some((header, true))
    } else if rest.starts_with('"') {
        let header = rest.strip_prefix('"')?.split('"').next()?.to_string();
        Some((header, false))
    } else {
        None
    }
}
// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

/// Short label for an inlay hint: `← pkg-version` or `← stdlib`.
pub fn include_inlay_label(entry: &HeaderEntry) -> String {
    let label = match entry.origin {
        HeaderOrigin::System => "stdlib".to_string(),
        _ => {
            if let Some(ref ver) = entry.package_version {
                if ver.is_empty() {
                    entry.package_name.clone()
                } else {
                    format!("{}-{ver}", entry.package_name)
                }
            } else {
                entry.package_name.clone()
            }
        }
    };
    format!("← {label}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_include_header_angle_brackets() {
        assert_eq!(
            parse_include_header("#include <zlib.h>"),
            Some(("zlib.h".into(), true))
        );
        assert_eq!(
            parse_include_header("  #include  <foo/bar.h>"),
            Some(("foo/bar.h".into(), true))
        );
    }

    #[test]
    fn parse_include_header_quotes() {
        assert_eq!(
            parse_include_header(r#"#include "myheader.h""#),
            Some(("myheader.h".into(), false))
        );
    }

    #[test]
    fn parse_include_header_cpp20_header_unit() {
        assert_eq!(
            parse_include_header("import <vector>;"),
            Some(("vector".into(), true))
        );
        assert_eq!(
            parse_include_header(r#"import "mymodule.hpp";"#),
            Some(("mymodule.hpp".into(), false))
        );
    }

    #[test]
    fn parse_include_header_cpp20_named_module() {
        assert_eq!(
            parse_include_header("import std.core;"),
            Some(("std.core".into(), true))
        );
        assert_eq!(
            parse_include_header("import mylib;"),
            Some(("mylib".into(), true))
        );
        // export module declaration — not an import, should not match
        assert_eq!(parse_include_header("export module mylib;"), None);
    }

    #[test]
    fn header_index_lookup_by_basename() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("include/mylib")).unwrap();
        std::fs::write(
            tmp.path().join("freight.toml"),
            "[package]\nname = \"mylib\"\nversion = \"1.0.0\"\n\n[language.c]\nstd = \"c17\"\n\n[lib]\ntype = \"static\"\nsrcs = []\nhdrs = [\"include/mylib/api.h\"]\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("include/mylib/api.h"), "// header").unwrap();

        let specs = [HeaderDirSpec {
            path: tmp.path(),
            origin: HeaderOrigin::PathDep,
            dep_key: Some("mylib".to_string()),
        }];
        let index = HeaderIndex::build(&specs, None);
        assert!(index.lookup("api.h").is_some());
        assert!(index.lookup("mylib/api.h").is_some());
        assert_eq!(index.lookup("api.h").unwrap().package_name, "mylib");
        assert_eq!(index.lookup("api.h").unwrap().origin, HeaderOrigin::PathDep);
    }

    #[test]
    fn include_hover_uses_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        let header = tmp.path().join("include/mylib/api.h");
        std::fs::create_dir_all(header.parent().unwrap()).unwrap();
        std::fs::write(
            tmp.path().join("freight.toml"),
            "[package]\nname = \"mylib\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(&header, "// header").unwrap();

        let md = include_hover_markdown(
            "mylib/api.h",
            &HeaderEntry {
                package_name: "mylib".to_string(),
                package_version: Some("1.0.0".to_string()),
                full_path: header,
                origin: HeaderOrigin::PathDep,
                dep_key: Some("mylib".to_string()),
            },
        );

        assert!(md.contains("`include/mylib/api.h`"));
        assert!(!md.contains(tmp.path().to_string_lossy().as_ref()));
    }
}
