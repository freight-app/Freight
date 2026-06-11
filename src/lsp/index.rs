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

    /// Serve `textDocument/references`. Returns LSP `Location[]` or `None`.
    fn references(&mut self, _uri: &str, _msg: &Value) -> Option<Vec<Value>> { None }

    /// Serve `textDocument/documentHighlight`. Returns `DocumentHighlight[]` or
    /// `None` if this indexer does not handle the file.
    fn document_highlight(&mut self, _uri: &str, _msg: &Value) -> Option<Vec<Value>> { None }

    /// Serve `textDocument/semanticTokens/full`. Returns the LSP-encoded token
    /// data array (5 u32s per token: deltaLine, deltaStart, length, type,
    /// modifiers) for the legend in [`semantic_tokens_legend`], or `None`.
    fn semantic_tokens(&mut self, _uri: &str) -> Option<Vec<u32>> { None }
}

/// The semantic-token legend advertised in server capabilities. The order must
/// match the `token_type` indices clang-bridge emits (0 = namespace … 8 = macro).
pub fn semantic_tokens_legend() -> Value {
    serde_json::json!({
        "tokenTypes": [
            "namespace", "type", "function", "method", "property",
            "variable", "parameter", "enumMember", "macro"
        ],
        "tokenModifiers": []
    })
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

    /// All indexed package/project headers as `(include_path, entry)`, one per
    /// file, preferring the relative-path spelling (`pkg/file.h`) over the bare
    /// basename when both are indexed. Sorted for stable completion lists.
    pub fn completion_entries(&self) -> Vec<(&str, &HeaderEntry)> {
        use std::collections::hash_map::Entry;
        let mut best: HashMap<&Path, (&str, &HeaderEntry)> = HashMap::new();
        for (key, entry) in &self.by_path {
            match best.entry(entry.full_path.as_path()) {
                Entry::Occupied(mut o) => {
                    if key.contains('/') && !o.get().0.contains('/') {
                        o.insert((key.as_str(), entry));
                    }
                }
                Entry::Vacant(v) => {
                    v.insert((key.as_str(), entry));
                }
            }
        }
        let mut out: Vec<_> = best.into_values().collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
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

/// Hover/tooltip markdown for a resolved `#include`. A clean two-line form:
/// a bold source (the package + version, or "C++ standard library"), then the
/// `<package>/<file>` location.
pub fn include_hover_markdown(header: &str, entry: &HeaderEntry) -> String {
    let title = match &entry.origin {
        HeaderOrigin::System => "C++ standard library".to_string(),
        HeaderOrigin::Own => "this project".to_string(),
        HeaderOrigin::Workspace | HeaderOrigin::PathDep | HeaderOrigin::Fetched => {
            match entry.package_version.as_deref().filter(|v| !v.is_empty()) {
                Some(ver) => format!("{} {ver}", entry.package_name),
                None => entry.package_name.clone(),
            }
        }
    };
    // System headers read cleaner as `<vector>`; package headers as `pkg/file`.
    let location = match entry.origin {
        HeaderOrigin::System => format!("<{header}>"),
        _ => package_qualified_name(header, entry),
    };
    format!("**{title}**\n\n`{location}`")
}

/// Tooltip for a C++20 named module import (`import std;`, `import foo;`).
pub fn module_hover_markdown(name: &str) -> String {
    if name == "std" || name == "std.compat" || name.starts_with("std.") {
        format!("**C++ standard-library module** `{name}`")
    } else {
        format!("**C++20 module** `{name}`")
    }
}

/// Inlay label for a named module import.
pub fn module_inlay_label(name: &str) -> String {
    if name == "std" || name == "std.compat" || name.starts_with("std.") {
        "← stdlib".to_string()
    } else {
        "← module".to_string()
    }
}

/// `"<package>/<filename>"` for the header — e.g. `vecmath/vec2.h`, `stdlib/vector`.
/// Falls back to the header spelling when no package name / resolved file is known.
fn package_qualified_name(header: &str, entry: &HeaderEntry) -> String {
    let filename = entry
        .full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(header);
    if entry.package_name.is_empty() {
        filename.to_string()
    } else {
        format!("{}/{}", entry.package_name, filename)
    }
}

/// Parse a header or module import directive from a line.
/// Returns `(name, is_system, is_module)`: `is_system` is true for `<…>` forms,
/// `is_module` is true for a C++20 named-module import (`import std;`) which has
/// no header file.
///
/// Handles:
/// - `#include <header>` / `#include "header"` — C/C++ includes
/// - `#import <header>` / `#import "header"` — ObjC / Clang module imports
/// - `import <header>;` / `import "header";` — C++20 header units
/// - `import module.name;` — C++20 named module imports
pub fn parse_include_header(line: &str) -> Option<(String, bool, bool)> {
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
            // Named module: `import std.core` → no file path.
            let name = r.split_whitespace().next()?.trim_end_matches(';');
            if name.is_empty() || name.contains('{') {
                return None;
            }
            return Some((name.to_string(), true, true));
        }
    } else {
        return None;
    };

    if rest.starts_with('<') {
        let header = rest.strip_prefix('<')?.split('>').next()?.to_string();
        Some((header, true, false))
    } else if rest.starts_with('"') {
        let header = rest.strip_prefix('"')?.split('"').next()?.to_string();
        Some((header, false, false))
    } else {
        None
    }
}
// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

