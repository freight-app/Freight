//! Native assembly language indexer (GAS `.s`/`.S`, NASM `.asm`/`.nasm`).
//!
//! A single-file model of an assembly translation unit. Assembly's named
//! entities are **labels** (`foo:`), **constants** (`.equ`/`.set`/`name = …`,
//! NASM `name equ …`/`%define`), and **macros** (`.macro`/`%macro`). We parse
//! those plus every identifier occurrence and the numeric local labels (`1:`
//! with directional `1f`/`1b` references), then answer:
//!
//! - `documentSymbol` — the symbol outline,
//! - `definition` — label/constant/macro references, directional numeric
//!   labels, and `.include "file"` navigation,
//! - `references` — every use of a named symbol,
//! - `hover` — symbol provenance, plus curated instruction / register /
//!   directive help,
//! - `completion` — symbols plus common directives,
//! - `foldingRange` — `.macro`/conditional blocks and per-label regions,
//! - `diagnostics` — a symbol defined more than once.
//!
//! Cross-file symbol resolution (merging symbols from `.include`d files) and a
//! full instruction/register database are tracked in `crates/freight/TODO.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::lsp::index::LanguageIndexer;
use crate::lsp::protocol::{path_from_uri, position, uri_from_path};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SymKind {
    Label,
    Constant,
    Macro,
}

impl SymKind {
    fn noun(self) -> &'static str {
        match self {
            SymKind::Label => "label",
            SymKind::Constant => "constant",
            SymKind::Macro => "macro",
        }
    }
    /// LSP `SymbolKind`.
    fn lsp_symbol_kind(self) -> u32 {
        match self {
            SymKind::Label => 12,    // Function
            SymKind::Constant => 14, // Constant
            SymKind::Macro => 12,    // Function
        }
    }
    /// LSP `CompletionItemKind`.
    fn lsp_completion_kind(self) -> u32 {
        match self {
            SymKind::Label => 3,     // Function
            SymKind::Constant => 21, // Constant
            SymKind::Macro => 3,     // Function
        }
    }
}

struct Symbol {
    name: String,
    kind: SymKind,
    line: u32,
    start_col: u32,
    end_col: u32,
}

/// A numeric local label (`1:`). These may repeat; references are directional
/// (`1f` forward / `1b` backward), so they are kept separate from named symbols.
struct NumLabel {
    value: String,
    line: u32,
}

struct Ident {
    name: String,
    line: u32,
    start_col: u32,
    end_col: u32,
    is_def: bool,
}

/// A directional reference to a numeric local label (`1f` / `1b`).
struct NumRef {
    value: String,
    forward: bool,
    line: u32,
    start_col: u32,
    end_col: u32,
}

/// An `.include "path"` / `%include "path"` directive and the span of its path.
struct Include {
    path: String,
    line: u32,
    start_col: u32,
    end_col: u32,
}

struct AsmFile {
    /// Lines of the source, kept for word-at-cursor extraction during hover.
    lines: Vec<String>,
    symbols: Vec<Symbol>,
    num_labels: Vec<NumLabel>,
    num_refs: Vec<NumRef>,
    idents: Vec<Ident>,
    includes: Vec<Include>,
    /// Foldable `[start_line, end_line]` regions.
    folds: Vec<(u32, u32)>,
}

pub struct AsmIndexer {
    files: HashMap<PathBuf, AsmFile>,
}

impl AsmIndexer {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
        }
    }

    fn is_asm(path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
                .unwrap_or(""),
            // `.S` lowercases to `s`; NASM uses `.asm`/`.nasm`.
            "s" | "asm" | "nasm"
        )
    }

    fn ensure_file(&mut self, path: &Path) -> Option<&AsmFile> {
        if !self.files.contains_key(path) {
            let source = std::fs::read_to_string(path).ok()?;
            self.files.insert(path.to_path_buf(), analyze(&source));
        }
        self.files.get(path)
    }

    /// The identifier occurrence under `(line, character)`, if any.
    fn ident_at(file: &AsmFile, line: u32, character: u32) -> Option<&Ident> {
        file.idents
            .iter()
            .find(|i| i.line == line && character >= i.start_col && character < i.end_col)
    }
}

impl Default for AsmIndexer {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageIndexer for AsmIndexer {
    fn handles(&self, path: &Path) -> bool {
        Self::is_asm(path)
    }

    fn refresh_flags(&mut self, _manifest_dir: &Path, _profile: &str) {
        // Single-file model — no include roots / compile flags consumed yet.
    }

    fn evict(&mut self, path: &Path) {
        self.files.remove(path);
    }

