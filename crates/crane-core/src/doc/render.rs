use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::extract::{DocItem, DocSet, DocTag, TagKind};
use super::markdown::to_html as md_html;

/// Write the full documentation site to `out_dir`.
///
/// Produces `index.html` (symbol list) plus one page per source file.
/// All output is self-contained — no external CSS or JS dependencies.
pub fn render_html(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;

    // Group items by path relative to source_root
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
        let page = rel_to_page(rel);
        std::fs::write(out_dir.join(format!("{page}.html")), render_file_page(rel, items))?;
    }

    // Index
    std::fs::write(out_dir.join("index.html"), render_index(&by_file))?;

    Ok(())
}

fn rel_to_page(rel: &str) -> String {
    rel.replace(['/', '\\', '.'], "_")
}

// ── Index page ────────────────────────────────────────────────────────────────

fn render_index(by_file: &BTreeMap<String, Vec<&DocItem>>) -> String {
    let mut h = page_head("Documentation");
    let total: usize = by_file.values().map(|v| v.len()).sum();
    let _ = write!(h, "<h1>Documentation</h1>");
    let _ = write!(
        h,
        r#"<p class="summary">{total} documented items across {} files</p>"#,
        by_file.len()
    );

    h.push_str(
        r#"<table><thead><tr>
            <th>File</th><th>Language</th><th>Items</th><th>Symbols</th>
            </tr></thead><tbody>"#,
    );

    for (rel, items) in by_file {
        let page = rel_to_page(rel);
        let lang = items.first().map(|i| i.lang.label()).unwrap_or("");
        // Up to 4 named symbols as a preview
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
        let _ = write!(
            h,
            r#"<tr>
                <td><a href="{page}.html">{rel}</a></td>
                <td><span class="badge">{lang}</span></td>
                <td>{}</td>
                <td class="sym">{sym_str}</td>
               </tr>"#,
            items.len(),
            rel = esc(rel),
        );
    }

    h.push_str("</tbody></table></main></body></html>");
    h
}

// ── Per-file page ─────────────────────────────────────────────────────────────

