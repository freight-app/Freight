use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clang_bridge::{Index, TranslationUnit};
use serde_json::{json, Value};

use crate::lsp::protocol::{path_from_uri, position, uri_from_path};

/// Per-file C/C++ indexer backed by `clang-bridge`.
///
/// Holds a single `Index` (reused across parses), a TU cache keyed on the
/// absolute source path, and a per-file compile-flag map derived directly from
/// the freight build context.
pub struct ClangIndexer {
    index: Index,
    /// Lazily-populated translation units, keyed on absolute source path.
    tus: HashMap<PathBuf, TranslationUnit>,
    /// file path → compile flags (no compiler binary, no -c/-o).
    /// Populated by `refresh_flags`; empty until the manifest is first loaded.
    source_flags: HashMap<PathBuf, Vec<String>>,
}

impl ClangIndexer {
    pub fn new() -> Self {
        Self {
            index: Index::new(),
            tus: HashMap::new(),
            source_flags: HashMap::new(),
        }
    }

    /// Replace the flag map and evict all cached TUs so they are reparsed with
    /// the new flags on next access.
    pub fn refresh_flags(&mut self, flags: HashMap<PathBuf, Vec<String>>) {
        self.source_flags = flags;
        self.tus.clear();
    }

    /// Evict the cached TU for `path` (call on `textDocument/didClose`).
    pub fn evict(&mut self, path: &Path) {
        self.tus.remove(path);
    }

    /// Return true if `path` is a C/C++ source or header file.
    pub fn handles(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()).unwrap_or(""),
            "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx"
        )
    }

    // ── LSP request handlers ──────────────────────────────────────────────────

    /// Try to serve `textDocument/hover`. Returns the LSP result value or `None`.
    pub fn hover(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::handles(&path) { return None; }
        let tu = self.ensure_tu(&path)?;
        let md = clang_bridge::hover::hover_markdown(tu, line as u32 + 1, col as u32 + 1)?;
        Some(json!({ "contents": { "kind": "markdown", "value": md } }))
    }

    /// Try to serve `textDocument/definition` / `textDocument/declaration`.
    /// Returns an LSP `Location` or `None`.
    pub fn goto_definition(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::handles(&path) { return None; }
        let tu = self.ensure_tu(&path)?;
        let loc = clang_bridge::goto::goto_definition(tu, line as u32 + 1, col as u32 + 1)?;
        let target_uri = uri_from_path(Path::new(&loc.file));
        Some(json!({
            "uri": target_uri,
            "range": {
                "start": { "line": loc.line.saturating_sub(1), "character": loc.col.saturating_sub(1) },
                "end":   { "line": loc.line.saturating_sub(1), "character": loc.col.saturating_sub(1) }
            }
        }))
    }

    /// Try to serve `textDocument/completion`. Returns an LSP `CompletionList`
    /// or `None`.
    pub fn completion(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::handles(&path) { return None; }
        let tu = self.ensure_tu(&path)?;
        let items: Vec<Value> = clang_bridge::completion::complete(
            tu,
            line as u32 + 1,
            col as u32 + 1,
            None,
        )
        .map(|item| {
            let mut v = json!({ "label": item.label, "kind": item.kind });
            if let Some(d) = item.detail        { v["detail"] = Value::String(d); }
            if let Some(d) = item.documentation { v["documentation"] = json!({ "kind": "markdown", "value": d }); }
            v
        })
        .collect();
        Some(json!({ "isIncomplete": false, "items": items }))
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    fn ensure_tu(&mut self, path: &Path) -> Option<&TranslationUnit> {
        if !self.tus.contains_key(path) {
            let flags: Vec<&str> = self.source_flags
                .get(path)
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let tu = self.index.parse(path.to_str()?, &flags)?;
            self.tus.insert(path.to_path_buf(), tu);
        }
        self.tus.get(path)
    }
}

impl Default for ClangIndexer {
    fn default() -> Self { Self::new() }
}
