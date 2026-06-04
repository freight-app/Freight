use super::common::{
    build_item, collect_c_block, collect_line_block, first_ident, item_has_content,
};

/// Like `next_non_blank` but also skips `#[...]` attribute lines so that
/// `#[derive(...)]` / `#[cfg(...)]` between a doc comment and a `pub struct`
/// are transparent to symbol detection.
fn next_decl_line<'a>(lines: &[&'a str], from: usize) -> &'a str {
    let mut i = from;
    loop {
        let Some(&l) = lines.get(i) else { return "" };
        let t = l.trim();
        if t.is_empty() || t.starts_with("#[") || t.starts_with("#![") {
            i += 1;
            continue;
        }
        return t;
    }
}
use super::{DocExtractor, DocItem, DocKind, DocLanguage};
use std::path::Path;

pub struct RustExtractor;

impl DocExtractor for RustExtractor {
    fn extensions(&self) -> &[&str] {
        &["rs"]
    }
    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_rust(src, path)
    }
}

pub(super) fn extract_rust(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    // Track inline `mod name { }` scope for name qualification.
    let mut brace_depth: usize = 0;
    let mut mod_stack: Vec<(usize, String)> = Vec::new();
    let mut pending_mod: Option<String> = None;

    while i < lines.len() {
        let t = lines[i].trim();

        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_decl_line(&lines, end + 1);
            let (name, kind) = detect_rust_symbol(sym);
            let ns = mod_stack.last().map(|(_, p)| p.as_str()).unwrap_or("");
            let item = build_item(
                normalize_sections(block),
                qualify_name(&name, ns),
                kind,
                file,
                i + 1,
                DocLanguage::Rust,
                sym.to_string(),
            );
            if item_has_content(&item) {
                items.push(item);
            }
            i = end + 1;
            continue;
        }

        if t.starts_with("/**") && !t.starts_with("/***/") {
            let (block, end) = collect_c_block(&lines, i);
            let sym = next_decl_line(&lines, end + 1);
            let (name, kind) = detect_rust_symbol(sym);
            let ns = mod_stack.last().map(|(_, p)| p.as_str()).unwrap_or("");
            let item = build_item(
                normalize_sections(block),
                qualify_name(&name, ns),
                kind,
                file,
                i + 1,
                DocLanguage::Rust,
                sym.to_string(),
            );
            if item_has_content(&item) {
                items.push(item);
            }
            i = end + 1;
            continue;
        }

        // Track inline mod scopes — mirrors cpp.rs namespace tracking.
        if !t.starts_with("//") && !t.starts_with("/*") && !t.starts_with('*') {
            // Detect `(pub(...))? mod name` with an opening brace (not `mod name;`).
            if let Some(mod_rest) = extract_mod_rest(t) {
                let name = first_ident(mod_rest);
                if !name.is_empty() && !t.trim_end().ends_with(';') {
                    let path = match mod_stack.last() {
                        Some((_, p)) => format!("{p}::{name}"),
                        None => name,
                    };
                    if t.contains('{') {
                        let opens = t.chars().filter(|&c| c == '{').count();
                        let closes = t.chars().filter(|&c| c == '}').count();
                        brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                        mod_stack.push((brace_depth, path));
                    } else {
                        pending_mod = Some(path);
                    }
                    i += 1;
                    continue;
                }
            }

            if pending_mod.is_some() && t.contains('{') {
                let path = pending_mod.take().unwrap();
                let opens = t.chars().filter(|&c| c == '{').count();
                let closes = t.chars().filter(|&c| c == '}').count();
                brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                mod_stack.push((brace_depth, path));
                while mod_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                    mod_stack.pop();
                }
                i += 1;
                continue;
            }

            let opens = t.chars().filter(|&c| c == '{').count();
            let closes = t.chars().filter(|&c| c == '}').count();
            brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
            while mod_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                mod_stack.pop();
            }
        }

        i += 1;
    }
    items
}

/// Strip visibility and keyword to reach the `mod <name>` part of a line.
/// Returns the slice starting at the identifier after `mod `.
fn extract_mod_rest(t: &str) -> Option<&str> {
    // Strip optional visibility: `pub`, `pub(crate)`, `pub(super)`, `pub(in ...)`, etc.
    let after_vis = if let Some(r) = t.strip_prefix("pub") {
        let r = r.trim_start();
        if r.starts_with('(') {
            // pub(xxx) — skip to closing paren
            let close = r.find(')')? + 1;
            r[close..].trim_start()
        } else {
            r
        }
    } else {
        t
    };
    after_vis.strip_prefix("mod ")
}

fn qualify_name(name: &str, ns: &str) -> String {
    if name.is_empty() || ns.is_empty() {
        name.to_string()
    } else {
        format!("{ns}::{name}")
    }
}

/// Convert Rust Markdown section headings into `@tag` equivalents so that
/// `build_item` treats them as structured tags rather than body prose.
///
/// Any heading level (`#`, `##`, `###`) is accepted.  Unknown headings are
/// left unchanged and land in the body.
fn normalize_sections(block: Vec<String>) -> Vec<String> {
    block
        .into_iter()
        .map(|line| {
            let t = line.trim_start();
            if !t.starts_with('#') {
                return line;
            }
            let heading = t.trim_start_matches('#').trim().to_ascii_lowercase();
            let tag = match heading.as_str() {
                "examples" | "example" => "@example",
                "panics" | "panic" => "@panics",
                "errors" | "error" => "@errors",
                "safety" => "@safety",
                "returns" | "return value" => "@returns",
                "note" | "notes" => "@note",
                "see also" | "see" => "@see",
                "deprecated" => "@deprecated",
                "since" => "@since",
                "warning" | "warnings" => "@warning",
                "arguments" | "parameters" => "@arguments",
                _ => return line,
            };
            tag.to_string()
        })
        .collect()
}

fn detect_rust_symbol(line: &str) -> (String, DocKind) {
    let words: Vec<&str> = line.split_whitespace().collect();
    let skip = words
        .iter()
        .take_while(|&&w| {
            matches!(w, "pub" | "async" | "unsafe" | "extern" | "default")
                || w.starts_with("pub(")
                || w.starts_with('"')
        })
        .count();

    let rest = &words[skip..];
    if rest.is_empty() {
        return (String::new(), DocKind::Unknown);
    }

    let keyword = rest[0];
    let name_raw = rest
        .get(1)
        .copied()
        .unwrap_or("")
        .split(['<', '(', '{', ':'])
        .next()
        .unwrap_or("");
    let name = name_raw
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
        .to_string();

    match keyword {
        "fn" => (name, DocKind::Function),
        "struct" => (name, DocKind::Struct),
        "enum" => (name, DocKind::Enum),
        "trait" => (name, DocKind::Interface),
        "type" => (name, DocKind::Typedef),
        "mod" => (name, DocKind::Module),
        "const" => (name, DocKind::Variable),
        "static" => (name, DocKind::Variable),
        "impl" => {
            if let Some(pos) = rest.iter().position(|w| *w == "for") {
                let after = rest.get(pos + 1).copied().unwrap_or("");
                let n = after.split('<').next().unwrap_or("").to_string();
                return (n, DocKind::Struct);
            }
            (name, DocKind::Struct)
        }
        _ => (String::new(), DocKind::Unknown),
    }
}