    fn reparse(&mut self, uri: &str, content: &str) {
        let Some(path) = path_from_uri(uri) else { return };
        if !Self::is_asm(&path) {
            return;
        }
        self.files.insert(path, analyze(content));
    }

    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        let Some(path) = path_from_uri(uri) else {
            return Vec::new();
        };
        if !Self::is_asm(&path) {
            return Vec::new();
        }
        let Some(file) = self.ensure_file(&path) else {
            return Vec::new();
        };

        // A named symbol defined more than once is an assembler error. Numeric
        // local labels are allowed to repeat and are excluded by construction.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for s in &file.symbols {
            *counts.entry(s.name.as_str()).or_insert(0) += 1;
        }
        file.symbols
            .iter()
            .filter(|s| counts.get(s.name.as_str()).copied().unwrap_or(0) > 1)
            .map(|s| {
                json!({
                    "range": span(s.line, s.start_col, s.end_col),
                    "severity": 1, // Error
                    "source": "freight",
                    "code": "duplicate-symbol",
                    "message": format!("{} `{}` is defined more than once", s.kind.noun(), s.name),
                })
            })
            .collect()
    }

    fn hover(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_asm(&path) {
            return None;
        }
        let file = self.ensure_file(&path)?;
        let line_text = file.lines.get(line)?.clone();
        let (word, word_start) = word_at(&line_text, character as u32)?;

        // 1. A named symbol defined in this file.
        if let Some(sym) = file.symbols.iter().find(|s| s.name == word) {
            let value = format!(
                "**{}** `{}`\n\nDefined at line {}.",
                sym.kind.noun(),
                sym.name,
                sym.line + 1
            );
            return Some(markdown(value));
        }

        // 2. The mnemonic slot (first token after an optional label) — a
        //    directive or an instruction.
        if Some(word_start) == mnemonic_slot(&line_text) {
            let pct = format!("%{word}");
            if let Some(desc) = directive_doc(&word).or_else(|| directive_doc(&pct)) {
                return Some(markdown(format!("**directive** `{word}`\n\n{desc}")));
            }
            if let Some(desc) = instruction_doc(&word) {
                return Some(markdown(format!("**instruction** `{word}`\n\n{desc}")));
            }
        }

        // 3. A CPU register.
        if let Some(desc) = register_doc(&word) {
            return Some(markdown(format!("**register** `{word}`\n\n{desc}")));
        }

        // 4. A directive used outside the mnemonic slot (rare, e.g. operand).
        if let Some(desc) = directive_doc(&word) {
            return Some(markdown(format!("**directive** `{word}`\n\n{desc}")));
        }

        None
    }

    fn goto_definition(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_asm(&path) {
            return None;
        }
        let line = line as u32;
        let character = character as u32;
        let file = self.ensure_file(&path)?;
        let uri_str = uri_from_path(&path);

        // `.include "file"` → open the included file (relative to this one).
        if let Some(inc) = file
            .includes
            .iter()
            .find(|i| i.line == line && character >= i.start_col && character < i.end_col)
        {
            let target = path.parent().unwrap_or(Path::new(".")).join(&inc.path);
            if target.is_file() {
                return Some(json!({
                    "uri": uri_from_path(&target),
                    "range": span(0, 0, 0),
                }));
            }
            return None;
        }

        // Directional numeric local label (`1f` / `1b`).
        if let Some(nr) = file
            .num_refs
            .iter()
            .find(|r| r.line == line && character >= r.start_col && character < r.end_col)
        {
            let target = if nr.forward {
                file.num_labels
                    .iter()
                    .filter(|l| l.value == nr.value && l.line > line)
                    .min_by_key(|l| l.line)
            } else {
                file.num_labels
                    .iter()
                    .filter(|l| l.value == nr.value && l.line < line)
                    .max_by_key(|l| l.line)
            }?;
            return Some(json!({ "uri": uri_str, "range": span(target.line, 0, nr.value.len() as u32) }));
        }

        // A named-symbol reference.
        let ident = Self::ident_at(file, line, character)?;
        let def = file.symbols.iter().find(|s| s.name == ident.name)?;
        Some(json!({
            "uri": uri_str,
            "range": span(def.line, def.start_col, def.end_col),
        }))
    }

    fn references(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_asm(&path) {
            return None;
        }
        // Honour `context.includeDeclaration` (default true).
        let include_decl = msg
            .get("params")
            .and_then(|p| p.get("context"))
            .and_then(|c| c.get("includeDeclaration"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let file = self.ensure_file(&path)?;
        let ident = Self::ident_at(file, line as u32, character as u32)?;
        if !file.symbols.iter().any(|s| s.name == ident.name) {
            return None;
        }
        let target = ident.name.clone();
        let uri_str = uri_from_path(&path);
        Some(
            file.idents
                .iter()
                .filter(|i| i.name == target && (include_decl || !i.is_def))
                .map(|i| json!({ "uri": uri_str, "range": span(i.line, i.start_col, i.end_col) }))
                .collect(),
        )
    }

    fn document_symbols(&mut self, uri: &str) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_asm(&path) {
            return None;
        }
        let file = self.ensure_file(&path)?;
        Some(
            file.symbols
                .iter()
                .map(|s| {
                    let range = span(s.line, s.start_col, s.end_col);
                    json!({
                        "name": s.name,
                        "detail": s.kind.noun(),
                        "kind": s.kind.lsp_symbol_kind(),
                        "range": range,
                        "selectionRange": range,
                    })
                })
                .collect(),
        )
    }

    fn folding_ranges(&mut self, uri: &str) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_asm(&path) {
            return None;
        }
        let file = self.ensure_file(&path)?;
        Some(
            file.folds
                .iter()
                .filter(|(s, e)| e > s)
                .map(|(s, e)| json!({ "startLine": s, "endLine": e }))
                .collect(),
        )
    }

    fn completion(&mut self, uri: &str, _msg: &Value) -> Option<Value> {
        let path = path_from_uri(uri)?;
        if !Self::is_asm(&path) {
            return None;
        }
        let file = self.ensure_file(&path)?;

        let mut items: Vec<Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for s in &file.symbols {
            if seen.insert(s.name.as_str()) {
                items.push(json!({
                    "label": s.name,
                    "kind": s.kind.lsp_completion_kind(),
                    "detail": s.kind.noun(),
                }));
            }
        }
        for (name, desc) in DIRECTIVES {
            items.push(json!({ "label": name, "kind": 14, "detail": desc }));
        }
        Some(json!({ "isIncomplete": false, "items": items }))
    }
}

