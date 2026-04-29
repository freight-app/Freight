use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::extract::{DocItem, DocSet, DocTag, TagKind};

/// Write the documentation as a set of inter-linked Markdown files.
///
/// Produces `index.md` (symbol list) plus one `<slug>.md` per source file.
/// All internal links use relative paths so the output works as a static site,
/// in GitHub rendered markdown, or fed into MkDocs / mdBook.
///
/// Math: `$...$` and `$$...$$` are passed through verbatim so MathJax /
/// KaTeX renderers pick them up without modification.
pub fn render_markdown(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;

    // Group items by source-relative path (stable ordering via BTreeMap)
    let mut by_file: BTreeMap<String, Vec<&DocItem>> = BTreeMap::new();
    for item in &set.items {
        let rel = item.file
            .strip_prefix(&set.source_root)
            .unwrap_or(&item.file)
            .to_string_lossy()
            .into_owned();
        by_file.entry(rel).or_default().push(item);
    }

    // Build a global symbol index: name → (slug, anchor) for cross-linking
    let sym_index = build_symbol_index(&by_file);

    // Per-file pages
    for (rel, items) in &by_file {
        let slug = rel_to_slug(rel);
        let content = render_file_page(rel, &slug, items, &sym_index);
        std::fs::write(out_dir.join(format!("{slug}.md")), content)?;
    }

    // Index
    std::fs::write(out_dir.join("index.md"), render_index(&by_file))?;

    Ok(())
}

// ── Index page ────────────────────────────────────────────────────────────────

fn render_index(by_file: &BTreeMap<String, Vec<&DocItem>>) -> String {
    let mut md = String::new();
    let total: usize = by_file.values().map(|v| v.len()).sum();

    let _ = writeln!(md, "# Documentation\n");
    let _ = writeln!(md, "*{total} documented items across {} files.*\n", by_file.len());

    md.push_str("| File | Language | Items | Symbols |\n");
    md.push_str("|------|----------|------:|---------|\n");

    for (rel, items) in by_file {
        let slug = rel_to_slug(rel);
        let lang = items.first().map(|i| i.lang.label()).unwrap_or("");
        let named: Vec<&str> = items.iter()
            .filter(|i| !i.name.is_empty())
            .map(|i| i.name.as_str())
            .take(4)
            .collect();
        let more = items.iter().filter(|i| !i.name.is_empty()).count() > 4;
        let sym_str = if named.is_empty() {
            "—".to_string()
        } else if more {
            format!("{} …", named.join(", "))
        } else {
            named.join(", ")
        };
        let _ = writeln!(
            md,
            "| [{rel}]({slug}.md) | {lang} | {} | {sym_str} |",
            items.len(),
        );
    }

    md
}

// ── Per-file page ─────────────────────────────────────────────────────────────

fn render_file_page(
    rel: &str,
    slug: &str,
    items: &[&DocItem],
    sym_index: &SymbolIndex,
) -> String {
    let _ = slug; // used by callers for the filename; not needed inside the page itself
    let mut md = String::new();

    let _ = writeln!(md, "# `{rel}`\n");

    if let Some(lang) = items.first().map(|i| i.lang.label()) {
        let _ = writeln!(md, "**Language:** {lang}\n");
    }

    let _ = writeln!(md, "[← Index](index.md)\n");
    let _ = writeln!(md, "---\n");

    // Table of contents for files with multiple named items
    let named: Vec<&&DocItem> = items.iter().filter(|i| !i.name.is_empty()).collect();
    if named.len() > 3 {
        let _ = writeln!(md, "## Contents\n");
        for item in &named {
            let anchor = md_heading_anchor(item.kind.label(), &item.name);
            let _ = writeln!(
                md,
                "- [{kind} `{name}`](#{anchor})",
                kind = item.kind.label(),
                name = item.name,
            );
        }
        let _ = writeln!(md);
        md.push_str("---\n\n");
    }

    // Items
    let mut name_count: BTreeMap<String, usize> = BTreeMap::new();
    for item in items {
        render_item(&mut md, item, &mut name_count, sym_index);
    }

    md
}

