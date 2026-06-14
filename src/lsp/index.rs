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

    /// Serve `textDocument/signatureHelp`. Returns an LSP `SignatureHelp`.
    fn signature_help(&mut self, _uri: &str, _msg: &Value) -> Option<Value> {
        None
    }

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
    fn diagnostics(&mut self, _uri: &str) -> Vec<Value> {
        vec![]
    }

    /// Return compile flags for `path`, used by external tools (e.g. clang-tidy).
    fn flags_for(&self, _path: &Path) -> Vec<String> {
        vec![]
    }

    /// Serve `textDocument/inlayHint` for source-code hints (parameter names,
    /// deduced types). Returns LSP `InlayHint[]` or `None` if this indexer
    /// does not handle the file. The default returns `None` so existing
    /// indexers that do not implement this are unaffected.
    fn inlay_hints(&mut self, _uri: &str, _msg: &Value) -> Option<Vec<Value>> {
        None
    }

    /// Serve `textDocument/documentSymbol`. Returns a hierarchical LSP
    /// `DocumentSymbol[]` or `None` if this indexer does not handle the file.
    fn document_symbols(&mut self, _uri: &str) -> Option<Vec<Value>> {
        None
    }

    /// Serve `textDocument/foldingRange`. Returns LSP `FoldingRange[]` or `None`.
    fn folding_ranges(&mut self, _uri: &str) -> Option<Vec<Value>> {
        None
    }

    /// Serve `textDocument/codeAction`. Returns LSP `CodeAction[]` or `None`.
    fn code_actions(&mut self, _uri: &str, _msg: &Value) -> Option<Vec<Value>> {
        None
    }

    /// Serve `textDocument/references`. Returns LSP `Location[]` or `None`.
    fn references(&mut self, _uri: &str, _msg: &Value) -> Option<Vec<Value>> {
        None
    }

    /// Serve `textDocument/documentHighlight`. Returns `DocumentHighlight[]` or
    /// `None` if this indexer does not handle the file.
    fn document_highlight(&mut self, _uri: &str, _msg: &Value) -> Option<Vec<Value>> {
        None
    }

    /// Serve `textDocument/semanticTokens/full`. Returns the LSP-encoded token
    /// data array (5 u32s per token: deltaLine, deltaStart, length, type,
    /// modifiers) for the legend in [`semantic_tokens_legend`], or `None`.
    fn semantic_tokens(&mut self, _uri: &str) -> Option<Vec<u32>> {
        None
    }

    /// Serve `textDocument/rename`. Returns an LSP `WorkspaceEdit` or `None`.
    fn rename(&mut self, _uri: &str, _msg: &Value) -> Option<Value> {
        None
    }
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

use crate::manifest::load_manifest_cached;

// ---------------------------------------------------------------------------
// HeaderIndex
// ---------------------------------------------------------------------------

/// Where a package's headers came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderOrigin {
    /// The current project itself, or a workspace member (both are local
    /// first-party packages: headers in `include/`, `src/`, or `[compiler].includes`).
    Own,
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
    /// Reserved for the planned "add the package that provides this header" code
    /// action (mirrors [`ModuleEntry::dep_key`]); not read yet.
    #[allow(dead_code)]
    pub dep_key: Option<String>,
    /// The package's root directory (where its `freight.toml` lives), so the
    /// inlay tooltip can show that package's manifest metadata. `None` for the
    /// standard library / system headers.
    pub pkg_dir: Option<PathBuf>,
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
                    pkg_dir: None,
                });
            }
        }
        None
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
    // Shared parser with the build include-hygiene probe; the index only wants
    // directories that actually exist on disk.
    Some(
        crate::build::include_policy::parse_search_dirs(&stderr)
            .into_iter()
            .filter(|p| p.is_dir())
            .collect(),
    )
}