fn markdown(value: String) -> Value {
    json!({ "contents": { "kind": "markdown", "value": value } })
}

fn span(line: u32, start_col: u32, end_col: u32) -> Value {
    json!({
        "start": { "line": line, "character": start_col },
        "end":   { "line": line, "character": end_col },
    })
}

// ── Parsing ──────────────────────────────────────────────────────────────────

fn is_ident_start(c: char) -> bool {
    // `$` is deliberately excluded: in GAS it is the immediate sigil (`$WIDTH`,
    // `$5`), so a symbol reference like `$WIDTH` should lex as the name `WIDTH`.
    c.is_ascii_alphabetic() || matches!(c, '_' | '.' | '?')
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$' | '?' | '@')
}

/// A lexical token on one line.
enum Tok {
    /// An identifier; `register` is true for an AT&T `%`-prefixed register.
    Word {
        text: String,
        start: u32,
        end: u32,
        register: bool,
    },
    /// A numeric token; `dir` is set for a directional local-label ref (`1f`/`1b`).
    Num {
        text: String,
        start: u32,
        end: u32,
        dir: Option<bool>, // Some(true)=forward, Some(false)=backward
    },
    Punct {
        ch: char,
    },
}

/// Lex one comment-stripped line into tokens.
fn lex(chars: &[char]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            out.push(Tok::Word {
                text: chars[start..i].iter().collect(),
                start: start as u32,
                end: i as u32,
                register: false,
            });
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            // The numeric value is the digit run only (so `1f`/`1b` matches the
            // `1:` label); the `f`/`b` direction suffix is consumed separately.
            let text: String = chars[start..i].iter().collect();
            let dir = if i < chars.len()
                && matches!(chars[i], 'f' | 'b')
                && (i + 1 >= chars.len() || !is_ident_char(chars[i + 1]))
            {
                let fwd = chars[i] == 'f';
                i += 1;
                Some(fwd)
            } else {
                None
            };
            out.push(Tok::Num {
                text,
                start: start as u32,
                end: i as u32,
                dir,
            });
        } else if c == '%' && i + 1 < chars.len() && is_ident_start(chars[i + 1]) {
            // `%rax` register (AT&T) or `%macro`/`%define` (NASM directive). The
            // analyzer disambiguates by position; lex it as a register-flagged
            // word covering only the identifier (the `%` is at `start - 1`).
            let pstart = i;
            i += 1;
            let nstart = i;
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            out.push(Tok::Word {
                text: chars[nstart..i].iter().collect(),
                start: nstart as u32,
                end: i as u32,
                register: true,
            });
            let _ = pstart;
        } else {
            if matches!(c, ':' | '=' | ',') {
                out.push(Tok::Punct { ch: c });
            }
            i += 1;
        }
    }
    out
}

