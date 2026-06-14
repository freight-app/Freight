//! Native assembly language indexer (GAS `.s`/`.S`, NASM `.asm`/`.nasm`).
//!
//! A single-file model of an assembly translation unit. Assembly's named
//! entities are **labels** (`foo:`), **constants** (`.equ`/`.set`/`name = …`,
//! NASM `name equ …`/`%define`), and **macros** (`.macro`/`%macro`). We parse
//! those plus every identifier occurrence and the numeric local labels (`1:`
//! with directional `1f`/`1b` references), then answer:
//!
//! - `documentSymbol` — the symbol outline,
//! - `definition` — label/constant/macro references (resolved across the
//!   `.include` closure), directional numeric labels, and `.include "file"`
//!   navigation,
//! - `references` — every use of a named symbol across the include closure,
//! - `hover` — symbol provenance, plus curated per-arch instruction / register
//!   / directive help (x86-64, AArch64, RISC-V; arch from the manifest target),
//! - `completion` — symbols (current file + includes) plus common directives,
//! - `foldingRange` — `.macro`/conditional blocks and per-label regions,
//! - `diagnostics` — a symbol defined more than once (macro-body labels, which
//!   are templated per expansion, are excluded).
//!
//! Cross-file resolution follows `.include`/`%include` transitively from the
//! queried file. Macro parameters (`\name`) and macro-body labels are treated as
//! macro-locals. Semantic tokens are left to the client until freight owns the
//! global token legend (see the clang-bridge note in `crates/freight/TODO.md`).

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
    /// Defined inside a `.macro`/`%macro` body. Such labels are templated per
    /// expansion (often via `\@`), so they are excluded from duplicate-symbol
    /// diagnostics — two macros may legitimately use the same internal label.
    in_macro: bool,
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
    /// CPU architecture for instruction/register help, from the manifest target
    /// (or the host) via [`refresh_flags`]. Defaults to the host arch.
    arch: Arch,
}

impl AsmIndexer {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            arch: Arch::host(),
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