/// Recursively visit every file under `dir` (depth-first), calling `visit` on
/// each non-directory entry. Unreadable directories are skipped silently. The
/// shared traversal behind the header and module index walks.
fn visit_files(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_files(&path, visit);
        } else {
            visit(&path);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_headers(
    root: &Path,
    pkg_name: &str,
    pkg_version: &Option<String>,
    origin: HeaderOrigin,
    dep_key: &Option<String>,
    pkg_dir: &Path,
    out: &mut HashMap<String, HeaderEntry>,
) {
    visit_files(root, &mut |path| {
        if !is_header(path) {
            return;
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        let entry = HeaderEntry {
            package_name: pkg_name.to_string(),
            package_version: pkg_version.clone(),
            full_path: path.to_path_buf(),
            origin: origin.clone(),
            dep_key: dep_key.clone(),
            pkg_dir: Some(pkg_dir.to_path_buf()),
        };
        insert_header(out, &rel.to_string_lossy(), entry);
    });
}

fn insert_header(map: &mut HashMap<String, HeaderEntry>, rel_path: &str, entry: HeaderEntry) {
    let normalized = rel_path.replace('\\', "/");
    let basename = Path::new(rel_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    map.entry(normalized).or_insert_with(|| entry.clone());
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
// ModuleIndex — C++20 named modules → owning package
// ---------------------------------------------------------------------------

/// Where a declared C++20 module's interface unit lives. Mirrors the relevant
/// `HeaderOrigin` cases so module imports get the same provenance labels as
/// `#include`s.
#[derive(Clone)]
pub struct ModuleEntry {
    pub package_name: String,
    pub package_version: Option<String>,
    pub origin: HeaderOrigin,
    /// The `[dependencies]` key of the providing package, mirroring
    /// [`HeaderEntry::dep_key`]. Reserved for the planned "add the package that
    /// exports this module" code action; not read yet.
    #[allow(dead_code)]
    pub dep_key: Option<String>,
    /// The interface unit that declares the module (`export module foo;`).
    pub interface_path: PathBuf,
    /// The package's root directory (where its `freight.toml` lives), for the
    /// inlay tooltip. `None` for standard-library modules.
    pub pkg_dir: Option<PathBuf>,
}

/// Maps a C++20 module name (`mylib.core`) to the package that declares it,
/// built by scanning each declared package's sources for `export module …;`.
///
/// This is the module-import analogue of [`HeaderIndex`]: it lets the LSP label
/// `import mylib.core;` with its owning package and flag imports of modules that
/// no declared dependency provides.
#[derive(Default)]
pub struct ModuleIndex {
    by_name: HashMap<String, ModuleEntry>,
}

impl ModuleIndex {
    /// Look up the package that declares module `name`.
    pub fn lookup(&self, name: &str) -> Option<&ModuleEntry> {
        self.by_name.get(name)
    }

    /// All declared module names, sorted — used for `import …;` completion.
    pub fn module_names(&self) -> Vec<(&str, &ModuleEntry)> {
        let mut out: Vec<_> = self.by_name.iter().map(|(k, v)| (k.as_str(), v)).collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }
}

/// C++ source / module-interface extensions that may carry an `export module`
/// declaration.
fn is_cpp_source(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e,
                "cppm" | "ixx" | "ccm" | "cxxm" | "mpp" | "cpp" | "cc" | "cxx" | "c++" | "cp"
            )
        })
        .unwrap_or(false)
}

/// Walk a `src/` tree once, classifying each file for both indexes: header files
/// (when `collect_headers`) feed `headers`, and C++ sources carrying an
/// `export module` declaration always feed `modules`. Header and C++-source
/// extensions are disjoint, so a file lands in at most one bucket. This is the
/// single traversal that replaces the former separate header-walk and
/// module-scan of the same `src/` directory.
#[allow(clippy::too_many_arguments)]
fn walk_src_tree(
    root: &Path,
    pkg_name: &str,
    pkg_version: &Option<String>,
    origin: HeaderOrigin,
    dep_key: &Option<String>,
    pkg_dir: &Path,
    collect_headers: bool,
    headers: &mut HashMap<String, HeaderEntry>,
    modules: &mut HashMap<String, ModuleEntry>,
) {
    visit_files(root, &mut |path| {
        if collect_headers && is_header(path) {
            let rel = path.strip_prefix(root).unwrap_or(path);
            insert_header(
                headers,
                &rel.to_string_lossy(),
                HeaderEntry {
                    package_name: pkg_name.to_string(),
                    package_version: pkg_version.clone(),
                    full_path: path.to_path_buf(),
                    origin: origin.clone(),
                    dep_key: dep_key.clone(),
                    pkg_dir: Some(pkg_dir.to_path_buf()),
                },
            );
        } else if is_cpp_source(path) {
            if let Some(name) = module_name_in_file(path) {
                modules.entry(name).or_insert_with(|| ModuleEntry {
                    package_name: pkg_name.to_string(),
                    package_version: pkg_version.clone(),
                    origin: origin.clone(),
                    dep_key: dep_key.clone(),
                    interface_path: path.to_path_buf(),
                    pkg_dir: Some(pkg_dir.to_path_buf()),
                });
            }
        }
    });
}

/// Both freight-owned source indexes, produced by a single traversal of each
/// package's source tree.
pub struct SourceIndexes {
    pub headers: HeaderIndex,
    pub modules: ModuleIndex,
}

/// Build the header and module indexes together. Each package's `src/` tree is
/// walked exactly once — headers and `export module` declarations are collected
/// in the same pass — while `include/` and `[compiler].includes` (header-only,
/// disjoint trees) keep their own walk. The `.pkgs/` cache contributes fetched
/// headers (`include/`) and modules (`src/`).
///
/// This is the single entry point the LSP refresh uses; consumers that only
/// need one half read [`SourceIndexes::headers`] or [`SourceIndexes::modules`].
pub fn build_source_indexes(
    package_dirs: &[HeaderDirSpec<'_>],
    pkgs_dir: Option<&Path>,
) -> SourceIndexes {
    let mut by_path: HashMap<String, HeaderEntry> = HashMap::new();
    let mut by_name: HashMap<String, ModuleEntry> = HashMap::new();

    for spec in package_dirs {
        let dir = spec.path;
        let manifest = load_manifest_cached(dir).ok();
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
        let is_own = spec.origin == HeaderOrigin::Own;

        // Public header surface for dependencies (non-Own packages):
        // [lib].hdrs is authoritative; otherwise fall back to walking include/.
        let lib_hdrs = manifest
            .as_ref()
            .and_then(|m| m.lib.as_ref())
            .map(|l| l.hdrs.as_slice())
            .unwrap_or(&[]);
        if !is_own && !lib_hdrs.is_empty() {
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
                            pkg_dir: Some(dir.to_path_buf()),
                        },
                    );
                }
            }
        } else if !is_own {
            let include_dir = dir.join("include");
            if include_dir.is_dir() {
                walk_headers(
                    &include_dir,
                    &pkg_name,
                    &pkg_version,
                    spec.origin.clone(),
                    &dep_key,
                    dir,
                    &mut by_path,
                );
            }
        }

        // The project itself: [compiler].includes are extra header roots where
        // relative `#include "..."` paths live.
        if is_own {
            if let Some(ref m) = manifest {
                for inc in &m.compiler.includes {
                    let inc_dir = dir.join(inc);
                    if inc_dir.is_dir() {
                        walk_headers(
                            &inc_dir,
                            &pkg_name,
                            &pkg_version,
                            HeaderOrigin::Own,
                            &dep_key,
                            dir,
                            &mut by_path,
                        );
                    }
                }
            }
        }

        // Single src/ pass: Own headers + module declarations for every package.
        let src_dir = dir.join("src");
        walk_src_tree(
            &src_dir,
            &pkg_name,
            &pkg_version,
            spec.origin.clone(),
            &dep_key,
            dir,
            is_own,
            &mut by_path,
            &mut by_name,
        );
    }

    // Installed packages in .pkgs/ — headers from include/, modules from src/.
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
            let version = Some(pkg_version.to_string());
            let include_dir = pkg_dir.join("include");
            if include_dir.is_dir() {
                walk_headers(
                    &include_dir,
                    pkg_name,
                    &version,
                    HeaderOrigin::Fetched,
                    &None,
                    &pkg_dir,
                    &mut by_path,
                );
            }
            walk_src_tree(
                &pkg_dir.join("src"),
                pkg_name,
                &version,
                HeaderOrigin::Fetched,
                &None,
                &pkg_dir,
                false,
                &mut by_path,
                &mut by_name,
            );
        }
    }

    SourceIndexes {
        headers: HeaderIndex {
            by_path,
            system_dirs: probe_system_include_dirs(),
        },
        modules: ModuleIndex { by_name },
    }
}

