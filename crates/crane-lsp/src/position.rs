//! Map `ValidationError.context` strings to positions in a `crane.toml` source buffer.
//!
//! `crane-core` reports errors with a free-form context like `"[package]"` or
//! `"[dependencies.foo]"` — no line/column. To highlight the right spot in the
//! editor we do a plain text search over the source. When nothing matches, we
//! fall back to the top of the file so the diagnostic is still visible.

use tower_lsp::lsp_types::{Position, Range};

/// Find a `Range` in `src` covering the first occurrence of `context`.
/// Falls back to the whole first line if no match is found.
pub fn locate(src: &str, context: &str) -> Range {
    // Strip any trailing ".<key>" from table-header style contexts so we still
    // find the section when the key was invented at validate-time (e.g.
    // "[bin.broken]" when the manifest only has "[[bin]]" headers).
    let candidates = expand_context(context);

    for needle in &candidates {
        if let Some(range) = find_str(src, needle) {
            return range;
        }
    }

    // Fallback: first non-empty line.
    let line = src.lines().next().unwrap_or("");
    Range {
        start: Position { line: 0, character: 0 },
        end: Position { line: 0, character: line.chars().count() as u32 },
    }
}

fn expand_context(ctx: &str) -> Vec<String> {
    let mut out = vec![ctx.to_string()];

    // `[dependencies.foo]` → also try the plain key `foo = ` inside `[dependencies]`.
    if let Some(inner) = ctx.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if let Some((_parent, child)) = inner.rsplit_once('.') {
            out.push(format!("{child} ="));
            out.push(format!("{child}="));
        }
    }

    // `[bin.name]` → try `name = "..."` line inside a `[[bin]]` table.
    if let Some(stripped) = ctx.strip_prefix("[bin.").and_then(|s| s.strip_suffix(']')) {
        out.push(format!("name = \"{stripped}\""));
        out.push(format!("name=\"{stripped}\""));
    }

    out
}

fn find_str(src: &str, needle: &str) -> Option<Range> {
    let byte_idx = src.find(needle)?;
    let start = byte_to_position(src, byte_idx);
    let end = byte_to_position(src, byte_idx + needle.len());
    Some(Range { start, end })
}

/// Convert a byte offset in `src` to an LSP `Position` (UTF-16 code units per spec).
pub fn byte_to_position(src: &str, byte_idx: usize) -> Position {
    let byte_idx = byte_idx.min(src.len());
    let mut line = 0u32;
    let mut line_start = 0usize;

    for (i, ch) in src.char_indices() {
        if i >= byte_idx { break; }
        if ch == '\n' {
            line += 1;
            line_start = i + 1;
        }
    }

    // Character offset on the line, counted in UTF-16 code units.
    let column: u32 = src[line_start..byte_idx]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();

    Position { line, character: column }
}

/// Convert an LSP `Position` (UTF-16 code units per line) back to a byte offset.
pub fn position_to_byte(src: &str, pos: Position) -> usize {
    let mut current_line = 0u32;
    let mut line_start = 0usize;

    for (i, ch) in src.char_indices() {
        if current_line == pos.line { break; }
        if ch == '\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }
    if current_line != pos.line {
        return src.len();
    }

    let mut col_utf16 = 0u32;
    for (i, ch) in src[line_start..].char_indices() {
        if col_utf16 >= pos.character {
            return line_start + i;
        }
        col_utf16 += ch.len_utf16() as u32;
    }
    src.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_top_level_section() {
        let src = "[package]\nname = \"x\"\n[dependencies]\nfoo = \"1\"\n";
        let r = locate(src, "[dependencies]");
        assert_eq!(r.start.line, 2);
        assert_eq!(r.start.character, 0);
    }

    #[test]
    fn expands_dotted_context_to_key() {
        let src = "[dependencies]\nfoo = \"1\"\n";
        let r = locate(src, "[dependencies.foo]");
        // Should find the `foo =` line.
        assert_eq!(r.start.line, 1);
    }

    #[test]
    fn byte_to_position_roundtrip() {
        let src = "ab\ncd\nef";
        assert_eq!(byte_to_position(src, 0), Position { line: 0, character: 0 });
        assert_eq!(byte_to_position(src, 3), Position { line: 1, character: 0 });
        assert_eq!(byte_to_position(src, 6), Position { line: 2, character: 0 });
    }
}