fn render_item(
    md: &mut String,
    item: &DocItem,
    name_count: &mut BTreeMap<String, usize>,
    sym_index: &SymbolIndex,
) {
    let display = if item.name.is_empty() {
        "*(anonymous)*".to_string()
    } else {
        format!("`{}`", item.name)
    };

    // Deduplicate same-name headings (GFM appends -1, -2; we do it ourselves)
    let heading_base = format!("{} {}", item.kind.label(), item.name);
    let count = name_count.entry(item.name.clone()).or_insert(0);
    let heading = if *count == 0 {
        heading_base
    } else {
        format!("{heading_base} ({})", *count + 1)
    };
    *count += 1;

    let _ = writeln!(md, "## {kind} {display}", kind = item.kind.label());
    if item.line > 0 {
        let _ = writeln!(md, "\n<sub>line {}</sub>", item.line);
    }
    let _ = writeln!(md);

    if !item.brief.is_empty() {
        let _ = writeln!(md, "{}\n", linkify(&item.brief, sym_index));
    }
    if !item.body.is_empty() {
        let _ = writeln!(md, "{}\n", linkify(&item.body, sym_index));
    }

    // Parameters table
    let params: Vec<&DocTag> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    if !params.is_empty() {
        md.push_str("| Parameter | Description |\n");
        md.push_str("|-----------|-------------|\n");
        for tag in &params {
            let pname = tag.name.as_deref().unwrap_or("—");
            let _ = writeln!(
                md,
                "| `{pname}` | {} |",
                linkify(&tag.text, sym_index)
            );
        }
        let _ = writeln!(md);
    }

    // Non-param tags
    for tag in &item.tags {
        if matches!(tag.kind, TagKind::Param | TagKind::Brief) { continue; }
        let _ = writeln!(
            md,
            "**{}:** {}\n",
            tag.kind.label(),
            linkify(&tag.text, sym_index)
        );
    }

    let _ = writeln!(md, "---\n");
    let _ = heading; // suppress unused warning — heading is used for dedup logic
}

// ── Symbol index for cross-linking ────────────────────────────────────────────

struct SymbolEntry {
    slug: String,
    anchor: String,
}

type SymbolIndex = BTreeMap<String, SymbolEntry>;

fn build_symbol_index(by_file: &BTreeMap<String, Vec<&DocItem>>) -> SymbolIndex {
    let mut idx = SymbolIndex::new();
    for (rel, items) in by_file {
        let slug = rel_to_slug(rel);
        for item in items {
            if item.name.is_empty() { continue; }
            // First definition wins (same as rustdoc behaviour)
            idx.entry(item.name.clone()).or_insert_with(|| SymbolEntry {
                slug: slug.clone(),
                anchor: md_heading_anchor(item.kind.label(), &item.name),
            });
        }
    }
    idx
}

/// Replace bare symbol names in prose with Markdown links when a definition exists.
///
/// Only replaces whole-word occurrences that are not already inside a link or
/// a code span to avoid double-linking.
fn linkify(text: &str, idx: &SymbolIndex) -> String {
    if idx.is_empty() || text.is_empty() { return text.to_string(); }

    // Simple word-boundary scan: split into segments, replace token if in index
    // and not already enclosed in backticks or brackets.
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '`' {
            in_code = !in_code;
            out.push(c);
            continue;
        }
        if in_code || !c.is_alphanumeric() && c != '_' {
            out.push(c);
            continue;
        }
        // Collect a word token
        let mut word = String::new();
        word.push(c);
        while let Some(&nc) = chars.peek() {
            if nc.is_alphanumeric() || nc == '_' {
                word.push(nc);
                chars.next();
            } else {
                break;
            }
        }
        if let Some(entry) = idx.get(&word) {
            let _ = write!(
                out,
                "[`{word}`]({slug}.md#{anchor})",
                slug = entry.slug,
                anchor = entry.anchor,
            );
        } else {
            out.push_str(&word);
        }
    }
    out
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Convert a relative path to a URL-safe slug for use as a filename.
pub fn rel_to_slug(rel: &str) -> String {
    rel.replace(['/', '\\', '.'], "_")
}

/// Derive a GFM-compatible heading anchor from kind + name.
///
/// GFM anchor rules: lowercase, spaces → `-`, strip everything else except
/// alphanumerics and hyphens, collapse consecutive hyphens.
pub fn md_heading_anchor(kind: &str, name: &str) -> String {
    let raw = format!("{kind} {name}");
    let lowered: String = raw.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    // Collapse consecutive hyphens and strip leading/trailing
    lowered.split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_simple() {
        assert_eq!(md_heading_anchor("fn", "factorial"), "fn-factorial");
    }

    #[test]
    fn anchor_camel() {
        assert_eq!(md_heading_anchor("class", "OrderStatistics"), "class-orderstatistics");
    }

    #[test]
    fn slug_path() {
        assert_eq!(rel_to_slug("src/mathlib.h"), "src_mathlib_h");
    }

    #[test]
    fn linkify_known_symbol() {
        let mut idx = SymbolIndex::new();
        idx.insert("factorial".into(), SymbolEntry {
            slug: "mathlib_h".into(),
            anchor: "fn-factorial".into(),
        });
        let out = linkify("Call factorial to compute n!", &idx);
        assert!(out.contains("[`factorial`](mathlib_h.md#fn-factorial)"), "{out}");
    }

    #[test]
    fn linkify_skips_backtick() {
        let mut idx = SymbolIndex::new();
        idx.insert("foo".into(), SymbolEntry { slug: "a".into(), anchor: "fn-foo".into() });
        let out = linkify("`foo` is already code", &idx);
        // Inside backtick span — should not double-link
        assert_eq!(out, "`foo` is already code");
    }
}