/// Short label for an inlay hint: `← stdlib` or `← <package>`. The version is
/// kept for the tooltip; the inline label stays terse.
pub fn include_inlay_label(entry: &HeaderEntry) -> String {
    match entry.origin {
        HeaderOrigin::System => "← stdlib".to_string(),
        _ => format!("← {}", entry.package_name),
    }
}

// ---------------------------------------------------------------------------
// Include completion
// ---------------------------------------------------------------------------

/// What kind of `#include` / `import` name is being completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeCompletionCtx {
    /// `#include <…` / `import <…` — payload is the prefix typed so far.
    Angled(String),
    /// `#include "…` / `import "…`.
    Quoted(String),
    /// `import std…` — a C++20 named-module import.
    Module(String),
}

/// Detect whether character `col` of `line` sits inside the header/module name
/// of an `#include` / `#import` / `import` directive being typed. Returns the
/// context plus the column where the name starts (the text-edit anchor).
pub fn include_completion_context(line: &str, col: usize) -> Option<(IncludeCompletionCtx, usize)> {
    let mut col = col.min(line.len());
    while !line.is_char_boundary(col) {
        col -= 1;
    }
    let before = &line[..col];

    // Angled / quoted: the opening delimiter must be preceded by a directive.
    if let Some(open) = before.rfind(['<', '"']) {
        let head: String = before[..open].chars().filter(|c| !c.is_whitespace()).collect();
        if !matches!(head.as_str(), "#include" | "#import" | "import" | "exportimport") {
            return None;
        }
        let prefix = &before[open + 1..];
        if prefix.contains('>') || prefix.contains('"') {
            return None; // cursor is past the closing delimiter
        }
        let ctx = if before.as_bytes()[open] == b'<' {
            IncludeCompletionCtx::Angled(prefix.to_string())
        } else {
            IncludeCompletionCtx::Quoted(prefix.to_string())
        };
        return Some((ctx, open + 1));
    }

    // Named module: `import std.` — no delimiter, identifier chars only.
    let t = before.trim_start();
    let t = t.strip_prefix("export ").map(str::trim_start).unwrap_or(t);
    let rest = t.strip_prefix("import")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None; // e.g. `importer`
    }
    let prefix = rest.trim_start();
    if !prefix.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
        return None;
    }
    Some((IncludeCompletionCtx::Module(prefix.to_string()), col - prefix.len()))
}

