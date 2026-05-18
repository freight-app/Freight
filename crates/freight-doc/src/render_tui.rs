/// Serialise a slice of `DocItem`s to a single Markdown string for TUI rendering.
///
/// The output is designed to be consumed by `markdown_to_blocks` in the CLI crate.
/// Each item becomes an H2 section (`## kind name`) so the TUI renderer can extract
/// symbol entries by walking headings.  Math is normalised to `$...$` / `$$...$$` so
/// `pulldown-cmark` with `ENABLE_MATH` handles it without extra pre-processing.
use crate::extract::{DocItem, TagKind};

pub fn items_to_markdown(items: &[DocItem]) -> String {
    let mut out = String::new();

    for item in items {
        let sig = clean_sig(&item.signature);

        // H2 heading — use the declaration itself; fall back to "kind name".
        out.push_str("## ");
        if !sig.is_empty() {
            out.push_str(sig);
        } else {
            out.push_str(item.kind.label());
            out.push(' ');
            out.push_str(&item.name);
        }
        out.push_str("\n\n");

        // Brief paragraph.
        if !item.brief.is_empty() {
            out.push_str(&normalize_math(&item.brief));
            out.push_str("\n\n");
        }

        // Body.
        if !item.body.is_empty() {
            out.push_str(&normalize_math(&item.body));
            out.push_str("\n\n");
        }

        // Parameters + Returns in one table; a separator row divides them.
        let params:  Vec<_> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
        let returns: Vec<_> = item.tags.iter().filter(|t| t.kind == TagKind::Return).collect();
        if !params.is_empty() || !returns.is_empty() {
            out.push_str("| Parameter | Description |\n");
            out.push_str("|-----------|-------------|\n");
            for p in &params {
                let name = p.name.as_deref().unwrap_or("?");
                let desc = normalize_math(&p.text).replace('|', "\\|");
                out.push_str("| `");
                out.push_str(name);
                out.push_str("` | ");
                out.push_str(&desc);
                out.push_str(" |\n");
            }
            if !params.is_empty() && !returns.is_empty() {
                out.push_str("| ─── | ─── |\n");
            }
            for r in &returns {
                let desc = normalize_math(&r.text).replace('|', "\\|");
                out.push_str("| **Returns** | ");
                out.push_str(&desc);
                out.push_str(" |\n");
            }
            out.push('\n');
        }

        // All other tags (Note, Warning, Deprecated, Example, See, Since, …).
        for tag in &item.tags {
            match &tag.kind {
                TagKind::Param | TagKind::Return | TagKind::Brief => {}
                TagKind::See => {
                    out.push_str("### See also\n\n");
                    // Emit each comma- or space-delimited name as a cross-reference link.
                    for name in tag.text.split(|c: char| c == ',' || c == '\n') {
                        let name = name.trim().trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != ':');
                        if name.is_empty() { continue; }
                        out.push_str("- [");
                        out.push_str(name);
                        out.push_str("](#");
                        // Anchor is the leaf name (after last ::)
                        let anchor = name.rfind("::").map_or(name, |p| &name[p + 2..]);
                        out.push_str(anchor);
                        out.push_str(")\n");
                    }
                    out.push('\n');
                }
                kind => {
                    out.push_str("### ");
                    out.push_str(kind.label());
                    out.push_str("\n\n");
                    out.push_str(&normalize_math(&tag.text));
                    out.push_str("\n\n");
                }
            }
        }

        out.push_str("---\n\n");
    }

    out
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn clean_sig(sig: &str) -> &str {
    sig.trim_end()
       .trim_end_matches(|c: char| c == '{' || c == ';')
       .trim_end()
}

/// Convert `\[...\]` display math and `\(...\)` inline math to the `$$...$$` /
/// `$...$` forms that pulldown-cmark's `ENABLE_MATH` understands.
fn normalize_math(text: &str) -> String {
    if !text.contains("\\[") && !text.contains("\\(") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut rest = text;
    loop {
        let pd = rest.find("\\[");
        let pi = rest.find("\\(");
        let (open, close, md_o, md_c, pos) = match (pd, pi) {
            (None, None) => { out.push_str(rest); break; }
            (Some(a), None)             => ("\\[", "\\]", "$$", "$$", a),
            (None, Some(b))             => ("\\(", "\\)", "$",  "$",  b),
            (Some(a), Some(b)) if a <= b => ("\\[", "\\]", "$$", "$$", a),
            (_, Some(b))                => ("\\(", "\\)", "$",  "$",  b),
        };
        out.push_str(&rest[..pos]);
        rest = &rest[pos + open.len()..];
        if let Some(end) = rest.find(close) {
            out.push_str(md_o);
            out.push_str(&rest[..end]);
            out.push_str(md_c);
            rest = &rest[end + close.len()..];
        } else {
            // Unclosed delimiter — emit raw.
            out.push_str(open);
            out.push_str(rest);
            break;
        }
    }
    out
}
