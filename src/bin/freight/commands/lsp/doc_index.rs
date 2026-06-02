//! Symbol documentation index built from the project's source files via docify.
//!
//! The index is rebuilt whenever `freight.toml` is saved (same cadence as
//! `compile_commands.json`). Hover requests for source files look up the symbol
//! at the cursor position and return formatted Markdown documentation before
//! falling back to the passthrough language server (clangd/fortls/asm-lsp).

use std::collections::HashMap;
use std::path::Path;

use docify::extract::{DocItem, DocKind, DocLanguage, TagKind, extract_dir};

// ---------------------------------------------------------------------------
// DocIndex
// ---------------------------------------------------------------------------

/// Flat lookup table: symbol name → first matching DocItem.
///
/// Names are stored lowercased for case-insensitive lookup (C++ symbols that
/// differ only in case are rare and the hover UX is best-effort anyway).
pub struct DocIndex {
    /// Lower-case symbol name → owned item.
    by_name: HashMap<String, DocItem>,
}

impl DocIndex {
    pub fn build(src_dir: &Path) -> Self {
        let set = extract_dir(src_dir);
        let mut by_name: HashMap<String, DocItem> = HashMap::new();
        for item in set.items {
            // Only index items that have documentation worth showing.
            if item.brief.is_empty() && item.body.is_empty() {
                continue;
            }
            // Use the simple (unqualified) name for lookup so hovering over a
            // call-site symbol finds the definition regardless of namespace.
            let key = simple_name(&item.name).to_ascii_lowercase();
            by_name.entry(key).or_insert(item);
        }
        Self { by_name }
    }

    /// Look up a symbol by name (case-insensitive).
    pub fn lookup(&self, name: &str) -> Option<&DocItem> {
        self.by_name.get(&name.to_ascii_lowercase())
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

/// Render a `DocItem` to a Markdown string suitable for an LSP hover response.
pub fn item_to_markdown(item: &DocItem) -> String {
    let mut out = String::new();

    // Fenced code block for the signature.
    if !item.signature.is_empty() {
        let lang = lang_id(item.lang.clone());
        out.push_str(&format!("```{lang}\n{}\n```\n\n", item.signature.trim()));
    }

    // Brief (first paragraph of doc comment).
    if !item.brief.is_empty() {
        out.push_str(item.brief.trim());
        out.push_str("\n\n");
    }

    // Extended body.
    if !item.body.is_empty() {
        out.push_str(item.body.trim());
        out.push('\n');
    }

    // Structured tags: @param, @return, @throws, @see, @note, @deprecated …
    let params: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    let returns: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Return).collect();
    let throws: Vec<&_> = item.tags.iter().filter(|t| {
        matches!(&t.kind, TagKind::Other(s) if s.eq_ignore_ascii_case("throws") || s.eq_ignore_ascii_case("exception"))
    }).collect();
    let examples: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Example).collect();
    let sees: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::See).collect();
    let notes: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Note).collect();
    let warnings: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Warning).collect();
    let deprecated: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Deprecated).collect();

    if !deprecated.is_empty() {
        out.push_str("\n> ⚠️ **Deprecated**");
        if let Some(text) = deprecated.first().map(|t| t.text.trim()).filter(|t| !t.is_empty()) {
            out.push_str(&format!(": {text}"));
        }
        out.push_str("\n\n");
    }

    if !params.is_empty() {
        out.push_str("\n**Parameters**\n\n");
        for tag in params {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `{name}` — {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !returns.is_empty() {
        out.push_str("\n**Returns**\n\n");
        for tag in &returns {
            out.push_str(&format!("{}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !throws.is_empty() {
        out.push_str("\n**Throws**\n\n");
        for tag in throws {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `{name}` — {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !notes.is_empty() {
        out.push_str("\n**Notes**\n\n");
        for tag in &notes {
            out.push_str(&format!("> {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !warnings.is_empty() {
        out.push_str("\n**Warning**\n\n");
        for tag in &warnings {
            out.push_str(&format!("> ⚠️ {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !examples.is_empty() {
        for tag in &examples {
            out.push_str("\n**Example**\n\n");
            out.push_str(tag.text.trim());
            out.push('\n');
        }
        out.push('\n');
    }

    if !sees.is_empty() {
        out.push_str("\n**See also**: ");
        let refs: Vec<String> = sees.iter().map(|t| format!("`{}`", t.text.trim())).collect();
        out.push_str(&refs.join(", "));
        out.push('\n');
    }

    // File/line footer.
    let rel = item.file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !rel.is_empty() && item.line > 0 {
        out.push_str(&format!("\n---\n*{}:{}*\n", rel, item.line));
    }

    out
}

// ---------------------------------------------------------------------------
// Word extraction
// ---------------------------------------------------------------------------

/// Extract the identifier word at `(line, character)` from `text`.
pub fn word_at(text: &str, line: usize, character: usize) -> Option<String> {
    let line_text = text.lines().nth(line)?;
    // character is a UTF-16 offset; approximate with byte offset for ASCII-heavy code.
    let char_idx = character.min(line_text.len());
    let before = &line_text[..char_idx];
    let after = &line_text[char_idx..];
    let start = before.rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '~')
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    let end = char_idx + after.find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(after.len());
    if start >= end { return None; }
    let word = &line_text[start..end];
    if word.is_empty() || word.chars().all(|c| c.is_ascii_digit()) { return None; }
    Some(word.to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn simple_name(name: &str) -> &str {
    // Strip leading `~` (destructors) then take the last `::` segment.
    let name = name.trim_start_matches('~');
    name.rsplit("::").next().unwrap_or(name)
}

fn lang_id(lang: DocLanguage) -> &'static str {
    match lang {
        DocLanguage::C         => "c",
        DocLanguage::Cpp       => "cpp",
        DocLanguage::Fortran   => "fortran",
        DocLanguage::Rust      => "rust",
        DocLanguage::Python    => "python",
        DocLanguage::Ada       => "ada",
        DocLanguage::D         => "d",
        _                      => "text",
    }
}

// ---------------------------------------------------------------------------
// Kind label (used for item kind badges)
// ---------------------------------------------------------------------------

pub fn kind_badge(kind: DocKind) -> &'static str {
    match kind {
        DocKind::Function | DocKind::Subroutine => "fn",
        DocKind::Class    | DocKind::Struct      => "struct",
        DocKind::Enum                            => "enum",
        DocKind::Module   | DocKind::Interface   => "mod",
        DocKind::Variable                        => "var",
        DocKind::Typedef                         => "type",
        DocKind::Macro                           => "macro",
        _                                        => "",
    }
}
