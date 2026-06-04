use super::common::{build_item, item_has_content};
use super::{DocExtractor, DocItem, DocKind, DocLanguage};
use std::path::Path;

pub struct AsmExtractor;

impl DocExtractor for AsmExtractor {
    fn extensions(&self) -> &[&str] {
        &["s", "S", "asm", "nasm", "nas", "inc"]
    }

    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_asm(src, path)
    }
}

/// Extract doc comments from assembly source files.
///
/// Supported comment styles:
/// - `;;` double-semicolon (NASM/MASM doc convention)
/// - `##` double-hash (GAS/AT&T doc convention)
/// - `//` C-style (LLVM/ARM assembly)
///
/// A doc block is one or more consecutive doc-comment lines immediately
/// before a label definition (`identifier:`) or a global procedure marker
/// (`.proc`, `.func`, `PROC`, `FUNCTION`).
pub(super) fn extract_asm(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();
        if is_doc_comment(t) {
            // Collect the doc block.
            let block_start = i;
            let mut block = Vec::new();
            while i < lines.len() && is_doc_comment(lines[i].trim()) {
                block.push(strip_doc_prefix(lines[i].trim()));
                i += 1;
            }
            // Skip blank lines between block and declaration.
            while i < lines.len() && lines[i].trim().is_empty() {
                i += 1;
            }
            if i >= lines.len() {
                break;
            }
            let decl_line = lines[i].trim();
            let Some((name, kind)) = detect_asm_symbol(decl_line) else {
                continue;
            };
            let item = build_item(
                block,
                name,
                kind,
                file,
                block_start + 1,
                DocLanguage::Unknown,
                decl_line.to_string(),
            );
            if item_has_content(&item) {
                items.push(item);
            }
        } else {
            i += 1;
        }
    }
    items
}

fn is_doc_comment(line: &str) -> bool {
    line.starts_with(";;")
        || line.starts_with("##")
        || line.starts_with("//")
}

fn strip_doc_prefix(line: &str) -> String {
    if let Some(rest) = line.strip_prefix(";;").or_else(|| line.strip_prefix("##")).or_else(|| line.strip_prefix("//")) {
        rest.trim_start().to_string()
    } else {
        line.to_string()
    }
}

/// Detect a label or procedure declaration.
/// Returns `(name, kind)` or `None` if the line isn't a declaration.
fn detect_asm_symbol(line: &str) -> Option<(String, DocKind)> {
    // Label: identifier followed by `:` (NASM/GAS style)
    if let Some(name) = line.strip_suffix(':').map(str::trim) {
        if is_asm_ident(name) {
            return Some((name.to_string(), DocKind::Function));
        }
    }

    let up = line.to_ascii_uppercase();

    // GAS `.type name, @function`
    if up.starts_with(".TYPE") {
        let rest = line[5..].trim();
        let name = rest.split(',').next()?.trim();
        if is_asm_ident(name) {
            return Some((name.to_string(), DocKind::Function));
        }
    }

    // MASM/TASM PROC
    if let Some(pos) = up.find(" PROC") {
        let name = line[..pos].trim();
        if is_asm_ident(name) {
            return Some((name.to_string(), DocKind::Function));
        }
    }

    // `.proc name` / `.func name`
    if up.starts_with(".PROC ") || up.starts_with(".FUNC ") {
        let name = line[6..].trim().split_whitespace().next()?;
        if is_asm_ident(name) {
            return Some((name.to_string(), DocKind::Function));
        }
    }

    // `global name` / `.global name` — public symbol declaration
    let stripped = up.strip_prefix(".GLOBAL ").or_else(|| up.strip_prefix("GLOBAL "));
    if let Some(_) = stripped {
        let name = line.split_whitespace().nth(1)?;
        let name = name.trim_end_matches(':');
        if is_asm_ident(name) {
            return Some((name.to_string(), DocKind::Function));
        }
    }

    None
}

fn is_asm_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_' || c == '.')
        && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '@')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn label_with_double_semicolon_doc() {
        let src = "\
;; Computes the absolute value of rdi.
;; Returns result in rax.
abs_val:
    mov rax, rdi
    ret
";
        let items = extract_asm(src, Path::new("test.asm"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "abs_val");
        assert!(items[0].brief.contains("absolute value"));
    }

    #[test]
    fn masm_proc_with_doc() {
        let src = "\
;; Multiply two 64-bit integers.
mul64 PROC
    imul rax, rcx
    ret
mul64 ENDP
";
        let items = extract_asm(src, Path::new("test.asm"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "mul64");
    }

    #[test]
    fn no_doc_comment_skipped() {
        let src = "\
; ordinary comment (single semicolon — not a doc comment)
plain_label:
    nop
";
        let items = extract_asm(src, Path::new("test.asm"));
        assert!(items.is_empty());
    }
}