/// Parse assembly source into its symbol/identifier/fold model.
fn analyze(source: &str) -> AsmFile {
    let mut symbols = Vec::new();
    let mut num_labels = Vec::new();
    let mut num_refs = Vec::new();
    let mut idents = Vec::new();
    let mut includes = Vec::new();
    let mut folds = Vec::new();
    let lines: Vec<String> = source.lines().map(str::to_string).collect();

    let mut in_block = false;
    // Stack of (block-kind, start-line) for fold detection.
    let mut block_stack: Vec<(BlockKind, u32)> = Vec::new();
    // Open per-label region: (start-line).
    let mut open_label_region: Option<u32> = None;

    for (lineno, raw) in lines.iter().enumerate() {
        let lineno = lineno as u32;
        let stripped = strip_comments(raw, &mut in_block);
        let chars: Vec<char> = stripped.chars().collect();
        let toks = lex(&chars);

        // ── folding: block directives ──
        let head = directive_head(&toks);
        if let Some(kind) = BlockKind::opens(&head) {
            block_stack.push((kind, lineno));
        } else if let Some(kind) = BlockKind::closes(&head) {
            if let Some(pos) = block_stack.iter().rposition(|(k, _)| *k == kind) {
                let (_, start) = block_stack.remove(pos);
                if lineno > start {
                    folds.push((start, lineno));
                }
            }
        }

        // ── symbol extraction ──
        let mut idx = 0usize;
        // Leading label (`name:`) or numeric label (`1:`).
        match (toks.first(), toks.get(1)) {
            (
                Some(Tok::Word {
                    text,
                    start,
                    end,
                    register: false,
                }),
                Some(Tok::Punct { ch: ':', .. }),
            ) => {
                symbols.push(Symbol {
                    name: text.clone(),
                    kind: SymKind::Label,
                    line: lineno,
                    start_col: *start,
                    end_col: *end,
                });
                idents.push(Ident {
                    name: text.clone(),
                    line: lineno,
                    start_col: *start,
                    end_col: *end,
                    is_def: true,
                });
                // A non-local label opens a new foldable region.
                if !text.starts_with(".L") && !text.starts_with('.') {
                    if let Some(start_line) = open_label_region.take() {
                        if lineno > start_line + 1 {
                            folds.push((start_line, lineno - 1));
                        }
                    }
                    open_label_region = Some(lineno);
                }
                idx = 2;
            }
            (Some(Tok::Num { text, .. }), Some(Tok::Punct { ch: ':', .. })) => {
                num_labels.push(NumLabel {
                    value: text.clone(),
                    line: lineno,
                });
                idx = 2;
            }
            _ => {}
        }

        // Constant / macro definitions in the mnemonic slot.
        if let Some(Tok::Word { text: head, .. }) = toks.get(idx) {
            let lower = head.to_ascii_lowercase();
            let is_register_head = matches!(toks.get(idx), Some(Tok::Word { register: true, .. }));
            // Directive-led definitions: `.equ NAME`, `.macro NAME`, NASM
            // `%define NAME` / `%macro NAME`.
            let def_kind = match lower.as_str() {
                ".equ" | ".set" | ".equiv" | ".equv" => Some(SymKind::Constant),
                ".macro" => Some(SymKind::Macro),
                "define" | "assign" | "xdefine" if is_register_head => Some(SymKind::Constant),
                "macro" if is_register_head => Some(SymKind::Macro),
                _ => None,
            };
            if let Some(kind) = def_kind {
                if let Some(Tok::Word {
                    text: name,
                    start,
                    end,
                    register: false,
                }) = toks.get(idx + 1)
                {
                    symbols.push(Symbol {
                        name: name.clone(),
                        kind,
                        line: lineno,
                        start_col: *start,
                        end_col: *end,
                    });
                    idents.push(Ident {
                        name: name.clone(),
                        line: lineno,
                        start_col: *start,
                        end_col: *end,
                        is_def: true,
                    });
                }
            } else if !is_register_head {
                // NASM `NAME equ VALUE` / GAS `NAME = VALUE` — a constant whose
                // name is the head word itself.
                let is_assign = matches!(
                    toks.get(idx + 1),
                    Some(Tok::Punct { ch: '=', .. })
                ) || matches!(
                    toks.get(idx + 1),
                    Some(Tok::Word { text, .. }) if text.eq_ignore_ascii_case("equ")
                );
                if is_assign {
                    if let Some(Tok::Word {
                        text: name,
                        start,
                        end,
                        register: false,
                    }) = toks.get(idx)
                    {
                        symbols.push(Symbol {
                            name: name.clone(),
                            kind: SymKind::Constant,
                            line: lineno,
                            start_col: *start,
                            end_col: *end,
                        });
                        idents.push(Ident {
                            name: name.clone(),
                            line: lineno,
                            start_col: *start,
                            end_col: *end,
                            is_def: true,
                        });
                    }
                }
            }

            // `.include "path"` / `%include "path"`.
            if lower == ".include" || (is_register_head && lower == "include") {
                if let Some((p, s, e)) = first_string(&chars) {
                    includes.push(Include {
                        path: p,
                        line: lineno,
                        start_col: s,
                        end_col: e,
                    });
                }
            }
        }

        // ── identifier & directional-ref occurrences (operands etc.) ──
        // Skip ones already pushed as definition names above by tracking spans.
        for t in &toks {
            match t {
                Tok::Word {
                    text,
                    start,
                    end,
                    register,
                } => {
                    if *register {
                        continue; // a register, not a symbol
                    }
                    let already = idents
                        .iter()
                        .any(|i| i.line == lineno && i.start_col == *start && i.is_def);
                    if !already {
                        idents.push(Ident {
                            name: text.clone(),
                            line: lineno,
                            start_col: *start,
                            end_col: *end,
                            is_def: false,
                        });
                    }
                }
                Tok::Num {
                    text,
                    start,
                    end,
                    dir: Some(forward),
                } => num_refs.push(NumRef {
                    value: text.clone(),
                    forward: *forward,
                    line: lineno,
                    start_col: *start,
                    end_col: *end,
                }),
                _ => {}
            }
        }
    }

    // Close the final per-label region at end of file.
    if let Some(start_line) = open_label_region {
        let last = lines.len().saturating_sub(1) as u32;
        if last > start_line + 1 {
            folds.push((start_line, last));
        }
    }

    AsmFile {
        lines,
        symbols,
        num_labels,
        num_refs,
        idents,
        includes,
        folds,
    }
}

