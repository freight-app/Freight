//! Lightweight Fortran intelligence inspired by fortls.
//!
//! This is not a full compiler front-end. It intentionally implements the pieces
//! that make Freight projects pleasant out-of-the-box when users do not have an
//! external `fortls` process configured: symbol indexing, simple structural
//! diagnostics, hover text, completion labels, and definition lookup for open
//! Fortran documents.

use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, DocumentSymbol, Position,
    Range, SymbolKind,
};

use crate::position::{byte_to_position, position_to_byte};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FortranKind {
    Program,
    Module,
    Subroutine,
    Function,
    Type,
}

impl FortranKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::Module => "module",
            Self::Subroutine => "subroutine",
            Self::Function => "function",
            Self::Type => "type",
        }
    }

    fn symbol_kind(self) -> SymbolKind {
        match self {
            Self::Program => SymbolKind::NAMESPACE,
            Self::Module => SymbolKind::MODULE,
            Self::Subroutine => SymbolKind::FUNCTION,
            Self::Function => SymbolKind::FUNCTION,
            Self::Type => SymbolKind::STRUCT,
        }
    }

    fn completion_kind(self) -> CompletionItemKind {
        match self {
            Self::Program | Self::Module => CompletionItemKind::MODULE,
            Self::Subroutine | Self::Function => CompletionItemKind::FUNCTION,
            Self::Type => CompletionItemKind::STRUCT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FortranSymbol {
    pub name: String,
    pub kind: FortranKind,
    pub range: Range,
    pub selection_range: Range,
}

impl FortranSymbol {
    pub fn detail(&self) -> String {
        format!("Fortran {}", self.kind.as_str())
    }

    pub fn to_document_symbol(&self) -> DocumentSymbol {
        DocumentSymbol {
            name: self.name.clone(),
            detail: Some(self.detail()),
            kind: self.kind.symbol_kind(),
            tags: None,
            deprecated: None,
            range: self.range,
            selection_range: self.selection_range,
            children: None,
        }
    }

    pub fn to_completion_item(&self) -> CompletionItem {
        CompletionItem {
            label: self.name.clone(),
            kind: Some(self.kind.completion_kind()),
            detail: Some(self.detail()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct FortranAnalysis {
    pub symbols: Vec<FortranSymbol>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
struct OpenBlock {
    kind: FortranKind,
    name: String,
    line: u32,
    start: u32,
}

pub fn analyze(src: &str) -> FortranAnalysis {
    let mut analysis = FortranAnalysis::default();
    let mut stack: Vec<OpenBlock> = Vec::new();

    for (line_idx, raw_line) in src.lines().enumerate() {
        let line_no = line_idx as u32;
        let code = strip_comment(raw_line);
        let trimmed = code.trim_start();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((end_kind, end_name, start_col)) = parse_end(trimmed) {
            close_block(
                &mut stack,
                &mut analysis.diagnostics,
                end_kind,
                end_name,
                line_no,
                start_col,
            );
            continue;
        }

        if let Some((kind, name, col_in_trimmed)) = parse_start(trimmed) {
            let leading = raw_line.len().saturating_sub(trimmed.len()) as u32;
            let start = leading + col_in_trimmed;
            let end = start + name.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
            let selection_range = Range {
                start: Position {
                    line: line_no,
                    character: start,
                },
                end: Position {
                    line: line_no,
                    character: end,
                },
            };
            let range = Range {
                start: Position {
                    line: line_no,
                    character: 0,
                },
                end: Position {
                    line: line_no,
                    character: raw_line.chars().map(|c| c.len_utf16() as u32).sum(),
                },
            };
            analysis.symbols.push(FortranSymbol {
                name: name.clone(),
                kind,
                range,
                selection_range,
            });
            stack.push(OpenBlock {
                kind,
                name,
                line: line_no,
                start,
            });
        }
    }

    for block in stack.into_iter().rev() {
        analysis.diagnostics.push(diagnostic(
            Range {
                start: Position {
                    line: block.line,
                    character: block.start,
                },
                end: Position {
                    line: block.line,
                    character: block.start + block.name.len() as u32,
                },
            },
            format!("unclosed Fortran {} `{}`", block.kind.as_str(), block.name),
        ));
    }

    analysis
}

pub fn completions(src: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = analyze(src)
        .symbols
        .into_iter()
        .map(|s| s.to_completion_item())
        .collect();

    items.extend(
        FORTRAN_KEYWORDS
            .iter()
            .map(|(label, detail)| CompletionItem {
                label: (*label).to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some((*detail).to_string()),
                ..Default::default()
            }),
    );

    items.extend(
        FORTRAN_SNIPPETS
            .iter()
            .map(|(label, snippet, detail)| CompletionItem {
                label: (*label).to_string(),
                kind: Some(CompletionItemKind::SNIPPET),
                detail: Some((*detail).to_string()),
                insert_text: Some((*snippet).to_string()),
                insert_text_format: Some(tower_lsp::lsp_types::InsertTextFormat::SNIPPET),
                ..Default::default()
            }),
    );

    items
}

pub fn identifier_at(src: &str, pos: Position) -> Option<String> {
    let byte = position_to_byte(src, pos).min(src.len());
    let bytes = src.as_bytes();

    let mut start = byte;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }

    let mut end = byte;
    while end < bytes.len() && is_ident_byte(bytes[end]) {
        end += 1;
    }

    if start == end {
        None
    } else {
        Some(src[start..end].to_string())
    }
}

pub fn hover(src: &str, pos: Position) -> Option<String> {
    let ident = identifier_at(src, pos)?;
    let sym = find_symbol(src, &ident)?;
    Some(format!("**Fortran {}** `{}`", sym.kind.as_str(), sym.name))
}

pub fn find_symbol(src: &str, name: &str) -> Option<FortranSymbol> {
    analyze(src)
        .symbols
        .into_iter()
        .find(|sym| sym.name.eq_ignore_ascii_case(name))
}

fn close_block(
    stack: &mut Vec<OpenBlock>,
    diagnostics: &mut Vec<Diagnostic>,
    end_kind: Option<FortranKind>,
    end_name: Option<String>,
    line: u32,
    character: u32,
) {
    let Some(open) = stack.pop() else {
        diagnostics.push(diagnostic(
            Range {
                start: Position { line, character },
                end: Position {
                    line,
                    character: character + 3,
                },
            },
            "unmatched Fortran end statement".to_string(),
        ));
        return;
    };

    if let Some(kind) = end_kind {
        if open.kind != kind {
            diagnostics.push(diagnostic(
                Range {
                    start: Position { line, character },
                    end: Position {
                        line,
                        character: character + kind.as_str().len() as u32,
                    },
                },
                format!(
                    "expected `end {}` for `{}`, found `end {}`",
                    open.kind.as_str(),
                    open.name,
                    kind.as_str()
                ),
            ));
            return;
        }
    }

    if let Some(name) = end_name {
        if !name.eq_ignore_ascii_case(&open.name) {
            diagnostics.push(diagnostic(
                Range {
                    start: Position { line, character },
                    end: Position {
                        line,
                        character: character + name.len() as u32,
                    },
                },
                format!("expected end name `{}`, found `{name}`", open.name),
            ));
        }
    }
}

fn diagnostic(range: Range, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        source: Some("freight-fortran".into()),
        message,
        ..Default::default()
    }
}

fn parse_start(line: &str) -> Option<(FortranKind, String, u32)> {
    let lower = line.to_ascii_lowercase();
    let lower = lower.trim_start();

    if lower.starts_with("module procedure") {
        return None;
    }

    for (prefix, kind) in [
        ("program", FortranKind::Program),
        ("module", FortranKind::Module),
        ("subroutine", FortranKind::Subroutine),
        ("function", FortranKind::Function),
    ] {
        if let Some(rest) = keyword_rest(lower, prefix) {
            return parse_name_after(line, rest, kind);
        }
    }

    if let Some(rest) = keyword_rest(lower, "type") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            return None;
        }
        let rest = rest
            .split_once("::")
            .map(|(_, name)| name)
            .unwrap_or(rest)
            .trim_start();
        return parse_named_rest(line, rest, FortranKind::Type);
    }

    parse_prefixed_procedure(line, lower, "subroutine", FortranKind::Subroutine)
        .or_else(|| parse_prefixed_procedure(line, lower, "function", FortranKind::Function))
}

fn parse_prefixed_procedure(
    original: &str,
    lower: &str,
    keyword: &str,
    kind: FortranKind,
) -> Option<(FortranKind, String, u32)> {
    let marker = format!(" {keyword} ");
    let idx = lower.find(&marker)?;
    let before = lower[..idx].trim();
    if before.contains("end") || before.contains("procedure") {
        return None;
    }
    let rest = &lower[idx + marker.len()..];
    parse_named_rest(original, rest, kind)
}

fn parse_end(line: &str) -> Option<(Option<FortranKind>, Option<String>, u32)> {
    let lower = line.to_ascii_lowercase();
    let lower = lower.trim_start();
    if !keyword_rest(lower, "end").is_some() {
        return None;
    }

    let leading = line.len().saturating_sub(line.trim_start().len()) as u32;
    let rest = keyword_rest(lower, "end").unwrap_or("").trim_start();
    if rest.is_empty() {
        return Some((None, None, leading));
    }

    // Ignore non-program-unit control constructs so common blocks such as
    // `end if` and `end do` do not produce false unmatched-end diagnostics.
    if [
        "if",
        "do",
        "select",
        "where",
        "associate",
        "block",
        "forall",
        "critical",
        "enum",
        "interface",
        "procedure",
    ]
    .iter()
    .any(|kw| keyword_rest(rest, kw).is_some())
    {
        return None;
    }

    for (kw, kind) in [
        ("program", FortranKind::Program),
        ("module", FortranKind::Module),
        ("subroutine", FortranKind::Subroutine),
        ("function", FortranKind::Function),
        ("type", FortranKind::Type),
    ] {
        if let Some(after_kind) = keyword_rest(rest, kw) {
            let name = first_identifier(after_kind.trim_start()).map(str::to_string);
            return Some((Some(kind), name, leading));
        }
    }

    Some((None, first_identifier(rest).map(str::to_string), leading))
}

fn parse_name_after(
    original: &str,
    lower_rest: &str,
    kind: FortranKind,
) -> Option<(FortranKind, String, u32)> {
    parse_named_rest(original, lower_rest.trim_start(), kind)
}

fn parse_named_rest(
    original: &str,
    lower_rest: &str,
    kind: FortranKind,
) -> Option<(FortranKind, String, u32)> {
    let name = first_identifier(lower_rest)?;
    let lower_original = original.to_ascii_lowercase();
    let byte = lower_original.find(name)?;
    let pos = byte_to_position(original, byte);
    Some((
        kind,
        original[byte..byte + name.len()].to_string(),
        pos.character,
    ))
}

fn keyword_rest<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    if rest.is_empty() || rest.starts_with(|c: char| !is_ident_char(c)) {
        Some(rest)
    } else {
        None
    }
}

fn first_identifier(s: &str) -> Option<&str> {
    let start = s.find(is_ident_char)?;
    let rest = &s[start..];
    let end = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn strip_comment(line: &str) -> &str {
    // Good enough for structural editing: honor quotes so strings containing `!`
    // do not truncate the line.
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '!' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

const FORTRAN_KEYWORDS: &[(&str, &str)] = &[
    ("program", "Program unit"),
    ("module", "Module unit"),
    ("subroutine", "Subroutine procedure"),
    ("function", "Function procedure"),
    ("use", "Import a module"),
    ("implicit none", "Disable implicit typing"),
    ("contains", "Begin module/program procedures"),
    ("type", "Derived type"),
    ("integer", "Integer declaration"),
    ("real", "Real declaration"),
    ("logical", "Logical declaration"),
    ("character", "Character declaration"),
    ("allocatable", "Allocatable attribute"),
    ("intent", "Dummy argument intent"),
];

const FORTRAN_SNIPPETS: &[(&str, &str, &str)] = &[
    (
        "program skeleton",
        "program ${1:main}\n    implicit none\n    ${2}\nend program ${1:main}",
        "Create a program unit",
    ),
    (
        "module skeleton",
        "module ${1:name}\n    implicit none\ncontains\n    ${2}\nend module ${1:name}",
        "Create a module unit",
    ),
    (
        "subroutine skeleton",
        "subroutine ${1:name}(${2})\n    implicit none\n    ${3}\nend subroutine ${1:name}",
        "Create a subroutine",
    ),
    (
        "function skeleton",
        "function ${1:name}(${2}) result(${3:res})\n    implicit none\n    ${4}\nend function ${1:name}",
        "Create a function",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_fortran_symbols() {
        let src = "module math\ncontains\nsubroutine add()\nend subroutine add\nend module math\n";
        let analysis = analyze(src);
        assert!(analysis.diagnostics.is_empty());
        assert!(analysis
            .symbols
            .iter()
            .any(|s| s.name == "math" && s.kind == FortranKind::Module));
        assert!(analysis
            .symbols
            .iter()
            .any(|s| s.name == "add" && s.kind == FortranKind::Subroutine));
    }

    #[test]
    fn reports_mismatched_end() {
        let src = "module math\nend program math\n";
        let analysis = analyze(src);
        assert!(!analysis.diagnostics.is_empty());
    }

    #[test]
    fn ignores_control_construct_end_statements() {
        let src = "program main
if (.true.) then
end if
end program main
";
        let analysis = analyze(src);
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn hover_finds_symbol_case_insensitively() {
        let src = "module Math\nend module Math\n";
        let pos = Position {
            line: 0,
            character: 2,
        };
        assert!(hover(src, pos).unwrap().contains("Math"));
    }
}