/// Read a source file and return the name of the primary module it declares
/// (`export module foo;`), if any. Scans only the file preamble — the module
/// declaration must precede all other declarations — so large implementation
/// files are cheap to skip.
fn module_name_in_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines().take(200) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if let Some(name) = crate::build::modules::parse_export_module(trimmed) {
            return Some(name);
        }
    }
    None
}

/// Inlay label for a named module import, given its resolved owner (if any).
pub fn module_inlay_label_for(name: &str, entry: Option<&ModuleEntry>) -> String {
    if is_std_module(name) {
        return "← stdlib".to_string();
    }
    match entry {
        Some(e) => match e.origin {
            HeaderOrigin::Own => "← module".to_string(),
            _ => format!("← {}", e.package_name),
        },
        None => "← module".to_string(),
    }
}

/// Inlay-tooltip markdown for a named module's owning package (the `← pkg`
/// annotation). Shows the providing package's `freight.toml [package]` info.
pub fn module_tooltip(name: &str, entry: Option<&ModuleEntry>) -> String {
    if is_std_module(name) {
        return format!("**{name}** — C++ standard-library module");
    }
    match entry {
        Some(e) => package_tooltip(
            e.pkg_dir.as_deref(),
            &e.package_name,
            e.package_version.as_deref(),
            &e.origin,
        ),
        None => format!("**{name}** — C++20 module"),
    }
}