fn render_file_page(rel: &str, items: &[&DocItem]) -> String {
    let mut h = page_head(rel);
    let _ = write!(h, r#"<p><a href="index.html">← Index</a></p>"#);
    let _ = write!(h, "<h1><code>{}</code></h1>", esc(rel));

    if let Some(lang) = items.first().map(|i| i.lang.label()) {
        let _ = write!(h, r#"<span class="badge">{lang}</span>"#);
    }

    // Table of contents for files with many named items
    let named: Vec<&&DocItem> = items.iter().filter(|i| !i.name.is_empty()).collect();
    if named.len() > 3 {
        h.push_str(r#"<nav class="toc"><h2>Contents</h2><ul>"#);
        for item in &named {
            let anchor = make_anchor(&item.name, item.line);
            let _ = write!(
                h,
                r##"<li><a href="#{anchor}"><span class="toc-kind">{}</span> {}</a></li>"##,
                item.kind.label(),
                esc(&item.name)
            );
        }
        h.push_str("</ul></nav>");
    }

    for item in items {
        render_item(&mut h, item);
    }

    h.push_str("</main></body></html>");
    h
}

fn render_item(h: &mut String, item: &DocItem) {
    let anchor = make_anchor(&item.name, item.line);
    let display_name = if item.name.is_empty() { "(anonymous)".to_string() } else { item.name.clone() };

    let _ = write!(h, r#"<div class="item" id="{anchor}"><div class="item-header">"#);
    let _ = write!(h, r#"<span class="kind">{}</span>"#, item.kind.label());
    let _ = write!(h, r#" <code class="name">{}</code>"#, esc(&display_name));
    if item.line > 0 {
        let _ = write!(h, r#" <span class="loc">line {}</span>"#, item.line);
    }
    h.push_str("</div>");

    if !item.brief.is_empty() {
        let _ = write!(h, r#"<div class="brief">{}</div>"#, md_html(&item.brief));
    }
    if !item.body.is_empty() {
        let _ = write!(h, r#"<div class="body">{}</div>"#, md_html(&item.body));
    }

    // Parameters table
    let params: Vec<&DocTag> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    if !params.is_empty() {
        h.push_str(
            r#"<table class="params"><thead>
               <tr><th>Parameter</th><th>Description</th></tr>
               </thead><tbody>"#,
        );
        for tag in &params {
            let pname = tag.name.as_deref().unwrap_or("—");
            let _ = write!(
                h,
                "<tr><td><code>{}</code></td><td>{}</td></tr>",
                esc(pname),
                md_html(&tag.text)
            );
        }
        h.push_str("</tbody></table>");
    }

    // Non-param, non-brief tags
    for tag in &item.tags {
        if matches!(tag.kind, TagKind::Param | TagKind::Brief) { continue; }
        let _ = write!(
            h,
            r#"<div class="tag"><span class="tag-label">{}:</span> {}</div>"#,
            esc(tag.kind.label()),
            md_html(&tag.text)
        );
    }

    h.push_str("</div>");
}

fn make_anchor(name: &str, line: usize) -> String {
    if name.is_empty() {
        format!("L{line}")
    } else {
        format!(
            "{}-{line}",
            name.replace(|c: char| !c.is_alphanumeric(), "_")
        )
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}

fn page_head(title: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{t} — crane doc</title>
<style>
*{{box-sizing:border-box;margin:0;padding:0}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;
      background:#0f1117;color:#d4d4d8;line-height:1.65;font-size:15px}}
main{{max-width:980px;margin:0 auto;padding:2rem 1.25rem}}
h1{{font-size:1.4rem;color:#f4f4f5;margin-bottom:.75rem}}
h2{{font-size:.9rem;color:#71717a;margin:.75rem 0 .4rem;
    text-transform:uppercase;letter-spacing:.07em}}
a{{color:#60a5fa;text-decoration:none}}
a:hover{{text-decoration:underline}}
code{{font-family:'JetBrains Mono','Fira Code','Cascadia Code',monospace;
      background:#1e2433;padding:.1em .35em;border-radius:3px;font-size:.875em}}
p{{margin:.4rem 0}}
.summary{{color:#71717a;margin-bottom:1rem}}
table{{width:100%;border-collapse:collapse;margin:.75rem 0}}
th{{text-align:left;padding:.4rem .75rem;border-bottom:1px solid #27272a;
    color:#52525b;font-size:.78rem;text-transform:uppercase;letter-spacing:.07em}}
td{{padding:.4rem .75rem;border-bottom:1px solid #18181b;vertical-align:top;font-size:.9rem}}
.sym{{color:#52525b;font-size:.85rem}}
.badge{{font-size:.72rem;background:#1c2d4a;color:#60a5fa;
        padding:.1em .45em;border-radius:3px;display:inline-block;margin:.2rem 0}}
nav.toc{{background:#161b27;border:1px solid #27272a;border-radius:6px;
          padding:.75rem 1rem;margin:1rem 0}}
nav.toc ul{{list-style:none;column-count:2;column-gap:2rem}}
nav.toc li{{margin:.2rem 0;font-size:.875rem}}
.toc-kind{{color:#3f3f46;display:inline-block;width:3.5rem;font-size:.75rem}}
.item{{border:1px solid #27272a;border-radius:6px;padding:1rem 1.25rem;
       margin:1rem 0;background:#161b27}}
.item-header{{display:flex;align-items:baseline;gap:.5rem;flex-wrap:wrap;margin-bottom:.5rem}}
.kind{{font-size:.7rem;background:#27272a;color:#a1a1aa;padding:.1em .45em;
       border-radius:3px;text-transform:uppercase;letter-spacing:.07em;flex-shrink:0}}
.name{{font-size:1.05rem;color:#93c5fd;font-weight:500}}
.loc{{font-size:.72rem;color:#3f3f46;margin-left:auto}}
.brief{{color:#e4e4e7;margin:.4rem 0}}
.body{{color:#a1a1aa;margin:.4rem 0;font-size:.9rem}}
.params{{margin:.6rem 0}}
.params td:first-child{{width:20%;color:#7dd3fc;font-size:.85rem}}
.tag{{margin:.3rem 0;font-size:.875rem}}
.tag-label{{color:#52525b}}
.math{{overflow-x:auto}}
</style>
<script>
MathJax = {{
  tex: {{ inlineMath: [['$','$'],['\\\\(','\\\\)']], displayMath: [['$$','$$'],['\\\\[','\\\\]']] }},
  options: {{ skipHtmlTags: ['script','noscript','style','textarea','pre'] }}
}};
</script>
<script async src="https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-chtml.js"></script>
</head>
<body><main>
"#,
        t = esc(title)
    )
}