/// The directive/instruction keyword at the head of a line's token list,
/// lowercased. Skips a leading `label:`. NASM `%`-directives come back as the
/// bare word (`macro`, `define`); GAS ones keep their dot (`.macro`).
fn directive_head(toks: &[Tok]) -> String {
    let mut idx = 0;
    if matches!(toks.first(), Some(Tok::Word { register: false, .. }) | Some(Tok::Num { .. }))
        && matches!(toks.get(1), Some(Tok::Punct { ch: ':', .. }))
    {
        idx = 2;
    }
    match toks.get(idx) {
        Some(Tok::Word { text, .. }) => text.to_ascii_lowercase(),
        _ => String::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Macro,
    Cond,
    Rep,
}

impl BlockKind {
    fn opens(head: &str) -> Option<BlockKind> {
        match head {
            ".macro" | "macro" => Some(BlockKind::Macro),
            ".rept" | ".irp" | ".irpc" | "rep" => Some(BlockKind::Rep),
            h if h.starts_with(".if") || h == "if" || h.starts_with("ifdef") || h.starts_with("ifndef") => {
                Some(BlockKind::Cond)
            }
            _ => None,
        }
    }
    fn closes(head: &str) -> Option<BlockKind> {
        match head {
            ".endm" | "endmacro" => Some(BlockKind::Macro),
            ".endr" | "endrep" => Some(BlockKind::Rep),
            ".endif" | "endif" => Some(BlockKind::Cond),
            _ => None,
        }
    }
}

/// Extract the first double-quoted string on a line and its column span
/// (delimiters excluded).
fn first_string(chars: &[char]) -> Option<(String, u32, u32)> {
    let open = chars.iter().position(|&c| c == '"')?;
    let close = chars[open + 1..].iter().position(|&c| c == '"')? + open + 1;
    let s: String = chars[open + 1..close].iter().collect();
    Some((s, (open + 1) as u32, close as u32))
}

/// The column of the mnemonic/directive slot on a line (first token after an
/// optional `label:`), or `None` if the line has no such token.
fn mnemonic_slot(line: &str) -> Option<u32> {
    let chars: Vec<char> = line.chars().collect();
    let toks = lex(&chars);
    let mut idx = 0;
    if matches!(toks.first(), Some(Tok::Word { register: false, .. }))
        && matches!(toks.get(1), Some(Tok::Punct { ch: ':', .. }))
    {
        idx = 2;
    }
    match toks.get(idx) {
        Some(Tok::Word { start, .. }) => Some(*start),
        _ => None,
    }
}

/// The identifier word under `character`, plus its start column. A run of
/// identifier characters; a leading `%` (register sigil) is excluded.
fn word_at(line: &str, character: u32) -> Option<(String, u32)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len() as u32;
    if n == 0 {
        return None;
    }
    let pos = character.min(n.saturating_sub(1)) as usize;
    if !is_ident_char(chars[pos]) {
        return None;
    }
    let mut start = pos;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = pos;
    while end + 1 < chars.len() && is_ident_char(chars[end + 1]) {
        end += 1;
    }
    // Drop leading sigils that aren't part of the name: `$` (GAS immediate) and
    // `@` (type sigil, e.g. `@function`).
    while start <= end && matches!(chars[start], '$' | '@') {
        start += 1;
    }
    if start > end {
        return None;
    }
    let word: String = chars[start..=end].iter().collect();
    Some((word, start as u32))
}