/// Whether `name` is a standard-library module (`std`, `std.compat`, `std.*`).
pub fn is_std_module(name: &str) -> bool {
    name == "std" || name == "std.compat" || name.starts_with("std.")
}

// ---------------------------------------------------------------------------
// Package tooltip (inlay-hint hover)
// ---------------------------------------------------------------------------

/// Inlay-tooltip markdown for a header's owning package (the `← pkg`
/// annotation). Shows the package's `freight.toml [package]` metadata —
/// description, authors, license, repository — loaded from `pkg_dir`. Falls
/// back to `name@version` when there's no manifest (system/stdlib, or a
/// metadata-only dep).
pub fn package_tooltip(
    pkg_dir: Option<&Path>,
    fallback_name: &str,
    fallback_version: Option<&str>,
    origin: &HeaderOrigin,
) -> String {
    if matches!(origin, HeaderOrigin::System) {
        return format!("**{fallback_name}** — C/C++ standard library");
    }
    let Some(manifest) = pkg_dir.and_then(|d| load_manifest_cached(d).ok()) else {
        return match fallback_version.filter(|v| !v.is_empty()) {
            Some(v) => format!("**{fallback_name}@{v}**"),
            None => format!("**{fallback_name}**"),
        };
    };
    let p = &manifest.package;
    let mut s = if p.version.is_empty() {
        format!("**{}**", p.name)
    } else {
        format!("**{}@{}**", p.name, p.version)
    };
    if !p.description.is_empty() {
        s.push_str("\n\n");
        s.push_str(&p.description);
    }
    if !p.authors.is_empty() {
        s.push_str(&format!("\n\n*Authors:* {}", p.authors.join(", ")));
    }
    if !p.license.is_empty() {
        s.push_str(&format!("\n\n*License:* {}", p.license));
    }
    if let Some(repo) = p.repository.as_deref().filter(|r| !r.is_empty()) {
        s.push_str(&format!("\n\n*Repository:* {repo}"));
    }
    s
}

