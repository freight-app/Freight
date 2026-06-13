use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clang_bridge::{Index, TranslationUnit};
use serde_json::{json, Value};

use crate::build::lsp_source_flags;
use crate::lsp::index::LanguageIndexer;
use crate::lsp::protocol::{path_from_uri, position, uri_from_path};

/// Extract the subset of compile flags that affect the system include search
/// path so they can be forwarded to the compiler probe.
///
/// Covers: `-stdlib=`, `--sysroot`, `-isysroot`, `--target`, `-target`,
/// `--gcc-toolchain` — the flags that change WHICH directories are searched,
/// not just what is compiled (so `-std=c++20` is intentionally excluded).
fn env_probe_flags(flags: &[String]) -> Vec<String> {
    // Flags whose value is attached with `=` (single token).
    const SINGLE: &[&str] = &[
        "-stdlib=",
        "--sysroot=",
        "--target=",
        "-target=",
        "--gcc-toolchain=",
    ];
    // Flags whose value is the next token (two tokens).
    const TWO: &[&str] = &[
        "--sysroot",
        "-isysroot",
        "--target",
        "-target",
        "--gcc-toolchain",
    ];
    let mut out = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let f = &flags[i];
        if SINGLE.iter().any(|p| f.starts_with(p)) {
            out.push(f.clone());
            i += 1;
        } else if TWO.iter().any(|p| f.as_str() == *p) && i + 1 < flags.len() {
            out.push(f.clone());
            out.push(flags[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Convert a clang-bridge `Diagnostic` to an LSP `Diagnostic` JSON object.
/// `source` is the value for the LSP `source` field (e.g. `"clang"` or `"clang-tidy"`).
pub(crate) fn diag_to_lsp(d: &clang_bridge::diag::Diagnostic, source: &str) -> Value {
    use clang_bridge::diag::Severity;
    let severity: u32 = match d.severity {
        Severity::Note | Severity::Remark => 4,
        Severity::Warning => 2,
        Severity::Error | Severity::Fatal => 1,
    };
    let line = d.line.saturating_sub(1) as u64;
    let col = d.col.saturating_sub(1) as u64;
    let mut v = json!({
        "range": {
            "start": { "line": line, "character": col },
            "end":   { "line": line, "character": col }
        },
        "severity": severity,
        "source":   source,
        "message":  d.message
    });
    if let Some(ref name) = d.check_name {
        v["code"] = Value::String(name.clone());
    }
    v
}

/// Per-file C/C++ indexer backed by `clang-bridge`.
///
/// Holds a single `Index` (reused across parses), a TU cache keyed on the
/// absolute source path, and per-file `(working_dir, flags)` derived from the
/// freight build context.
pub struct ClangIndexer {
    index: Index,
    /// Lazily-populated translation units, keyed on absolute source path.
    tus: HashMap<PathBuf, TranslationUnit>,
    /// file path → (working_dir, compile_flags).
    /// `working_dir` is the project root; flags have no compiler binary, -c, or -o.
    source_data: HashMap<PathBuf, (String, Vec<String>)>,
}

impl ClangIndexer {
    pub fn new() -> Self {
        Self {
            index: Index::new(),
            tus: HashMap::new(),
            source_data: HashMap::new(),
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
            let (wd, flags) = self
                .source_data
                .get(path)
                .map(|(wd, f)| {
                    (
                        wd.as_str(),
                        f.iter().map(String::as_str).collect::<Vec<_>>(),
                    )
                })
                .unwrap_or(("", vec![]));
            let tu = self.index.parse(path.to_str()?, wd, &flags)?;
            self.tus.insert(path.to_path_buf(), tu);
        }
        self.tus.get(path)
    }
}

impl Default for ClangIndexer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageIndexer for ClangIndexer {
    fn handles(&self, path: &Path) -> bool {
        Self::is_c_family(path)
    }

    fn refresh_flags(&mut self, manifest_dir: &Path, profile: &str) {
        let Ok(per_file) = lsp_source_flags(manifest_dir, profile) else {
            return;
        };

        // Pass an explicit -resource-dir so ClangTool (running inside the
        // freight binary, not an installed clang binary) finds builtins like
        // stddef.h regardless of where freight lives on disk.
        let resource_dir = crate::lsp::index::probe_clang_resource_dir();

        // Probe system C++ include dirs using the actual compiler and the
        // env-relevant subset of flags for each file (stdlib, sysroot, target).
        // Cache by (compiler, env_fingerprint) so we run at most one subprocess
        // per distinct build configuration — usually just one.
        let mut probe_cache: std::collections::HashMap<(String, String), Vec<PathBuf>> =
            std::collections::HashMap::new();

        self.source_data = per_file
            .into_iter()
            .map(|(path, (compiler, dir, file_flags))| {
                let env = env_probe_flags(&file_flags);
                let cache_key = (compiler.clone(), env.join("\x00"));
                let sys_dirs = probe_cache.entry(cache_key).or_insert_with(|| {
                    let env_refs: Vec<&str> = env.iter().map(String::as_str).collect();
                    crate::lsp::index::probe_for_file(&compiler, &env_refs)
                });
                let mut combined = file_flags;
                if let Some(ref rd) = resource_dir {
                    combined.push("-resource-dir".to_string());
                    combined.push(rd.to_string_lossy().into_owned());
                }
                for d in sys_dirs.iter() {
                    combined.push("-isystem".to_string());
                    combined.push(d.to_string_lossy().into_owned());
                }
                (path, (dir, combined))
            })
            .collect();

        self.tus.clear();
    }

    fn evict(&mut self, path: &Path) {
        self.tus.remove(path);
    }

    fn hover(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let tu = self.ensure_tu(&path)?;
        let md = clang_bridge::hover::hover_full(tu, line as u32 + 1, col as u32 + 1)?;
        Some(json!({ "contents": { "kind": "markdown", "value": md } }))
    }

    fn goto_definition(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
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

    fn reparse(&mut self, uri: &str, content: &str) {
        let Some(path) = path_from_uri(uri) else {
            return;
        };
        if !Self::is_c_family(&path) {
            return;
        }
        // Ensure the TU exists (parses from disk on first call).
        self.ensure_tu(&path);
        if let Some(tu) = self.tus.get(&path) {
            clang_bridge::hover::reparse(tu, Some(content));
        }
    }

    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        let Some(path) = path_from_uri(uri) else {
            return vec![];
        };
        if !Self::is_c_family(&path) {
            return vec![];
        }
        let Some(tu) = self.ensure_tu(&path) else {
            return vec![];
        };
        let source_str = path.to_string_lossy().into_owned();
        tu.diagnostics()
            .filter(|d| d.file == source_str)
            .map(|d| diag_to_lsp(&d, "clang"))
            .collect()
    }

    fn flags_for(&self, path: &Path) -> Vec<String> {
        self.source_data
            .get(path)
            .map(|(_, f)| f.clone())
            .unwrap_or_default()
    }

    fn inlay_hints(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let tu = self.ensure_tu(&path)?;

        let range = msg.get("params")?.get("range")?;
        let start_line = range.get("start")?.get("line")?.as_u64()? as u32;
        let end_line = range.get("end")?.get("line")?.as_u64()? as u32;

        // clang-bridge uses 1-based lines; LSP uses 0-based.
        let hints = clang_bridge::inlay::inlay_hints(tu, start_line + 1, end_line + 1);
        let items: Vec<serde_json::Value> = hints
            .iter()
            .map(|h| {
                // clang-bridge kind: 0 = param, 1 = type, 2 = block-end, 3 = designator
                // LSP InlayHintKind:  2 = Parameter, 1 = Type, 4 = BlockEnd (ext), 0 = None
                let lsp_kind: u8 = match h.kind {
                    0 => 2,
                    2 => 4,
                    3 => 0,
                    _ => 1,
                };
                let padding_right = h.kind == 0; // param hints: space after label
                let padding_left = h.kind == 1; // type hints: space before ": T"
                json!({
                    "position": {
                        "line":      h.line.saturating_sub(1),
                        "character": h.col.saturating_sub(1)
                    },
                    "label":        h.label,
                    "kind":         lsp_kind,
                    "paddingLeft":  padding_left,
                    "paddingRight": padding_right
                })
            })
            .collect();

        Some(items)
    }

    fn completion(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let tu = self.ensure_tu(&path)?;
        let items: Vec<Value> =
            clang_bridge::completion::complete(tu, line as u32 + 1, col as u32 + 1, None)
                .map(|item| {
                    let mut v = json!({ "label": item.label, "kind": item.kind });
                    if let Some(d) = item.detail {
                        v["detail"] = Value::String(d);
                    }
                    if let Some(d) = item.documentation {
                        v["documentation"] = json!({ "kind": "markdown", "value": d });
                    }
                    v
                })
                .collect();
        Some(json!({ "isIncomplete": false, "items": items }))
    }

    fn document_symbols(&mut self, uri: &str) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let tu = self.ensure_tu(&path)?;
        let syms: Vec<clang_bridge::docsym::DocSym> = tu.document_symbols()?.iter().collect();
        let n = syms.len();
        // Build child lists from the flat parent-index representation.
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots: Vec<usize> = Vec::new();
        for (i, s) in syms.iter().enumerate() {
            match usize::try_from(s.parent) {
                Ok(p) if p < n => kids[p].push(i),
                _ => roots.push(i),
            }
        }
        Some(
            roots
                .iter()
                .map(|&r| doc_symbol_node(r, &syms, &kids))
                .collect(),
        )
    }

    fn references(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let include_decl = msg
            .get("params")
            .and_then(|p| p.get("context"))
            .and_then(|c| c.get("includeDeclaration"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let tu = self.ensure_tu(&path)?;
        let sym = tu.symbol_at(line as u32 + 1, col as u32 + 1)?;
        let usr = sym.usr().to_string();
        // symbol_at reports the qualified name (e.g. `geo::Point::x`); each
        // occurrence spells only the trailing identifier, so size the range to it.
        let name = sym.name();
        let name_len = name.rsplit("::").next().unwrap_or(name).chars().count() as u32;
        let out: Vec<Value> = clang_bridge::refs::references(tu, &usr)
            .iter()
            .filter(|r| include_decl || !r.is_definition)
            .map(|r| {
                let l = r.line.saturating_sub(1);
                let c = r.col.saturating_sub(1);
                json!({
                    "uri": uri_from_path(Path::new(&r.file)),
                    "range": {
                        "start": { "line": l, "character": c },
                        "end":   { "line": l, "character": c + name_len }
                    }
                })
            })
            .collect();
        Some(out)
    }

    fn document_highlight(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let (line, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let tu = self.ensure_tu(&path)?;
        let hl = clang_bridge::highlight::highlight(tu, line as u32 + 1, col as u32 + 1);
        if hl.is_empty() {
            return None;
        }
        let out: Vec<Value> = hl
            .iter()
            .map(|h| {
                let l = h.line.saturating_sub(1);
                json!({
                    "range": {
                        "start": { "line": l, "character": h.col.saturating_sub(1) },
                        "end":   { "line": l, "character": h.end_col.saturating_sub(1) }
                    },
                    // clang-bridge kind 1=text/2=read/3=write == LSP DocumentHighlightKind.
                    "kind": h.kind
                })
            })
            .collect();
        Some(out)
    }

    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<u32>> {
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let tu = self.ensure_tu(&path)?;
        let toks = clang_bridge::semtok::semantic_tokens(tu);
        // Encode as the LSP relative-delta format. clang-bridge already sorts by
        // (line, col); token_type is the legend index directly.
        let mut data: Vec<u32> = Vec::with_capacity(toks.len() * 5);
        let mut prev_line = 0u32;
        let mut prev_col = 0u32;
        for t in toks.iter() {
            let line = t.line.saturating_sub(1);
            let col = t.col.saturating_sub(1);
            let delta_line = line.saturating_sub(prev_line);
            let delta_col = if delta_line == 0 {
                col.saturating_sub(prev_col)
            } else {
                col
            };
            data.extend_from_slice(&[delta_line, delta_col, t.length, t.token_type as u32, 0]);
            prev_line = line;
            prev_col = col;
        }
        Some(data)
    }

    fn folding_ranges(&mut self, uri: &str) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_c_family(&path) {
            return None;
        }
        let tu = self.ensure_tu(&path)?;
        let out: Vec<Value> = clang_bridge::folding::folding_ranges(tu)
            .iter()
            .map(|r| {
                // LSP foldingRange lines are 0-based; clang-bridge is 1-based.
                let mut v = json!({
                    "startLine": r.start_line.saturating_sub(1),
                    "endLine":   r.end_line.saturating_sub(1),
                });
                // Only "comment" is a standard FoldingRangeKind we want to tag;
                // brace/region folds are left untagged (default region behaviour).
                if r.kind == "comment" {
                    v["kind"] = json!("comment");
                }
                v
            })
            .collect();
        Some(out)
    }
}

/// Map a clang-bridge document-symbol kind string to an LSP `SymbolKind`.
fn symbol_kind(kind: &str) -> u32 {
    match kind {
        "namespace" => 3,          // Namespace
        "class" => 5,              // Class
        "method" => 6,             // Method
        "field" | "property" => 8, // Field
        "enum" => 10,              // Enum
        "function" => 12,          // Function
        "var" => 13,               // Variable
        "enumconst" => 22,         // EnumMember
        "struct" | "union" => 23,  // Struct
        "concept" => 11,           // Interface (closest LSP kind for a concept)
        "typedef" => 5,            // Class (type alias)
        _ => 13,                   // Variable fallback
    }
}

/// Recursively build a hierarchical LSP `DocumentSymbol` from the flat list.
fn doc_symbol_node(i: usize, syms: &[clang_bridge::docsym::DocSym], kids: &[Vec<usize>]) -> Value {
    let s = &syms[i];
    let children: Vec<Value> = kids[i]
        .iter()
        .map(|&c| doc_symbol_node(c, syms, kids))
        .collect();
    let sel_line = s.sel_line.saturating_sub(1);
    let sel_start = s.sel_col.saturating_sub(1);
    let mut node = json!({
        "name": s.name,
        "kind": symbol_kind(&s.kind),
        "range": {
            "start": { "line": s.range_start_line.saturating_sub(1), "character": s.range_start_col.saturating_sub(1) },
            "end":   { "line": s.range_end_line.saturating_sub(1),   "character": s.range_end_col.saturating_sub(1) }
        },
        // selectionRange must be contained in range; cover the name token.
        "selectionRange": {
            "start": { "line": sel_line, "character": sel_start },
            "end":   { "line": sel_line, "character": sel_start + s.name.chars().count() as u32 }
        }
    });
    if !s.detail.is_empty() {
        node["detail"] = json!(s.detail);
    }
    if !children.is_empty() {
        node["children"] = json!(children);
    }
    node
}