/// Remove assembly comments while preserving column positions for kept text.
/// Handles `#`, `;`, `//` line comments, `/* … */` block comments (state carried
/// across lines via `in_block`), and `"…"`/`'…'` strings.
fn strip_comments(line: &str, in_block: &mut bool) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;
    let mut in_str: Option<char> = None;

    while i < chars.len() {
        let c = chars[i];
        if *in_block {
            if c == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                *in_block = false;
                out.push(' ');
                out.push(' ');
                i += 2;
            } else {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        if let Some(q) = in_str {
            out.push(c);
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            in_str = Some(c);
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            *in_block = true;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if (c == '/' && i + 1 < chars.len() && chars[i + 1] == '/') || c == '#' || c == ';' {
            break; // rest of line is a comment
        }
        out.push(c);
        i += 1;
    }
    out
}

// ── Curated help tables ──────────────────────────────────────────────────────

/// Common GAS/NASM directives with a one-line description.
const DIRECTIVES: &[(&str, &str)] = &[
    (".text", "Switch to the executable code section."),
    (".data", "Switch to the initialised read/write data section."),
    (".bss", "Switch to the zero-initialised data section."),
    (".rodata", "Switch to the read-only data section."),
    (".section", "Switch to (or declare) a named section."),
    (".global", "Make a symbol visible to the linker."),
    (".globl", "Make a symbol visible to the linker."),
    (".extern", "Declare a symbol defined in another object."),
    (".local", "Mark a symbol as local to this object."),
    (".weak", "Declare a weak symbol."),
    (".type", "Set a symbol's type (e.g. `@function`, `@object`)."),
    (".size", "Set a symbol's size."),
    (".byte", "Emit one or more 8-bit values."),
    (".word", "Emit one or more 16-bit values."),
    (".short", "Emit one or more 16-bit values."),
    (".long", "Emit one or more 32-bit values."),
    (".int", "Emit one or more 32-bit values."),
    (".quad", "Emit one or more 64-bit values."),
    (".ascii", "Emit a string with no trailing NUL."),
    (".asciz", "Emit a NUL-terminated string."),
    (".string", "Emit a NUL-terminated string."),
    (".align", "Align the location counter to a boundary."),
    (".balign", "Align the location counter to a byte boundary."),
    (".p2align", "Align to a power-of-two boundary."),
    (".skip", "Emit a block of filler bytes."),
    (".space", "Emit a block of filler bytes."),
    (".zero", "Emit a block of zero bytes."),
    (".equ", "Define a symbol equal to an expression."),
    (".set", "Define a symbol equal to an expression."),
    (".equiv", "Define a symbol equal to an expression (error if already set)."),
    (".comm", "Declare a common (uninitialised) symbol."),
    (".macro", "Begin a macro definition."),
    (".endm", "End a macro definition."),
    (".rept", "Repeat the following block N times."),
    (".endr", "End a `.rept`/`.irp` block."),
    (".if", "Begin a conditional-assembly block."),
    (".else", "Else branch of a conditional-assembly block."),
    (".endif", "End a conditional-assembly block."),
    (".include", "Textually include another assembly file."),
    (".file", "Record the source file name."),
    (".cfi_startproc", "Begin a call-frame-information procedure."),
    (".cfi_endproc", "End a call-frame-information procedure."),
    // NASM
    ("section", "Switch to (or declare) a named section (NASM)."),
    ("global", "Export a symbol to the linker (NASM)."),
    ("extern", "Import a symbol from another object (NASM)."),
    ("%macro", "Begin a multi-line macro (NASM)."),
    ("%endmacro", "End a multi-line macro (NASM)."),
    ("%define", "Define a single-line macro (NASM)."),
    ("%assign", "Define a numeric single-line macro (NASM)."),
    ("%include", "Textually include another file (NASM)."),
    ("%if", "Begin conditional assembly (NASM)."),
    ("%endif", "End conditional assembly (NASM)."),
];

fn directive_doc(name: &str) -> Option<&'static str> {
    DIRECTIVES
        .iter()
        .find(|(d, _)| d.eq_ignore_ascii_case(name))
        .map(|(_, desc)| *desc)
}

