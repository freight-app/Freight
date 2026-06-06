use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clang_bridge::{Index, TranslationUnit};
use serde_json::{json, Value};

use crate::build::lsp_source_flags;
use crate::lsp::index::LanguageIndexer;
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

    fn is_c_family(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()).unwrap_or(""),
            "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx"
        )
    }

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

impl LanguageIndexer for ClangIndexer {
    fn handles(&self, path: &Path) -> bool {
        Self::is_c_family(path)
    }

    fn refresh_flags(&mut self, manifest_dir: &Path, profile: &str) {
        if let Ok(flags) = lsp_source_flags(manifest_dir, profile) {
            self.source_flags = flags;
            self.tus.clear();
        }
    }

    fn evict(&mut self, path: &Path) {
        self.tus.remove(path);
    }

    fn hover(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) { return None; }
        let tu = self.ensure_tu(&path)?;
        let md = clang_bridge::hover::hover_markdown(tu, line as u32 + 1, col as u32 + 1)?;
        Some(json!({ "contents": { "kind": "markdown", "value": md } }))
    }

    fn goto_definition(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) { return None; }
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

    fn completion(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) { return None; }
        let tu = self.ensure_tu(&path)?;
        let items: Vec<Value> = clang_bridge::completion::complete(tu, line as u32 + 1, col as u32 + 1, None)
            .map(|item| {
                let mut v = json!({ "label": item.label, "kind": item.kind });
                if let Some(d) = item.detail        { v["detail"] = Value::String(d); }
                if let Some(d) = item.documentation { v["documentation"] = json!({ "kind": "markdown", "value": d }); }
                v
            })
            .collect();
        Some(json!({ "isIncomplete": false, "items": items }))
    }
}