    /// The transitive closure of asm files reachable from `path` via `.include`
    /// / `%include`, starting with `path` itself. Breadth-first so the queried
    /// file ranks before its includes (a symbol it defines wins over an included
    /// one). Cycle-safe (dedup by canonical path); missing/unreadable includes
    /// are skipped. Ensures every file in the closure is parsed and cached.
    fn include_closure(&mut self, path: &Path) -> Vec<PathBuf> {
        use std::collections::{HashSet, VecDeque};
        let mut order: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<PathBuf> = VecDeque::new();
        queue.push_back(path.to_path_buf());
        while let Some(p) = queue.pop_front() {
            let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
            if !seen.insert(canon) {
                continue;
            }
            if self.ensure_file(&p).is_none() {
                continue;
            }
            order.push(p.clone());
            let dir = p.parent().unwrap_or(Path::new(".")).to_path_buf();
            let targets: Vec<PathBuf> = self
                .files
                .get(&p)
                .map(|f| {
                    f.includes
                        .iter()
                        .map(|i| dir.join(&i.path))
                        .filter(|t| t.is_file())
                        .collect()
                })
                .unwrap_or_default();
            for t in targets {
                queue.push_back(t);
            }
        }
        order
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

    fn refresh_flags(&mut self, manifest_dir: &Path, _profile: &str) {
        // Pick the instruction/register help arch from the project's target
        // (e.g. `[target] arch = "aarch64"`), falling back to the host arch.
        let arch_str = crate::manifest::load_manifest_cached(manifest_dir)
            .ok()
            .and_then(|m| m.target.arch.clone())
            .unwrap_or_else(|| std::env::consts::ARCH.to_string());
        self.arch = Arch::from_target(&arch_str);
    }

    fn evict(&mut self, path: &Path) {
        self.files.remove(path);
    }

    fn reparse(&mut self, uri: &str, content: &str) {
        let Some(path) = path_from_uri(uri) else {
            return;
        };
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
        // Symbols defined inside a macro body are templated per expansion (often
        // via `\@`) and excluded — counted neither as duplicates nor as causes.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for s in file.symbols.iter().filter(|s| !s.in_macro) {
            *counts.entry(s.name.as_str()).or_insert(0) += 1;
        }
        file.symbols
            .iter()
            .filter(|s| !s.in_macro && counts.get(s.name.as_str()).copied().unwrap_or(0) > 1)
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
        let arch = self.arch;
        let closure = self.include_closure(&path);
        let line_text = self.files.get(&path)?.lines.get(line)?.clone();
        let (word, word_start) = word_at(&line_text, character as u32)?;

        // 1. A named symbol defined in this file or any `.include`d file.
        for p in &closure {
            let Some(f) = self.files.get(p) else { continue };
            if let Some(sym) = f.symbols.iter().find(|s| s.name == word) {
                let origin = if p == &path {
                    format!("Defined at line {}.", sym.line + 1)
                } else {
                    format!(
                        "Defined in `{}` at line {}.",
                        p.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                        sym.line + 1
                    )
                };
                return Some(markdown(format!(
                    "**{}** `{}`\n\n{origin}",
                    sym.kind.noun(),
                    sym.name
                )));
            }
        }

        // 2. The mnemonic slot (first token after an optional label) — a
        //    directive or an instruction.
        if Some(word_start) == mnemonic_slot(&line_text) {
            let pct = format!("%{word}");
            if let Some(desc) = directive_doc(&word).or_else(|| directive_doc(&pct)) {
                return Some(markdown(format!("**directive** `{word}`\n\n{desc}")));
            }
            if let Some(desc) = instruction_doc(&word, arch) {
                return Some(markdown(format!("**instruction** `{word}`\n\n{desc}")));
            }
        }

        // 3. A CPU register.
        if let Some(desc) = register_doc(&word, arch) {
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
        let closure = self.include_closure(&path);
        let file = self.files.get(&path)?;
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

        // Directional numeric local label (`1f` / `1b`) — always file-local.
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
            return Some(
                json!({ "uri": uri_str, "range": span(target.line, 0, nr.value.len() as u32) }),
            );
        }

        // A named-symbol reference — resolve across the `.include` closure
        // (current file first, then included files).
        let ident = Self::ident_at(file, line, character)?;
        let name = ident.name.clone();
        for p in &closure {
            let Some(f) = self.files.get(p) else { continue };
            if let Some(def) = f.symbols.iter().find(|s| s.name == name) {
                return Some(json!({
                    "uri": uri_from_path(p),
                    "range": span(def.line, def.start_col, def.end_col),
                }));
            }
        }
        None
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
        let closure = self.include_closure(&path);
        let file = self.files.get(&path)?;
        let ident = Self::ident_at(file, line as u32, character as u32)?;
        let target = ident.name.clone();
        // Only resolve names that are actually defined somewhere in the closure.
        if !closure
            .iter()
            .filter_map(|p| self.files.get(p))
            .any(|f| f.symbols.iter().any(|s| s.name == target))
        {
            return None;
        }
        // Gather every occurrence across the current file and its includes.
        let mut out = Vec::new();
        for p in &closure {
            let Some(f) = self.files.get(p) else { continue };
            let uri_str = uri_from_path(p);
            for i in &f.idents {
                if i.name == target && (include_decl || !i.is_def) {
                    out.push(json!({ "uri": uri_str, "range": span(i.line, i.start_col, i.end_col) }));
                }
            }
        }
        Some(out)
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
        let closure = self.include_closure(&path);

        let mut items: Vec<Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Symbols from the current file and every `.include`d file.
        for p in &closure {
            let Some(f) = self.files.get(p) else { continue };
            let from_include = p != &path;
            for s in &f.symbols {
                if seen.insert(s.name.clone()) {
                    let detail = if from_include {
                        format!(
                            "{} (from {})",
                            s.kind.noun(),
                            p.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        )
                    } else {
                        s.kind.noun().to_string()
                    };
                    items.push(json!({
                        "label": s.name,
                        "kind": s.kind.lsp_completion_kind(),
                        "detail": detail,
                    }));
                }
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

        // Whether this line sits inside a `.macro` body — captured before this
        // line's own block open/close, so the `.macro foo` definition line (and
        // its macro-name symbol) is *not* counted as inside the body.
        let in_macro_body = block_stack.iter().any(|(k, _)| *k == BlockKind::Macro);

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
        // A `.macro name p1, p2` / `%macro name N` line: the tokens after the
        // name are *parameter declarations*, not references to global symbols.
        let mut is_macro_def_line = false;
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
                    in_macro: in_macro_body,
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
                is_macro_def_line = kind == SymKind::Macro;
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
                        in_macro: in_macro_body,
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
                let is_assign = matches!(toks.get(idx + 1), Some(Tok::Punct { ch: '=', .. }))
                    || matches!(
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
                            in_macro: in_macro_body,
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
                    // Parameter declarations on a `.macro name p1, p2` line are
                    // macro-locals, not global symbol references.
                    if is_macro_def_line {
                        continue;
                    }
                    // A `\name` macro-parameter reference (GAS) — local to the
                    // macro body, not a global symbol. The `\` sits just before
                    // the word in the comment-stripped line.
                    if *start > 0 && chars.get(*start as usize - 1) == Some(&'\\') {
                        continue;
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
    if matches!(
        toks.first(),
        Some(Tok::Word {
            register: false,
            ..
        }) | Some(Tok::Num { .. })
    ) && matches!(toks.get(1), Some(Tok::Punct { ch: ':', .. }))
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
            h if h.starts_with(".if")
                || h == "if"
                || h.starts_with("ifdef")
                || h.starts_with("ifndef") =>
            {
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
    if matches!(
        toks.first(),
        Some(Tok::Word {
            register: false,
            ..
        })
    ) && matches!(toks.get(1), Some(Tok::Punct { ch: ':', .. }))
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
    (
        ".data",
        "Switch to the initialised read/write data section.",
    ),
    (".bss", "Switch to the zero-initialised data section."),
    (".rodata", "Switch to the read-only data section."),
    (".section", "Switch to (or declare) a named section."),
    (".global", "Make a symbol visible to the linker."),
    (".globl", "Make a symbol visible to the linker."),
    (".extern", "Declare a symbol defined in another object."),
    (".local", "Mark a symbol as local to this object."),
    (".weak", "Declare a weak symbol."),
    (
        ".type",
        "Set a symbol's type (e.g. `@function`, `@object`).",
    ),
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
    (
        ".equiv",
        "Define a symbol equal to an expression (error if already set).",
    ),
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
    (
        ".cfi_startproc",
        "Begin a call-frame-information procedure.",
    ),
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
const X86_INSTRUCTIONS: &[(&str, &str)] = &[
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
    (
        "test",
        "Bitwise AND that sets flags, discarding the result.",
    ),
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

fn instruction_doc(name: &str, arch: Arch) -> Option<&'static str> {
    arch.instruction_tables()
        .iter()
        .find_map(|table| lookup_ci(table, name))
}

/// Common x86-64 registers (names without the AT&T `%` sigil).
const X86_REGISTERS: &[(&str, &str)] = &[
    ("rax", "64-bit accumulator / integer return value (SysV)."),
    ("rbx", "64-bit general-purpose (callee-saved)."),
    (
        "rcx",
        "64-bit general-purpose / 4th integer argument (SysV).",
    ),
    (
        "rdx",
        "64-bit general-purpose / 3rd integer argument (SysV).",
    ),
    (
        "rsi",
        "64-bit general-purpose / 2nd integer argument (SysV).",
    ),
    (
        "rdi",
        "64-bit general-purpose / 1st integer argument (SysV).",
    ),
    ("rbp", "64-bit base / frame pointer (callee-saved)."),
    ("rsp", "64-bit stack pointer."),
    (
        "r8",
        "64-bit general-purpose / 5th integer argument (SysV).",
    ),
    (
        "r9",
        "64-bit general-purpose / 6th integer argument (SysV).",
    ),
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

/// Common AArch64 (ARM64) instruction mnemonics.
const AARCH64_INSTRUCTIONS: &[(&str, &str)] = &[
    ("mov", "Copy a register or immediate into a register."),
    ("movz", "Move a 16-bit immediate, zeroing the rest."),
    ("movk", "Move a 16-bit immediate, keeping other bits."),
    ("movn", "Move the bitwise-NOT of a 16-bit immediate."),
    ("ldr", "Load a register from memory."),
    ("ldp", "Load a pair of registers from memory."),
    ("str", "Store a register to memory."),
    ("stp", "Store a pair of registers to memory."),
    ("adr", "Compute a PC-relative address."),
    ("adrp", "Compute a PC-relative page address."),
    ("add", "Integer addition."),
    ("sub", "Integer subtraction."),
    ("subs", "Subtract and set condition flags."),
    ("mul", "Integer multiply."),
    ("madd", "Multiply-add."),
    ("sdiv", "Signed integer divide."),
    ("udiv", "Unsigned integer divide."),
    ("neg", "Negate (two's complement)."),
    ("and", "Bitwise AND."),
    ("orr", "Bitwise OR."),
    ("eor", "Bitwise exclusive-OR."),
    ("lsl", "Logical shift left."),
    ("lsr", "Logical shift right."),
    ("asr", "Arithmetic shift right."),
    ("cmp", "Compare (subtract and set flags)."),
    ("cmn", "Compare negative (add and set flags)."),
    ("tst", "Test bits (AND and set flags)."),
    ("b", "Unconditional branch."),
    ("bl", "Branch with link (call)."),
    ("blr", "Branch with link to register (indirect call)."),
    ("br", "Branch to register (indirect jump)."),
    ("ret", "Return from subroutine (branch to LR)."),
    ("cbz", "Compare and branch if zero."),
    ("cbnz", "Compare and branch if non-zero."),
    ("b.eq", "Branch if equal (Z=1)."),
    ("b.ne", "Branch if not equal (Z=0)."),
    ("csel", "Conditionally select between two registers."),
    ("svc", "Supervisor call (system call)."),
    ("nop", "No operation."),
];

/// Common AArch64 (ARM64) registers.
const AARCH64_REGISTERS: &[(&str, &str)] = &[
    ("x0", "64-bit general-purpose / 1st argument & return value."),
    ("x1", "64-bit general-purpose / 2nd argument."),
    ("x2", "64-bit general-purpose / 3rd argument."),
    ("x3", "64-bit general-purpose / 4th argument."),
    ("x4", "64-bit general-purpose / 5th argument."),
    ("x5", "64-bit general-purpose / 6th argument."),
    ("x6", "64-bit general-purpose / 7th argument."),
    ("x7", "64-bit general-purpose / 8th argument."),
    ("x8", "64-bit indirect-result / syscall number register."),
    ("x9", "64-bit general-purpose (caller-saved)."),
    ("x16", "64-bit intra-procedure-call scratch (IP0)."),
    ("x17", "64-bit intra-procedure-call scratch (IP1)."),
    ("x18", "64-bit platform register (reserved on some ABIs)."),
    ("x19", "64-bit general-purpose (callee-saved)."),
    ("x29", "Frame pointer (FP)."),
    ("x30", "Link register (LR) — return address."),
    ("w0", "Low 32 bits of X0."),
    ("w1", "Low 32 bits of X1."),
    ("sp", "Stack pointer."),
    ("lr", "Link register (alias of X30)."),
    ("fp", "Frame pointer (alias of X29)."),
    ("xzr", "64-bit zero register (reads 0, writes discarded)."),
    ("wzr", "32-bit zero register."),
    ("pc", "Program counter."),
];

/// Common RISC-V instruction mnemonics (RV32/RV64 base + M extension).
const RISCV_INSTRUCTIONS: &[(&str, &str)] = &[
    ("li", "Load immediate (pseudo-instruction)."),
    ("la", "Load address (pseudo-instruction)."),
    ("mv", "Copy a register (pseudo for `addi rd, rs, 0`)."),
    ("lw", "Load 32-bit word from memory."),
    ("ld", "Load 64-bit doubleword (RV64)."),
    ("sw", "Store 32-bit word to memory."),
    ("sd", "Store 64-bit doubleword (RV64)."),
    ("lui", "Load upper immediate."),
    ("auipc", "Add upper immediate to PC."),
    ("add", "Integer addition."),
    ("addi", "Add immediate."),
    ("sub", "Integer subtraction."),
    ("mul", "Integer multiply (M extension)."),
    ("div", "Signed divide (M extension)."),
    ("rem", "Signed remainder (M extension)."),
    ("and", "Bitwise AND."),
    ("or", "Bitwise OR."),
    ("xor", "Bitwise exclusive-OR."),
    ("andi", "Bitwise AND with immediate."),
    ("ori", "Bitwise OR with immediate."),
    ("sll", "Shift left logical."),
    ("srl", "Shift right logical."),
    ("sra", "Shift right arithmetic."),
    ("slt", "Set if less than (signed)."),
    ("sltu", "Set if less than (unsigned)."),
    ("beq", "Branch if equal."),
    ("bne", "Branch if not equal."),
    ("blt", "Branch if less than (signed)."),
    ("bge", "Branch if greater or equal (signed)."),
    ("j", "Unconditional jump (pseudo)."),
    ("jal", "Jump and link (call)."),
    ("jalr", "Jump and link register (indirect call/return)."),
    ("ret", "Return (pseudo for `jalr x0, ra, 0`)."),
    ("call", "Call a far subroutine (pseudo)."),
    ("ecall", "Environment call (system call)."),
    ("ebreak", "Environment breakpoint."),
    ("nop", "No operation (pseudo for `addi x0, x0, 0`)."),
];

/// Common RISC-V registers (ABI names; numeric `x0`–`x31` also map here).
const RISCV_REGISTERS: &[(&str, &str)] = &[
    ("zero", "Hard-wired zero (x0)."),
    ("ra", "Return address (x1, caller-saved)."),
    ("sp", "Stack pointer (x2, callee-saved)."),
    ("gp", "Global pointer (x3)."),
    ("tp", "Thread pointer (x4)."),
    ("t0", "Temporary (x5, caller-saved)."),
    ("t1", "Temporary (x6, caller-saved)."),
    ("t2", "Temporary (x7, caller-saved)."),
    ("s0", "Saved register / frame pointer (x8, callee-saved)."),
    ("fp", "Frame pointer (alias of s0/x8)."),
    ("s1", "Saved register (x9, callee-saved)."),
    ("a0", "Function argument / return value (x10)."),
    ("a1", "Function argument / return value (x11)."),
    ("a2", "Function argument (x12)."),
    ("a3", "Function argument (x13)."),
    ("a4", "Function argument (x14)."),
    ("a5", "Function argument (x15)."),
    ("a6", "Function argument (x16)."),
    ("a7", "Function argument / syscall number (x17)."),
    ("s2", "Saved register (x18, callee-saved)."),
    ("t3", "Temporary (x28, caller-saved)."),
    ("pc", "Program counter."),
];

fn register_doc(name: &str, arch: Arch) -> Option<&'static str> {
    arch.register_tables()
        .iter()
        .find_map(|table| lookup_ci(table, name))
}

/// Case-insensitive lookup in a `(key, description)` table.
fn lookup_ci(table: &[(&str, &'static str)], name: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, desc)| *desc)
}

/// CPU architecture for instruction/register help dispatch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arch {
    X86_64,
    Aarch64,
    RiscV,
    /// Arch not recognised — help falls back to trying every table.
    Unknown,
}

impl Arch {
    fn host() -> Arch {
        Arch::from_target(std::env::consts::ARCH)
    }

    /// Classify a target arch or triple string (`"aarch64"`, `"arm64"`,
    /// `"riscv64gc-unknown-linux-gnu"`, `"x86_64"`, …).
    fn from_target(s: &str) -> Arch {
        let s = s.to_ascii_lowercase();
        if s.contains("aarch64") || s.contains("arm64") {
            Arch::Aarch64
        } else if s.contains("riscv") {
            Arch::RiscV
        } else if s.contains("x86_64") || s.contains("amd64") || s.contains("x86") || s == "i686" {
            Arch::X86_64
        } else {
            Arch::Unknown
        }
    }

    /// Instruction tables to consult, the detected arch first. `Unknown` tries
    /// all so hover still works when the arch can't be determined.
    fn instruction_tables(self) -> &'static [&'static [(&'static str, &'static str)]] {
        match self {
            Arch::X86_64 => &[X86_INSTRUCTIONS],
            Arch::Aarch64 => &[AARCH64_INSTRUCTIONS],
            Arch::RiscV => &[RISCV_INSTRUCTIONS],
            Arch::Unknown => &[X86_INSTRUCTIONS, AARCH64_INSTRUCTIONS, RISCV_INSTRUCTIONS],
        }
    }

    /// Register tables to consult, the detected arch first.
    fn register_tables(self) -> &'static [&'static [(&'static str, &'static str)]] {
        match self {
            Arch::X86_64 => &[X86_REGISTERS],
            Arch::Aarch64 => &[AARCH64_REGISTERS],
            Arch::RiscV => &[RISCV_REGISTERS],
            Arch::Unknown => &[X86_REGISTERS, AARCH64_REGISTERS, RISCV_REGISTERS],
        }
    }
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
        assert!(instruction_doc("MOV", Arch::X86_64).is_some()); // case-insensitive
        assert!(register_doc("RAX", Arch::X86_64).is_some());
        assert!(register_doc("not_a_reg", Arch::X86_64).is_none());
    }

    #[test]
    fn arch_detection_and_per_arch_help() {
        assert_eq!(Arch::from_target("aarch64-unknown-linux-gnu"), Arch::Aarch64);
        assert_eq!(Arch::from_target("arm64"), Arch::Aarch64);
        assert_eq!(Arch::from_target("riscv64gc-unknown-linux-gnu"), Arch::RiscV);
        assert_eq!(Arch::from_target("x86_64"), Arch::X86_64);
        assert_eq!(Arch::from_target("sparc"), Arch::Unknown);

        // Arch-specific registers resolve under their arch.
        assert!(register_doc("x0", Arch::Aarch64).is_some());
        assert!(register_doc("a7", Arch::RiscV).is_some());
        assert!(instruction_doc("bl", Arch::Aarch64).is_some());
        assert!(instruction_doc("ecall", Arch::RiscV).is_some());

        // Unknown arch falls back to trying every table.
        assert!(register_doc("x0", Arch::Unknown).is_some());
        assert!(register_doc("rax", Arch::Unknown).is_some());
    }

    #[test]
    fn macro_body_labels_excluded_from_duplicate_diagnostics() {
        // Two macros each define an internal `.Lloop` label (templated via `\@`
        // at assembly time). Our model collapses the name, but macro-body symbols
        // must not be flagged as duplicates.
        let src = "\
.macro one
.Lloop\\@:
    nop
.endm
.macro two
.Lloop\\@:
    nop
.endm
";
        let f = analyze(src);
        let loop_syms: Vec<&Symbol> = f.symbols.iter().filter(|s| s.name == ".Lloop").collect();
        assert_eq!(loop_syms.len(), 2);
        assert!(loop_syms.iter().all(|s| s.in_macro));
        // The macro names themselves are not inside a body.
        assert!(f
            .symbols
            .iter()
            .any(|s| s.name == "one" && !s.in_macro));
    }

    #[test]
    fn macro_parameter_refs_are_not_global_idents() {
        // `\count` is a macro parameter, not a reference to a global symbol.
        let src = ".macro fill count\n    .rept \\count\n    nop\n    .endr\n.endm\n";
        let f = analyze(src);
        assert!(
            !f.idents.iter().any(|i| i.name == "count" && !i.is_def),
            "macro-parameter ref `\\count` must not be recorded as a symbol use"
        );
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
    fn cross_file_include_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let inc = tmp.path().join("helper.inc");
        let main = tmp.path().join("main.s");
        std::fs::write(
            &inc,
            ".equ WIDTH, 80\n.macro prologue\n    push %rbp\n.endm\n",
        )
        .unwrap();
        std::fs::write(
            &main,
            ".include \"helper.inc\"\nmain:\n    mov $WIDTH, %eax\n    prologue\n",
        )
        .unwrap();

        let main_uri = uri_from_path(&main);
        let pos = |line: u32, ch: u32| {
            json!({ "params": { "textDocument": { "uri": main_uri },
                                "position": { "line": line, "character": ch } } })
        };

        let mut ix = AsmIndexer::new();

        // goto `WIDTH` (operand on line 2) → its `.equ` in helper.inc.
        let g = ix.goto_definition(&main_uri, &pos(2, 10)).expect("goto WIDTH");
        assert_eq!(g["uri"], json!(uri_from_path(&inc)));
        assert_eq!(g["range"]["start"]["line"], json!(0));

        // hover `prologue` (line 3) → macro defined in the included file.
        let h = ix.hover(&main_uri, &pos(3, 6)).expect("hover prologue");
        let text = h["contents"]["value"].as_str().unwrap();
        assert!(text.contains("macro") && text.contains("helper.inc"), "{text}");

        // completion offers symbols from the included file.
        let c = ix.completion(&main_uri, &pos(4, 0)).expect("completion");
        let labels: Vec<&str> = c["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["label"].as_str())
            .collect();
        assert!(labels.contains(&"WIDTH"));
        assert!(labels.contains(&"prologue"));

        // references to WIDTH find the use in main.s.
        let refs = ix
            .references(
                &main_uri,
                &json!({ "params": { "textDocument": { "uri": main_uri },
                                     "position": { "line": 2, "character": 10 },
                                     "context": { "includeDeclaration": true } } }),
            )
            .expect("references WIDTH");
        assert!(refs.iter().any(|r| r["uri"] == json!(main_uri)));
        assert!(refs.iter().any(|r| r["uri"] == json!(uri_from_path(&inc))));
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