/// Common x86-64 instruction mnemonics.
const INSTRUCTIONS: &[(&str, &str)] = &[
    ("mov", "Copy a value between registers/memory/immediate."),
    ("movzx", "Move with zero-extension to a wider register."),
    ("movsx", "Move with sign-extension to a wider register."),
    ("lea", "Load the effective address of a memory operand."),
    ("push", "Push a value onto the stack."),
    ("pop", "Pop a value off the stack."),
    ("call", "Call a procedure (pushes the return address)."),
    ("ret", "Return from a procedure."),
    ("leave", "Tear down a stack frame (`mov rsp,rbp; pop rbp`)."),
    ("jmp", "Unconditional jump."),
    ("je", "Jump if equal / zero (ZF=1)."),
    ("jz", "Jump if zero (ZF=1)."),
    ("jne", "Jump if not equal / not zero (ZF=0)."),
    ("jnz", "Jump if not zero (ZF=0)."),
    ("jg", "Jump if greater (signed)."),
    ("jge", "Jump if greater or equal (signed)."),
    ("jl", "Jump if less (signed)."),
    ("jle", "Jump if less or equal (signed)."),
    ("ja", "Jump if above (unsigned)."),
    ("jae", "Jump if above or equal (unsigned)."),
    ("jb", "Jump if below (unsigned)."),
    ("jbe", "Jump if below or equal (unsigned)."),
    ("cmp", "Compare two operands (sets flags)."),
    ("test", "Bitwise AND that sets flags, discarding the result."),
    ("add", "Integer addition."),
    ("sub", "Integer subtraction."),
    ("inc", "Increment by one."),
    ("dec", "Decrement by one."),
    ("imul", "Signed integer multiply."),
    ("mul", "Unsigned integer multiply."),
    ("idiv", "Signed integer divide."),
    ("div", "Unsigned integer divide."),
    ("neg", "Two's-complement negate."),
    ("and", "Bitwise AND."),
    ("or", "Bitwise OR."),
    ("xor", "Bitwise XOR (often used to zero a register)."),
    ("not", "Bitwise NOT."),
    ("shl", "Shift left (logical)."),
    ("shr", "Shift right (logical)."),
    ("sar", "Shift right (arithmetic, sign-preserving)."),
    ("sal", "Shift left (arithmetic)."),
    ("nop", "No operation."),
    ("syscall", "Invoke a system call (x86-64)."),
    ("int", "Software interrupt."),
    ("cqo", "Sign-extend RAX into RDX:RAX before idiv."),
    ("cdq", "Sign-extend EAX into EDX:EAX before idiv."),
    ("sete", "Set byte to 1 if equal/zero."),
    ("setne", "Set byte to 1 if not equal/not zero."),
];

fn instruction_doc(name: &str) -> Option<&'static str> {
    INSTRUCTIONS
        .iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(name))
        .map(|(_, desc)| *desc)
}

/// Common x86-64 registers (names without the AT&T `%` sigil).
const REGISTERS: &[(&str, &str)] = &[
    ("rax", "64-bit accumulator / integer return value (SysV)."),
    ("rbx", "64-bit general-purpose (callee-saved)."),
    ("rcx", "64-bit general-purpose / 4th integer argument (SysV)."),
    ("rdx", "64-bit general-purpose / 3rd integer argument (SysV)."),
    ("rsi", "64-bit general-purpose / 2nd integer argument (SysV)."),
    ("rdi", "64-bit general-purpose / 1st integer argument (SysV)."),
    ("rbp", "64-bit base / frame pointer (callee-saved)."),
    ("rsp", "64-bit stack pointer."),
    ("r8", "64-bit general-purpose / 5th integer argument (SysV)."),
    ("r9", "64-bit general-purpose / 6th integer argument (SysV)."),
    ("r10", "64-bit general-purpose (caller-saved)."),
    ("r11", "64-bit general-purpose (caller-saved)."),
    ("r12", "64-bit general-purpose (callee-saved)."),
    ("r13", "64-bit general-purpose (callee-saved)."),
    ("r14", "64-bit general-purpose (callee-saved)."),
    ("r15", "64-bit general-purpose (callee-saved)."),
    ("eax", "Low 32 bits of RAX."),
    ("ebx", "Low 32 bits of RBX."),
    ("ecx", "Low 32 bits of RCX."),
    ("edx", "Low 32 bits of RDX."),
    ("esi", "Low 32 bits of RSI."),
    ("edi", "Low 32 bits of RDI."),
    ("ebp", "Low 32 bits of RBP."),
    ("esp", "Low 32 bits of RSP."),
    ("rip", "Instruction pointer (RIP-relative addressing)."),
    ("al", "Low 8 bits of RAX."),
    ("cl", "Low 8 bits of RCX (shift count)."),
];

