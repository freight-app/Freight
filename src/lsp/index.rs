//! Shared indexer trait and symbol documentation index.
//!
//! `LanguageIndexer` is the contract every per-language indexer in
//! `indexers/` must implement so the LSP server can drive them uniformly.
//!
//! `DocIndex` and `HeaderIndex` are freight-specific lookup structures used
//! across all indexers for `#include` hover, inlay hints, and doc lookup.

use std::collections::{BTreeMap, HashMap};
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
}

use crate::doc::{extract_dir, extract_file, DocItem, DocKind, DocLanguage, TagKind};
use crate::manifest::load_manifest;
use crate::manifest::types::Manifest;

// ---------------------------------------------------------------------------
// DocIndex
// ---------------------------------------------------------------------------

/// Symbol documentation index built from docified sources.
///
/// Supports two lookup strategies:
/// - by name (case-insensitive, for word-under-cursor fallback)
/// - by file + line (for precise position-based hover)
pub struct DocIndex {
    items: Vec<DocItem>,
    /// Lower-case symbol name → index into `items`.
    by_name: HashMap<String, usize>,
    /// Canonical file path → (doc-comment start line → index into `items`).
    by_location: HashMap<PathBuf, BTreeMap<usize, usize>>,
}

impl DocIndex {
    pub fn build_freight_packages<'a>(package_dirs: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut items: Vec<DocItem> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut by_location: HashMap<PathBuf, BTreeMap<usize, usize>> = HashMap::new();
        for package_dir in package_dirs {
            let manifest = load_manifest(package_dir).ok();
            for item in extract_pkg_items(package_dir, manifest.as_ref()) {
                if item.brief.is_empty() && item.body.is_empty() {
                    continue;
                }
                let idx = items.len();
                let key = simple_name(&item.name).to_ascii_lowercase();
                // name map: first occurrence wins
                by_name.entry(key).or_insert(idx);
                // location map: keyed by canonical file path + doc-comment start line
                if item.line > 0 {
                    let canonical = item
                        .file
                        .canonicalize()
                        .unwrap_or_else(|_| item.file.clone());
                    by_location
                        .entry(canonical)
                        .or_default()
                        .insert(item.line, idx);
                }
                items.push(item);
            }
        }
        Self {
            items,
            by_name,
            by_location,
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&DocItem> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|&i| self.items.get(i))
    }

    /// Find the doc item whose doc-comment is nearest at or before `line` in `file`.
    pub fn lookup_by_location(&self, file: &Path, line: usize) -> Option<&DocItem> {
        let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        let tree = self.by_location.get(&canonical)?;
        // Walk backwards from line to find the nearest item at or before the cursor.
        let (_, &idx) = tree.range(..=line + 5).next_back()?;
        self.items.get(idx)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Extract doc items from one Freight package, following `freight doc` TUI rules:
/// prefer public `[lib].hdrs`, then `[lib].srcs`, then `src/`, then package root.
fn extract_pkg_items(dir: &Path, manifest: Option<&Manifest>) -> Vec<DocItem> {
    let hdr_files: Vec<PathBuf> = manifest
        .and_then(|m| m.lib.as_ref())
        .map(|lib| lib.hdrs.iter().map(|h| dir.join(h)).collect())
        .unwrap_or_default();

    if !hdr_files.is_empty() {
        let mut items = Vec::new();
        for path in &hdr_files {
            if path.is_file() {
                items.extend(extract_file(path));
            }
        }
        if !items.is_empty() {
            return items;
        }
    }

    let src_dirs: Vec<PathBuf> = manifest
        .and_then(|m| m.lib.as_ref())
        .map(|lib| {
            lib.srcs
                .iter()
                .map(|s| {
                    let path = dir.join(s);
                    if path.is_dir() {
                        path
                    } else {
                        path.parent()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| dir.to_path_buf())
                    }
                })
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();

    if !src_dirs.is_empty() {
        let mut items = Vec::new();
        for src_dir in &src_dirs {
            items.extend(extract_dir(src_dir).items);
        }
        return items;
    }

    // Scan all conventional source/header directories, deduplicating.
    let candidates = ["src", "include", "inc"];
    let mut items = Vec::new();
    let mut any = false;
    for name in candidates {
        let d = dir.join(name);
        if d.is_dir() {
            items.extend(extract_dir(&d).items);
            any = true;
        }
    }
    if any {
        items
    } else {
        extract_dir(dir).items
    }
}

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
/// Parses the `#include <...> search starts here:` block from `gcc -v` output.
pub(crate) fn probe_system_include_dirs() -> Vec<PathBuf> {
    let compilers = ["c++", "g++", "clang++", "cc", "gcc", "clang"];
    for compiler in compilers {
        if let Some(dirs) = try_probe_compiler(compiler) {
            if !dirs.is_empty() {
                return dirs;
            }
        }
    }
    Vec::new()
}

fn try_probe_compiler(compiler: &str) -> Option<Vec<PathBuf>> {
    let out = std::process::Command::new(compiler)
        .args(["-xc++", "-E", "-v", "/dev/null"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .ok()?;

    let stderr = String::from_utf8_lossy(&out.stderr);
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
                // Strip any trailing " (framework directory)" annotation (macOS clang)
                let path_str = trimmed.split_once(" (").map(|(p, _)| p).unwrap_or(trimmed);
                let p = PathBuf::from(path_str);
                if p.is_dir() {
                    dirs.push(p);
                }
            }
        }
    }
    Some(dirs)
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

/// Render a `DocItem` to a Markdown string suitable for an LSP hover response.
pub fn item_to_markdown(item: &DocItem) -> String {
    let mut out = String::new();

    // Fenced code block for the signature.
    if let Some(signature) = hover_signature(item) {
        let lang = lang_id(item.lang.clone());
        out.push_str(&format!("```{lang}\n{signature}\n```\n\n"));
    }

    // Brief (first paragraph of doc comment).
    if !item.brief.is_empty() {
        out.push_str(item.brief.trim());
        out.push_str("\n\n");
    }

    // Extended body.
    if !item.body.is_empty() {
        out.push_str(item.body.trim());
        out.push_str("\n\n");
    }

    // Structured tags
    let params: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Param)
        .collect();
    let tparams: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| matches!(&t.kind, TagKind::Other(s) if s.eq_ignore_ascii_case("tparam")))
        .collect();
    let returns: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Return)
        .collect();
    let throws: Vec<&_> = item.tags.iter().filter(|t| {
        matches!(&t.kind, TagKind::Other(s) if s.eq_ignore_ascii_case("throws") || s.eq_ignore_ascii_case("exception"))
    }).collect();
    let examples: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Example)
        .collect();
    let sees: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::See)
        .collect();
    let notes: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Note)
        .collect();
    let warnings: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Warning)
        .collect();
    let deprecated: Vec<&_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Deprecated)
        .collect();

    if !deprecated.is_empty() {
        out.push_str("> ⚠️ **Deprecated**");
        if let Some(text) = deprecated
            .first()
            .map(|t| t.text.trim())
            .filter(|t| !t.is_empty())
        {
            out.push_str(&format!(": {text}"));
        }
        out.push_str("\n\n");
    }

    if !params.is_empty() || !tparams.is_empty() {
        for tag in &tparams {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `<{name}>` — {}\n", tag.text.trim()));
        }
        for tag in &params {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `{name}` — {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !returns.is_empty() {
        for tag in &returns {
            out.push_str(&format!("**Returns** {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !throws.is_empty() {
        for tag in &throws {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `throws {name}` — {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !notes.is_empty() {
        out.push_str("**Note**\n\n");
        for tag in &notes {
            out.push_str(&format!("> {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !warnings.is_empty() {
        out.push_str("**Warning**\n\n");
        for tag in &warnings {
            out.push_str(&format!("> ⚠️ {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !examples.is_empty() {
        for tag in &examples {
            out.push_str("**Example**\n\n");
            let text = tag.text.trim();
            if text.starts_with("```") {
                out.push_str(text);
            } else {
                out.push_str(&format!("```\n{text}\n```"));
            }
            out.push_str("\n\n");
        }
    }

    if !sees.is_empty() {
        out.push_str("**See also**: ");
        let refs: Vec<String> = sees
            .iter()
            .map(|t| format!("`{}`", t.text.trim()))
            .collect();
        out.push_str(&refs.join(", "));
        out.push('\n');
    }

    // File/line footer
    let rel = item.file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !rel.is_empty() && item.line > 0 {
        out.push_str(&format!("\n---\n*defined in `{}:{}`*\n", rel, item.line));
    }

    out
}

// ---------------------------------------------------------------------------
// Word extraction
// ---------------------------------------------------------------------------

/// Extract the identifier word at `(line, character)` from `text`.
pub fn word_at(text: &str, line: usize, character: usize) -> Option<String> {
    let line_text = text.lines().nth(line)?;
    let char_idx = character.min(line_text.len());
    let before = &line_text[..char_idx];
    let after = &line_text[char_idx..];
    let start = before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '~')
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    let end = char_idx
        + after
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(after.len());
    if start >= end {
        return None;
    }
    let word = &line_text[start..end];
    if word.is_empty() || word.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(word.to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn simple_name(name: &str) -> &str {
    let name = name.trim_start_matches('~');
    name.rsplit("::").next().unwrap_or(name)
}

fn hover_signature(item: &DocItem) -> Option<String> {
    let signature = if matches!(item.kind, DocKind::Variable)
        && matches!(item.lang, DocLanguage::C | DocLanguage::Cpp)
    {
        concise_c_like_variable_signature(&item.signature, &item.name)
            .unwrap_or_else(|| item.display_signature())
    } else {
        item.display_signature()
    };

    let signature = signature.trim();
    if signature.is_empty() {
        None
    } else {
        Some(signature.to_string())
    }
}

fn concise_c_like_variable_signature(raw: &str, name: &str) -> Option<String> {
    let line = raw.lines().find(|line| !line.trim().is_empty())?.trim();
    let ident = simple_name(name);
    if ident.is_empty() {
        return None;
    }

    let name_end = find_top_level_ident_end(line, ident)?;
    let mut concise = line[..name_end].trim_end().to_string();
    concise = strip_c_variable_storage(&concise).to_string();
    if concise.is_empty() {
        None
    } else {
        Some(concise)
    }
}

fn find_top_level_ident_end(line: &str, ident: &str) -> Option<usize> {
    let mut angle_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (idx, ch) in line.char_indices() {
        match ch {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }

        if angle_depth != 0 || paren_depth != 0 || bracket_depth != 0 {
            continue;
        }
        if !line[idx..].starts_with(ident) {
            continue;
        }

        let before = line[..idx].chars().next_back();
        let after = line[idx + ident.len()..].chars().next();
        if before.is_some_and(is_ident_char) || after.is_some_and(is_ident_char) {
            continue;
        }
        return Some(idx + ident.len());
    }
    None
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn strip_c_variable_storage(mut signature: &str) -> &str {
    loop {
        let trimmed = signature.trim_start();
        if let Some(rest) = trimmed.strip_prefix("static ") {
            signature = rest;
        } else if let Some(rest) = trimmed.strip_prefix("extern ") {
            signature = rest;
        } else if let Some(rest) = trimmed.strip_prefix("inline ") {
            signature = rest;
        } else if let Some(rest) = trimmed.strip_prefix("mutable ") {
            signature = rest;
        } else if let Some(rest) = trimmed.strip_prefix("thread_local ") {
            signature = rest;
        } else {
            return trimmed.trim_end();
        }
    }
}

fn lang_id(lang: DocLanguage) -> &'static str {
    match lang {
        DocLanguage::C => "c",
        DocLanguage::Cpp => "cpp",
        DocLanguage::Fortran => "fortran",
        DocLanguage::Rust => "rust",
        DocLanguage::Ada => "ada",
        DocLanguage::D => "d",
        DocLanguage::Zig => "zig",
        _ => "text",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freight_package_index_prefers_public_headers() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("include")).unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("freight.toml"),
            r#"
[package]
name = "core"
version = "0.1.0"

[language.c]
std = "c17"

[lib]
type = "static"
srcs = ["src/private.c"]
hdrs = ["include/core.h"]
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("include/core.h"),
            "/// Public API.\nint core_public(void);",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("src/private.c"),
            "/// Private implementation.\nint core_private(void) { return 1; }",
        )
        .unwrap();

        let index = DocIndex::build_freight_packages([tmp.path()]);

        assert!(index.lookup("core_public").is_some());
        assert!(index.lookup("core_private").is_none());
    }

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

    #[test]
    fn hover_markdown_uses_highlighted_concise_cpp_variable_signature() {
        let item = test_item(
            "data",
            DocKind::Variable,
            DocLanguage::Cpp,
            "std::vector<double> data = {2., 4., 4.};",
        );

        let md = item_to_markdown(&item);

        assert!(md.starts_with("```cpp\nstd::vector<double> data\n```\n\n"));
        assert!(!md.contains("{2."));
    }

    #[test]
    fn hover_signature_strips_cpp_direct_initializer_args() {
        let item = test_item(
            "tada",
            DocKind::Variable,
            DocLanguage::Cpp,
            "std::pair<double, double> tada(mean(data), variance(data));",
        );

        assert_eq!(
            hover_signature(&item).as_deref(),
            Some("std::pair<double, double> tada")
        );
    }

    #[test]
    fn hover_signature_removes_c_like_storage_class() {
        let item = test_item(
            "cout",
            DocKind::Variable,
            DocLanguage::Cpp,
            "extern ostream cout;",
        );

        assert_eq!(hover_signature(&item).as_deref(), Some("ostream cout"));
    }

    #[test]
    fn hover_markdown_uses_language_specific_fence() {
        let item = test_item(
            "parse",
            DocKind::Function,
            DocLanguage::Zig,
            "pub fn parse(value: []const u8) void {",
        );

        assert!(item_to_markdown(&item).starts_with("```zig\n"));
    }

    fn test_item(name: &str, kind: DocKind, lang: DocLanguage, signature: &str) -> DocItem {
        DocItem {
            name: name.to_string(),
            kind,
            brief: "Docs.".to_string(),
            body: String::new(),
            tags: Vec::new(),
            file: PathBuf::from("src/main.cpp"),
            line: 1,
            lang,
            signature: signature.to_string(),
            meta: Default::default(),
        }
    }
}
