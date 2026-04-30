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

    // Per-file pages
    for (rel, items) in &by_file {
        let slug = rel_to_slug(rel);
        let content = render_file_page(rel, &slug, items);
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

fn render_file_page(rel: &str, slug: &str, items: &[&DocItem]) -> String {
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
        render_item(&mut md, item, &mut name_count);
    }

    md
}

fn render_item(
    md: &mut String,
    item: &DocItem,
    name_count: &mut BTreeMap<String, usize>,
) {
    let display = if item.name.is_empty() {
        "*(anonymous)*".to_string()
    } else {
        format!("`{}`", item.name)
    };

    // Deduplicate same-name headings (GFM appends -1, -2; we do it ourselves)
    let heading_base = format!("{} {display}", item.kind.label());
    let count = name_count.entry(item.name.clone()).or_insert(0);
    let heading = if *count == 0 {
        heading_base
    } else {
        format!("{heading_base} ({})", *count + 1)
    };
    *count += 1;

    let _ = writeln!(md, "## {heading}");
    if item.line > 0 {
        let _ = writeln!(md, "\n<sub>line {}</sub>", item.line);
    }
    let _ = writeln!(md);

    if !item.signature.is_empty() {
        let sig = item.signature.trim_end_matches('{').trim();
        let _ = writeln!(md, "```\n{sig}\n```\n");
    }

    if !item.brief.is_empty() {
        let _ = writeln!(md, "{}\n", item.brief);
    }
    if !item.body.is_empty() {
        let _ = writeln!(md, "{}\n", item.body);
    }

    // Parameters table — pipe-escaped so the table cell stays valid
    let params: Vec<&DocTag> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    if !params.is_empty() {
        md.push_str("| Parameter | Description |\n");
        md.push_str("|-----------|-------------|\n");
        for tag in &params {
            let pname = tag.name.as_deref().unwrap_or("—");
            let _ = writeln!(
                md,
                "| `{pname}` | {} |",
                tag.text.replace('|', r"\|")
            );
        }
        let _ = writeln!(md);
    }

    // Non-param tags
    for tag in &item.tags {
        if matches!(tag.kind, TagKind::Param | TagKind::Brief) { continue; }
        let _ = writeln!(md, "**{}:** {}\n", tag.kind.label(), tag.text);
    }

    let _ = writeln!(md, "---\n");
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

}