fn register_doc(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    REGISTERS
        .iter()
        .find(|(r, _)| *r == lower)
        .map(|(_, desc)| *desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labels_constants_macros() {
        let src = "\
.equ WIDTH, 80
HEIGHT = 25
main:
    mov $WIDTH, %eax
.macro prologue
    push %rbp
.endm
";
        let f = analyze(src);
        let by_kind = |k: SymKind| -> Vec<&str> {
            f.symbols
                .iter()
                .filter(|s| s.kind == k)
                .map(|s| s.name.as_str())
                .collect()
        };
        assert_eq!(by_kind(SymKind::Constant), vec!["WIDTH", "HEIGHT"]);
        assert_eq!(by_kind(SymKind::Label), vec!["main"]);
        assert_eq!(by_kind(SymKind::Macro), vec!["prologue"]);
    }

    #[test]
    fn nasm_equ_and_macro() {
        let src = "MAX equ 100\n%macro pushall 0\n    push rax\n%endmacro\n";
        let f = analyze(src);
        assert!(f
            .symbols
            .iter()
            .any(|s| s.name == "MAX" && s.kind == SymKind::Constant));
        assert!(f
            .symbols
            .iter()
            .any(|s| s.name == "pushall" && s.kind == SymKind::Macro));
    }

    #[test]
    fn registers_are_not_identifiers() {
        let f = analyze("    mov %rax, %rbx\n");
        let names: Vec<&str> = f.idents.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["mov"]);
    }

    #[test]
    fn numeric_local_labels_and_directional_refs() {
        let src = "\
    jmp 1f
1:
    nop
    jmp 1b
1:
";
        let f = analyze(src);
        assert_eq!(f.num_labels.len(), 2);
        assert_eq!(f.num_refs.len(), 2);
        assert!(f.num_refs[0].forward);
        assert!(!f.num_refs[1].forward);
    }

    #[test]
    fn comments_and_strings_are_handled() {
        let src = "\
foo:           # comment with bar:
    .asciz \"text ; not # a comment\"
    /* block
    still */ baz:
";
        let f = analyze(src);
        let names: Vec<&str> = f
            .symbols
            .iter()
            .filter(|s| s.kind == SymKind::Label)
            .map(|s| s.name.as_str())
            .collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"baz"));
        assert!(!names.contains(&"bar"));
    }

    #[test]
    fn include_directive_captured() {
        let f = analyze("    .include \"macros.inc\"\n");
        assert_eq!(f.includes.len(), 1);
        assert_eq!(f.includes[0].path, "macros.inc");
    }

    #[test]
    fn macro_block_folds() {
        let src = ".macro foo\n    nop\n    nop\n.endm\n";
        let f = analyze(src);
        assert!(f.folds.contains(&(0, 3)));
    }

    #[test]
    fn word_at_extracts_register_without_sigil() {
        let (w, start) = word_at("    mov %rax, %rbx", 9).unwrap();
        assert_eq!(w, "rax");
        assert_eq!(start, 9);
    }

    #[test]
    fn help_lookups() {
        assert!(directive_doc(".globl").is_some());
        assert!(instruction_doc("MOV").is_some()); // case-insensitive
        assert!(register_doc("RAX").is_some());
        assert!(register_doc("not_a_reg").is_none());
    }

    #[test]
    fn duplicate_symbol_counts() {
        let f = analyze("dup:\n    nop\ndup:\n");
        let n = f.symbols.iter().filter(|s| s.name == "dup").count();
        assert_eq!(n, 2);
    }

    #[test]
    fn gas_immediate_sigil_is_stripped() {
        // `$WIDTH` references the constant `WIDTH`, not a symbol named `$WIDTH`.
        let f = analyze(".equ WIDTH, 80\n    mov $WIDTH, %eax\n");
        assert!(f
            .idents
            .iter()
            .any(|i| i.name == "WIDTH" && i.line == 1 && !i.is_def));
        // word_at over the immediate yields the bare name.
        let (w, _) = word_at("    mov $WIDTH, %eax", 9).unwrap();
        assert_eq!(w, "WIDTH");
    }

    #[test]
    fn directional_ref_value_is_digits_only() {
        // `1f` must match the `1:` label, so its recorded value is `1`, not `1f`.
        let f = analyze("    jmp 1f\n1:\n");
        assert_eq!(f.num_refs.len(), 1);
        assert_eq!(f.num_refs[0].value, "1");
        assert!(f.num_refs[0].forward);
        assert_eq!(f.num_labels[0].value, "1");
    }
}
