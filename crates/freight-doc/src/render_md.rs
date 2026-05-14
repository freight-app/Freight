use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::extract::{DocItem, DocKind, DocSet, DocTag, TagKind};

/// Write the documentation as a set of inter-linked Markdown files.
///
/// Produces `index.md` (file list), `symbols.md` (quick symbol navigation),
/// plus one `<slug>.md` per source file.
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

    // Indexes
    std::fs::write(out_dir.join("index.md"), render_index(&by_file))?;
    std::fs::write(out_dir.join("symbols.md"), render_symbol_index(&by_file))?;

    Ok(())
}

// ── Index page ────────────────────────────────────────────────────────────────

fn render_index(by_file: &BTreeMap<String, Vec<&DocItem>>) -> String {
    let mut md = String::new();
    let total: usize = by_file.values().map(|v| v.len()).sum();

    let _ = writeln!(md, "# Documentation\n");
    md.push_str(&nav_tabs("Files"));
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

fn render_symbol_index(by_file: &BTreeMap<String, Vec<&DocItem>>) -> String {
    let mut md = String::new();
    let total_named: usize = by_file
        .values()
        .flat_map(|items| items.iter())
        .filter(|item| !item.name.is_empty())
        .count();

    let _ = writeln!(md, "# Symbols\n");
    md.push_str(&nav_tabs("Symbols"));
    let _ = writeln!(md, "*{total_named} named documented items.*\n");

    let mut by_kind: BTreeMap<&'static str, Vec<(&str, &DocItem)>> = BTreeMap::new();
    for (rel, items) in by_file {
        for item in items.iter().filter(|item| !item.name.is_empty()) {
            by_kind.entry(kind_title(&item.kind)).or_default().push((rel.as_str(), item));
        }
    }

    if by_kind.is_empty() {
        md.push_str("No named symbols found.\n");
        return md;
    }

    for (kind, mut symbols) in by_kind {
        symbols.sort_by(|(a_rel, a), (b_rel, b)| {
            a.name
                .cmp(&b.name)
                .then_with(|| a_rel.cmp(b_rel))
                .then_with(|| a.line.cmp(&b.line))
        });

        let _ = writeln!(md, "## {kind}\n");
        md.push_str("| Symbol | File | Line | Summary |\n");
        md.push_str("|--------|------|-----:|---------|\n");
        for (rel, item) in symbols {
            let slug = rel_to_slug(rel);
            let anchor = symbol_anchor(item);
            let summary = md_table_escape(if item.brief.is_empty() { "—" } else { &item.brief });
            let _ = writeln!(
                md,
                "| [{label} `{name}`]({slug}.md#{anchor}) | [{rel}]({slug}.md) | {line} | {summary} |",
                label = item.kind.label(),
                name = md_table_escape(&item.name),
                line = item.line,
            );
        }
        let _ = writeln!(md);
    }

    md
}

fn nav_tabs(active: &str) -> String {
    let files = if active == "Files" {
        "**Files**".to_string()
    } else {
        "[Files](index.md)".to_string()
    };
    let symbols = if active == "Symbols" {
        "**Symbols**".to_string()
    } else {
        "[Symbols](symbols.md)".to_string()
    };

    format!("{files} | {symbols}\n\n")
}

// ── Per-file page ─────────────────────────────────────────────────────────────

fn render_file_page(rel: &str, slug: &str, items: &[&DocItem]) -> String {
    let _ = slug; // used by callers for the filename; not needed inside the page itself
    let mut md = String::new();

    let _ = writeln!(md, "# `{rel}`\n");

    if let Some(lang) = items.first().map(|i| i.lang.label()) {
        let _ = writeln!(md, "**Language:** {lang}\n");
    }

    let _ = writeln!(md, "[← Index](index.md) | [All symbols](symbols.md)\n");
    let _ = writeln!(md, "---\n");

    // Table of contents for files with multiple named items
    let named: Vec<&&DocItem> = items.iter().filter(|i| !i.name.is_empty()).collect();
    if named.len() > 3 {
        let _ = writeln!(md, "## Contents\n");
        for item in &named {
            let anchor = symbol_anchor(item);
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
        let anchor = symbol_anchor(item);
        render_item(&mut md, item, &mut name_count, &anchor);
    }

    md
}

fn render_item(
    md: &mut String,
    item: &DocItem,
    name_count: &mut BTreeMap<String, usize>,
    anchor: &str,
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

    let _ = writeln!(md, "<a id=\"{anchor}\"></a>");
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

    let params: Vec<&DocTag> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    if !params.is_empty() {
        render_parameter_table(md, &params);
    }

    // Non-param tags
    for tag in &item.tags {
        if matches!(tag.kind, TagKind::Param | TagKind::Brief) { continue; }
        let _ = writeln!(md, "**{}:** {}\n", tag.kind.label(), tag.text);
    }

    let _ = writeln!(md, "---\n");
}

fn render_parameter_table(md: &mut String, params: &[&DocTag]) {
    md.push_str("<table style=\"border-collapse: collapse; margin: 0.75rem 0 1rem; font-size: 0.92em;\">\n");
    md.push_str("  <thead>\n");
    md.push_str("    <tr style=\"background: #eaf4ff; color: #0b3d68;\">\n");
    md.push_str("      <th style=\"border: 1px solid #b6d7f2; padding: 0.25rem 0.5rem; text-align: left;\">Parameter</th>\n");
    md.push_str("      <th style=\"border: 1px solid #b6d7f2; padding: 0.25rem 0.5rem; text-align: left;\">Description</th>\n");
    md.push_str("    </tr>\n");
    md.push_str("  </thead>\n");
    md.push_str("  <tbody>\n");
    for (idx, tag) in params.iter().enumerate() {
        let pname = html_escape(tag.name.as_deref().unwrap_or("—"));
        let description = html_escape(&tag.text);
        let row_color = if idx % 2 == 0 { "#ffffff" } else { "#f7fbff" };
        let _ = writeln!(
            md,
            "    <tr style=\"background: {row_color};\"><td style=\"border: 1px solid #d0e3f4; padding: 0.2rem 0.5rem; white-space: nowrap;\"><code>{pname}</code></td><td style=\"border: 1px solid #d0e3f4; padding: 0.2rem 0.5rem;\">{description}</td></tr>"
        );
    }
    md.push_str("  </tbody>\n");
    md.push_str("</table>\n\n");
}

fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn symbol_anchor(item: &DocItem) -> String {
    let name = if item.name.is_empty() { "anonymous" } else { &item.name };
    format!(
        "{}-{}-line-{}",
        item.kind.label(),
        md_heading_anchor("", name),
        item.line
    )
}

fn kind_title(kind: &DocKind) -> &'static str {
    match kind {
        DocKind::Function => "Functions",
        DocKind::Struct => "Structs",
        DocKind::Class => "Classes",
        DocKind::Enum => "Enums",
        DocKind::Typedef => "Types",
        DocKind::Variable => "Variables",
        DocKind::Macro => "Macros",
        DocKind::Module => "Modules",
        DocKind::Subroutine => "Subroutines",
        DocKind::Interface => "Interfaces",
        DocKind::Unknown => "Other items",
    }
}

fn md_table_escape(text: &str) -> String {
    text.replace('|', r"\|").replace('\n', " ")
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
    fn parameter_table_is_compact_and_colored() {
        let tags = [DocTag {
            kind: TagKind::Param,
            name: Some("value".into()),
            text: "Input <value> & scale.".into(),
        }];
        let params: Vec<&DocTag> = tags.iter().collect();
        let mut md = String::new();

        render_parameter_table(&mut md, &params);

        assert!(md.contains("<table style=\"border-collapse: collapse"), "{md}");
        assert!(md.contains("background: #eaf4ff"), "{md}");
        assert!(md.contains("<code>value</code>"), "{md}");
        assert!(md.contains("Input &lt;value&gt; &amp; scale."), "{md}");
    }

    #[test]
    fn symbol_index_lists_named_items() {
        let item = DocItem {
            name: "add".into(),
            kind: DocKind::Function,
            brief: "Add two values.".into(),
            body: String::new(),
            tags: Vec::new(),
            file: Path::new("src/math.c").to_path_buf(),
            line: 7,
            lang: super::super::extract::DocLanguage::C,
            signature: "int add(int a, int b);".into(),
        };
        let mut by_file = BTreeMap::new();
        by_file.insert("src/math.c".to_string(), vec![&item]);

        let md = render_symbol_index(&by_file);

        assert!(md.contains("**Symbols**"), "{md}");
        assert!(md.contains("## Functions"), "{md}");
        assert!(md.contains("[fn `add`](src_math_c.md#fn-add-line-7)"), "{md}");
    }

}