/// Build the completion response for an `#include` / `import` directive, or
/// `None` if the cursor isn't inside one. Suggests only standard-library
/// headers and headers from declared packages (the project, workspace members,
/// path deps, and `.pkgs/` installs) — never undeclared system headers — and
/// labels each item with the library it comes from.
pub fn include_completion(
    line_text: &str,
    line_no: usize,
    col: usize,
    lang: crate::build::include_policy::Language,
    index: &HeaderIndex,
) -> Option<Value> {
    use crate::build::include_policy::{self as ip, Language};

    let (ctx, start_col) = include_completion_context(line_text, col)?;
    let after: &str = {
        let mut c = col.min(line_text.len());
        while !line_text.is_char_boundary(c) {
            c -= 1;
        }
        &line_text[c..]
    };

    let edit_range = serde_json::json!({
        "start": { "line": line_no, "character": start_col },
        "end":   { "line": line_no, "character": col },
    });
    let item = |name: &str, detail: &str, kind: u32, closer: Option<char>| {
        // Append the closing delimiter only when it isn't already there.
        let insert = match closer {
            Some(c) if !after.starts_with(c) => format!("{name}{c}"),
            _ => name.to_string(),
        };
        serde_json::json!({
            "label": name,
            "kind": kind,
            "detail": detail,
            "filterText": name,
            "textEdit": { "range": edit_range.clone(), "newText": insert },
        })
    };

    const KIND_FILE: u32 = 17;
    const KIND_MODULE: u32 = 9;
    let mut items = Vec::new();

    match &ctx {
        IncludeCompletionCtx::Module(_) => {
            if lang == Language::Cxx {
                for name in ["std", "std.compat"] {
                    items.push(item(name, "C++ standard-library module", KIND_MODULE, Some(';')));
                }
            }
        }
        IncludeCompletionCtx::Angled(_) | IncludeCompletionCtx::Quoted(_) => {
            let closer = if matches!(ctx, IncludeCompletionCtx::Angled(_)) { '>' } else { '"' };
            // Stdlib headers only behind `<…>` — quoted form is for project files.
            if closer == '>' {
                for h in ip::c_std_headers() {
                    items.push(item(h, "C standard library", KIND_FILE, Some(closer)));
                }
                if lang == Language::Cxx {
                    for h in ip::cxx_std_headers() {
                        items.push(item(h, "C++ standard library", KIND_FILE, Some(closer)));
                    }
                }
            }
            for (path, entry) in index.completion_entries() {
                let detail = match &entry.origin {
                    HeaderOrigin::System => continue, // stdlib handled by name above
                    HeaderOrigin::Own => "this project".to_string(),
                    HeaderOrigin::Workspace | HeaderOrigin::PathDep | HeaderOrigin::Fetched => {
                        match entry.package_version.as_deref().filter(|v| !v.is_empty()) {
                            Some(ver) => format!("{} {ver}", entry.package_name),
                            None => entry.package_name.clone(),
                        }
                    }
                };
                items.push(item(path, &detail, KIND_FILE, Some(closer)));
            }
        }
    }

    Some(serde_json::json!({ "isIncomplete": false, "items": items }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_context_angled_and_quoted() {
        let line = "#include <vec";
        assert_eq!(
            include_completion_context(line, line.len()),
            Some((IncludeCompletionCtx::Angled("vec".into()), 10))
        );
        let line = r#"#include "stats"#;
        assert_eq!(
            include_completion_context(line, line.len()),
            Some((IncludeCompletionCtx::Quoted("stats".into()), 10))
        );
        // Cursor after the closing delimiter → no include completion.
        let line = "#include <vector> ";
        assert_eq!(include_completion_context(line, line.len()), None);
        // `<` that isn't part of a directive.
        let line = "if (a < b";
        assert_eq!(include_completion_context(line, line.len()), None);
    }

    #[test]
    fn completion_context_import_forms() {
        let line = "import <vec";
        assert_eq!(
            include_completion_context(line, line.len()),
            Some((IncludeCompletionCtx::Angled("vec".into()), 8))
        );
        let line = "import std.";
        assert_eq!(
            include_completion_context(line, line.len()),
            Some((IncludeCompletionCtx::Module("std.".into()), 7))
        );
        let line = "export import st";
        assert_eq!(
            include_completion_context(line, line.len()),
            Some((IncludeCompletionCtx::Module("st".into()), 14))
        );
        // Not an import statement.
        assert_eq!(include_completion_context("importer x", 10), None);
    }

    #[test]
    fn include_completion_lists_stdlib_with_source_detail() {
        let index = HeaderIndex {
            by_path: HashMap::new(),
            system_dirs: Vec::new(),
        };
        let line = "#include <vec";
        let result = include_completion(
            line,
            0,
            line.len(),
            crate::build::include_policy::Language::Cxx,
            &index,
        )
        .expect("include completion context");
        let items = result["items"].as_array().expect("items array");
        let vector = items
            .iter()
            .find(|i| i["label"] == "vector")
            .expect("vector in completion");
        assert_eq!(vector["detail"], "C++ standard library");
        // Closing `>` is appended because the line has none.
        assert_eq!(vector["textEdit"]["newText"], "vector>");
        assert_eq!(vector["textEdit"]["range"]["start"]["character"], 10);
        // stdio.h comes from the C table.
        let stdio = items
            .iter()
            .find(|i| i["label"] == "stdio.h")
            .expect("stdio.h in completion");
        assert_eq!(stdio["detail"], "C standard library");
    }

    #[test]
    fn include_completion_labels_package_headers() {
        let mut by_path = HashMap::new();
        let entry = HeaderEntry {
            package_name: "vecmath".into(),
            package_version: Some("0.2.0".into()),
            full_path: PathBuf::from("/x/vecmath/include/vecmath/vec2.h"),
            origin: HeaderOrigin::PathDep,
            dep_key: Some("vecmath".into()),
        };
        by_path.insert("vecmath/vec2.h".to_string(), entry.clone());
        by_path.insert("vec2.h".to_string(), entry);
        let index = HeaderIndex {
            by_path,
            system_dirs: Vec::new(),
        };
        let line = r#"#include "vec"#;
        let result = include_completion(
            line,
            3,
            line.len(),
            crate::build::include_policy::Language::Cxx,
            &index,
        )
        .expect("include completion context");
        let items = result["items"].as_array().expect("items array");
        // Quoted form: no stdlib, only the package header — path spelling wins.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["label"], "vecmath/vec2.h");
        assert_eq!(items[0]["detail"], "vecmath 0.2.0");
        assert_eq!(items[0]["textEdit"]["newText"], "vecmath/vec2.h\"");
    }

    #[test]
    fn include_completion_module_suggests_std() {
        let index = HeaderIndex {
            by_path: HashMap::new(),
            system_dirs: Vec::new(),
        };
        let line = "import st";
        let result = include_completion(
            line,
            0,
            line.len(),
            crate::build::include_policy::Language::Cxx,
            &index,
        )
        .expect("module completion context");
        let items = result["items"].as_array().expect("items array");
        let labels: Vec<_> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert_eq!(labels, vec!["std", "std.compat"]);
        assert_eq!(items[0]["detail"], "C++ standard-library module");
        assert_eq!(items[0]["textEdit"]["newText"], "std;");
    }

    #[test]
    fn parse_include_header_angle_brackets() {
        assert_eq!(
            parse_include_header("#include <zlib.h>"),
            Some(("zlib.h".into(), true, false))
        );
        assert_eq!(
            parse_include_header("  #include  <foo/bar.h>"),
            Some(("foo/bar.h".into(), true, false))
        );
    }

    #[test]
    fn parse_include_header_quotes() {
        assert_eq!(
            parse_include_header(r#"#include "myheader.h""#),
            Some(("myheader.h".into(), false, false))
        );
    }

    #[test]
    fn parse_include_header_cpp20_header_unit() {
        assert_eq!(
            parse_include_header("import <vector>;"),
            Some(("vector".into(), true, false))
        );
        assert_eq!(
            parse_include_header(r#"import "mymodule.hpp";"#),
            Some(("mymodule.hpp".into(), false, false))
        );
    }

    #[test]
    fn parse_include_header_cpp20_named_module() {
        assert_eq!(
            parse_include_header("import std.core;"),
            Some(("std.core".into(), true, true))
        );
        assert_eq!(
            parse_include_header("import mylib;"),
            Some(("mylib".into(), true, true))
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
    fn include_hover_uses_package_qualified_path() {
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

        // Path is shown as "<package>/<filename>", not the absolute resolved path.
        assert!(md.contains("`mylib/api.h`"), "got: {md}");
        assert!(!md.contains(tmp.path().to_string_lossy().as_ref()));
    }
}