// ---------------------------------------------------------------------------
// Include hover rendering
// ---------------------------------------------------------------------------

/// Compact include-hint title line: **pkg@version**/header — the package and
/// version are bold, the header file plain. Without a version: **pkg**/header.
/// The header is the file's basename (or the spelling's last component).
pub fn include_hint_line(header: &str, entry: &HeaderEntry) -> String {
    let file = entry
        .full_path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| header.rsplit('/').next().unwrap_or(header));
    let pkg = if entry.package_name.is_empty() {
        "unknown"
    } else {
        &entry.package_name
    };
    match entry.package_version.as_deref().filter(|v| !v.is_empty()) {
        Some(ver) => format!("**{pkg}@{ver}**/{file}"),
        None => format!("**{pkg}**/{file}"),
    }
}

/// Compact include-hint title line for a named module: **pkg@version**/module,
/// **stdlib**/module for the standard library, or **module**/name when the
/// owner is unknown.
pub fn module_hint_line(name: &str, entry: Option<&ModuleEntry>) -> String {
    if is_std_module(name) {
        return format!("**stdlib**/{name}");
    }
    match entry {
        Some(e) => match e.origin {
            HeaderOrigin::Own => format!("**{}**/{name}", e.package_name),
            _ => match e.package_version.as_deref().filter(|v| !v.is_empty()) {
                Some(ver) => format!("**{}@{ver}**/{name}", e.package_name),
                None => format!("**{}**/{name}", e.package_name),
            },
        },
        None => format!("**module**/{name}"),
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
        let head: String = before[..open]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if !matches!(
            head.as_str(),
            "#include" | "#import" | "import" | "exportimport"
        ) {
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
    if !prefix
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    Some((
        IncludeCompletionCtx::Module(prefix.to_string()),
        col - prefix.len(),
    ))
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
    modules: &ModuleIndex,
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
    // `data` rides along to `completionItem/resolve`, which reads the resolved
    // header/module file and renders its Doxygen banner into the doc panel.
    let item = |name: &str, detail: &str, kind: u32, closer: Option<char>, data: Value| {
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
            "data": data,
        })
    };

    const KIND_FILE: u32 = 17;
    const KIND_MODULE: u32 = 9;
    let mut items = Vec::new();

    match &ctx {
        IncludeCompletionCtx::Module(_) => {
            if lang == Language::Cxx {
                for name in ["std", "std.compat"] {
                    items.push(item(
                        name,
                        "C++ standard-library module",
                        KIND_MODULE,
                        Some(';'),
                        serde_json::json!({ "freightInclude": true }),
                    ));
                }
                // Modules exported by the project and its declared dependencies.
                for (name, entry) in modules.module_names() {
                    let detail = match entry.origin {
                        HeaderOrigin::Own => "this project".to_string(),
                        _ => match entry.package_version.as_deref().filter(|v| !v.is_empty()) {
                            Some(ver) => format!("{} {ver}", entry.package_name),
                            None => entry.package_name.clone(),
                        },
                    };
                    let data = serde_json::json!({
                        "freightInclude": true,
                        "path": entry.interface_path.to_string_lossy(),
                    });
                    items.push(item(name, &detail, KIND_MODULE, Some(';'), data));
                }
            }
        }
        IncludeCompletionCtx::Angled(_) | IncludeCompletionCtx::Quoted(_) => {
            let closer = if matches!(ctx, IncludeCompletionCtx::Angled(_)) {
                '>'
            } else {
                '"'
            };
            // Stdlib headers only behind `<…>` — quoted form is for project files.
            if closer == '>' {
                for h in ip::c_std_headers() {
                    let data = serde_json::json!({ "freightInclude": true, "header": h });
                    items.push(item(h, "C standard library", KIND_FILE, Some(closer), data));
                }
                if lang == Language::Cxx {
                    for h in ip::cxx_std_headers() {
                        let data = serde_json::json!({ "freightInclude": true, "header": h });
                        items.push(item(h, "C++ standard library", KIND_FILE, Some(closer), data));
                    }
                }
            }
            for (path, entry) in index.completion_entries() {
                let detail = match &entry.origin {
                    HeaderOrigin::System => continue, // stdlib handled by name above
                    HeaderOrigin::Own => "this project".to_string(),
                    HeaderOrigin::PathDep | HeaderOrigin::Fetched => {
                        match entry.package_version.as_deref().filter(|v| !v.is_empty()) {
                            Some(ver) => format!("{} {ver}", entry.package_name),
                            None => entry.package_name.clone(),
                        }
                    }
                };
                let data = serde_json::json!({
                    "freightInclude": true,
                    "path": entry.full_path.to_string_lossy(),
                });
                items.push(item(path, &detail, KIND_FILE, Some(closer), data));
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
            &ModuleIndex::default(),
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
            pkg_dir: Some(PathBuf::from("/x/vecmath")),
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
            &ModuleIndex::default(),
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
            &ModuleIndex::default(),
        )
        .expect("module completion context");
        let items = result["items"].as_array().expect("items array");
        let labels: Vec<_> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
        assert_eq!(labels, vec!["std", "std.compat"]);
        assert_eq!(items[0]["detail"], "C++ standard-library module");
        assert_eq!(items[0]["textEdit"]["newText"], "std;");
    }

    #[test]
    fn include_completion_module_suggests_declared_packages() {
        let index = HeaderIndex {
            by_path: HashMap::new(),
            system_dirs: Vec::new(),
        };
        let mut by_name = HashMap::new();
        by_name.insert(
            "mylib.core".to_string(),
            ModuleEntry {
                package_name: "mylib".into(),
                package_version: Some("1.2.0".into()),
                origin: HeaderOrigin::PathDep,
                dep_key: Some("mylib".into()),
                interface_path: PathBuf::from("/x/mylib/src/core.cppm"),
                pkg_dir: None,
            },
        );
        let modules = ModuleIndex { by_name };
        let line = "import my";
        let result = include_completion(
            line,
            0,
            line.len(),
            crate::build::include_policy::Language::Cxx,
            &index,
            &modules,
        )
        .expect("module completion context");
        let items = result["items"].as_array().expect("items array");
        let core = items
            .iter()
            .find(|i| i["label"] == "mylib.core")
            .expect("declared module in completion");
        assert_eq!(core["detail"], "mylib 1.2.0");
        assert_eq!(core["textEdit"]["newText"], "mylib.core;");
    }

    #[test]
    fn module_labels_reflect_provenance() {
        // Standard-library modules.
        assert!(is_std_module("std"));
        assert!(is_std_module("std.compat"));
        assert!(is_std_module("std.core"));
        assert!(!is_std_module("mylib.core"));

        assert_eq!(module_inlay_label_for("std", None), "← stdlib");
        assert_eq!(
            module_tooltip("std", None),
            "**std** — C++ standard-library module"
        );

        // Unknown module (no declared package provides it).
        assert_eq!(module_inlay_label_for("mystery", None), "← module");
        assert_eq!(module_tooltip("mystery", None), "**mystery** — C++20 module");

        // Resolved to a declared dependency — with no manifest on disk the
        // tooltip falls back to the bold name@version.
        let dep = ModuleEntry {
            package_name: "mylib".into(),
            package_version: Some("1.2.0".into()),
            origin: HeaderOrigin::PathDep,
            dep_key: Some("mylib".into()),
            interface_path: PathBuf::from("/x/mylib/src/core.cppm"),
            pkg_dir: None,
        };
        assert_eq!(module_inlay_label_for("mylib.core", Some(&dep)), "← mylib");
        assert_eq!(module_tooltip("mylib.core", Some(&dep)), "**mylib@1.2.0**");

        // The project's own module.
        let own = ModuleEntry {
            package_name: "app".into(),
            package_version: None,
            origin: HeaderOrigin::Own,
            dep_key: None,
            interface_path: PathBuf::from("/app/src/app.cppm"),
            pkg_dir: None,
        };
        assert_eq!(module_inlay_label_for("app.gui", Some(&own)), "← module");
        assert_eq!(module_tooltip("app.gui", Some(&own)), "**app**");
    }

    #[test]
    fn module_index_scans_export_module_declarations() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = tmp.path();
        let src = tmp.join("dep/src/sub");
        std::fs::create_dir_all(&src).unwrap();
        // A primary interface unit (preceded by a global module fragment).
        std::fs::write(
            src.join("core.cppm"),
            "module;\n#include <vector>\nexport module mylib.core;\nexport int f();\n",
        )
        .unwrap();
        // A partition (must be ignored — not a whole-module import target).
        std::fs::write(src.join("part.cppm"), "export module mylib.core:detail;\n").unwrap();
        // An implementation unit (`module foo;`, not `export module`) — ignored.
        std::fs::write(
            src.join("impl.cpp"),
            "module mylib.core;\nint f() { return 0; }\n",
        )
        .unwrap();

        let spec = HeaderDirSpec {
            path: &tmp.join("dep"),
            origin: HeaderOrigin::PathDep,
            dep_key: Some("mylib".into()),
        };
        let idx = build_source_indexes(&[spec], None).modules;

        let entry = idx.lookup("mylib.core").expect("primary module indexed");
        // No freight.toml in the fixture, so the package name falls back to the
        // directory name; the dep key is preserved from the spec.
        assert_eq!(entry.package_name, "dep");
        assert_eq!(entry.dep_key.as_deref(), Some("mylib"));
        assert_eq!(entry.origin, HeaderOrigin::PathDep);
        assert!(entry.interface_path.ends_with("core.cppm"));
        // Partition and implementation units don't create separate entries.
        assert_eq!(idx.module_names().len(), 1);
        assert!(idx.lookup("mylib.core:detail").is_none());
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
        let index = build_source_indexes(&specs, None).headers;
        assert!(index.lookup("api.h").is_some());
        assert!(index.lookup("mylib/api.h").is_some());
        assert_eq!(index.lookup("api.h").unwrap().package_name, "mylib");
        assert_eq!(index.lookup("api.h").unwrap().origin, HeaderOrigin::PathDep);
    }

    #[test]
    fn include_hint_line_and_package_tooltip() {
        let tmp = tempfile::tempdir().unwrap();
        let header = tmp.path().join("include/mylib/api.h");
        std::fs::create_dir_all(header.parent().unwrap()).unwrap();
        std::fs::write(
            tmp.path().join("freight.toml"),
            "[package]\nname = \"mylib\"\nversion = \"1.0.0\"\n\
             description = \"A demo library\"\nauthors = [\"Jane Doe\"]\nlicense = \"MIT\"\n",
        )
        .unwrap();
        std::fs::write(&header, "// header").unwrap();

        let entry = HeaderEntry {
            package_name: "mylib".to_string(),
            package_version: Some("1.0.0".to_string()),
            full_path: header,
            origin: HeaderOrigin::PathDep,
            dep_key: Some("mylib".to_string()),
            pkg_dir: Some(tmp.path().to_path_buf()),
        };

        // Hover title: **pkg@version**/file (basename), version bold.
        assert_eq!(
            include_hint_line("mylib/api.h", &entry),
            "**mylib@1.0.0**/api.h"
        );

        // Inlay tooltip shows the package's freight.toml [package] metadata.
        let tip = package_tooltip(
            entry.pkg_dir.as_deref(),
            &entry.package_name,
            entry.package_version.as_deref(),
            &entry.origin,
        );
        assert!(tip.contains("**mylib@1.0.0**"), "got: {tip}");
        assert!(tip.contains("A demo library"), "got: {tip}");
        assert!(tip.contains("Jane Doe"), "got: {tip}");
        assert!(tip.contains("MIT"), "got: {tip}");
    }
}
