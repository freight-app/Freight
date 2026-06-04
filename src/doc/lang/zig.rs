use super::common::{build_item, collect_line_block, item_has_content, next_non_blank};
use super::{DocExtractor, DocItem, DocKind, DocLanguage};
use std::path::Path;

pub struct ZigExtractor;

impl DocExtractor for ZigExtractor {
    fn extensions(&self) -> &[&str] {
        &["zig"]
    }
    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_zig(src, path)
    }
}

pub(super) fn extract_zig(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();

        // `///` doc comments — but NOT `////` which is a section separator
        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_zig_symbol(sym);
            let item = build_item(
                block,
                name,
                kind,
                file,
                i + 1,
                DocLanguage::Zig,
                sym.to_string(),
            );
            if item_has_content(&item) {
                items.push(item);
            }
            i = end + 1;
            continue;
        }

        i += 1;
    }
    items
}

fn detect_zig_symbol(line: &str) -> (String, DocKind) {
    let t = line.trim();

    // Strip `pub` visibility prefix if present
    let t = t.strip_prefix("pub").map(|r| r.trim_start()).unwrap_or(t);

    if let Some(r) = t.strip_prefix("fn ") {
        return (first_ident_zig(r), DocKind::Function);
    }
    if let Some(r) = t.strip_prefix("const ") {
        return (first_ident_zig(r), DocKind::Variable);
    }
    if let Some(r) = t.strip_prefix("var ") {
        return (first_ident_zig(r), DocKind::Variable);
    }
    if let Some(r) = t.strip_prefix("struct ") {
        return (first_ident_zig(r), DocKind::Struct);
    }
    if let Some(r) = t.strip_prefix("enum ") {
        return (first_ident_zig(r), DocKind::Enum);
    }
    if let Some(r) = t.strip_prefix("union ") {
        return (first_ident_zig(r), DocKind::Struct);
    }
    if let Some(r) = t.strip_prefix("error ") {
        return (first_ident_zig(r), DocKind::Enum);
    }

    (String::new(), DocKind::Unknown)
}

/// Extract the first identifier from a Zig declaration fragment.
/// Stops at whitespace, `(`, `:`, `=`, `{`, or `<`.
fn first_ident_zig(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn items(src: &str) -> Vec<DocItem> {
        extract_zig(src, Path::new("test.zig"))
    }

    #[test]
    fn zig_fn_doc() {
        let src = "/// Compute the absolute value of x.\npub fn abs(x: i64) i64 { return if (x < 0) -x else x; }";
        let got = items(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Compute the absolute value of x.");
        assert_eq!(got[0].name, "abs");
        assert!(matches!(got[0].kind, DocKind::Function));
    }

    #[test]
    fn zig_struct_doc() {
        let src = "/// A 2D vector.\npub const Vec2 = struct {\n    x: f32,\n    y: f32,\n};";
        let got = items(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "A 2D vector.");
        assert_eq!(got[0].name, "Vec2");
    }

    #[test]
    fn zig_enum_doc() {
        let src = "/// Error codes returned by the API.\npub const ApiError = enum {\n    NotFound,\n    Timeout,\n};";
        let got = items(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Error codes returned by the API.");
        assert_eq!(got[0].name, "ApiError");
        assert!(matches!(got[0].kind, DocKind::Variable));
    }

    #[test]
    fn zig_section_separator_not_extracted() {
        // `////` lines are section separators, not doc comments
        let src = "////////////////////\npub fn foo() void {}";
        let got = items(src);
        assert!(got.is_empty(), "section separators must not be extracted");
    }

    #[test]
    fn zig_no_doc_comment_not_extracted() {
        let src = "// Just a regular comment.\npub fn foo() void {}";
        let got = items(src);
        assert!(got.is_empty(), "regular // comments must not be extracted");
    }

    #[test]
    fn zig_multiline_doc() {
        let src = "/// Add two integers.\n/// Returns the sum of a and b.\npub fn add(a: i32, b: i32) i32 { return a + b; }";
        let got = items(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Add two integers.");
        assert!(got[0].body.contains("Returns the sum"));
    }

    #[test]
    fn zig_pub_fn_without_pub() {
        let src = "/// Internal helper.\nfn helper(x: u32) u32 { return x; }";
        let got = items(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "helper");
        assert!(matches!(got[0].kind, DocKind::Function));
    }
}
