use super::common::{
    build_item, collect_c_block, collect_line_block, first_ident, item_has_content, next_non_blank,
};
use super::cpp::detect_c_symbol;
use super::{DocExtractor, DocItem, DocKind, DocLanguage};
use std::path::Path;

pub struct DExtractor;

impl DocExtractor for DExtractor {
    fn extensions(&self) -> &[&str] {
        &["d"]
    }
    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_d(src, path)
    }
}

pub(super) fn extract_d(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    // Pre-scan for the module declaration so we can qualify all names.
    let mod_name: String = lines
        .iter()
        .find_map(|l| {
            let rest = l.trim().strip_prefix("module ")?;
            let name = first_ident(rest);
            if !name.is_empty() {
                Some(name)
            } else {
                None
            }
        })
        .unwrap_or_default();

    while i < lines.len() {
        let t = lines[i].trim();

        if t.starts_with("/++") {
            let (block, end) = collect_d_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let qualified = qualify_mod(&mod_name, name, &kind);
            let item = build_item(
                block,
                qualified,
                kind,
                file,
                end + 2,
                DocLanguage::D,
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
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let qualified = qualify_mod(&mod_name, name, &kind);
            let item = build_item(
                block,
                qualified,
                kind,
                file,
                end + 2,
                DocLanguage::D,
                sym.to_string(),
            );
            if item_has_content(&item) {
                items.push(item);
            }
            i = end + 1;
            continue;
        }

        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let qualified = qualify_mod(&mod_name, name, &kind);
            let item = build_item(
                block,
                qualified,
                kind,
                file,
                end + 2,
                DocLanguage::D,
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

fn qualify_mod(mod_name: &str, name: String, kind: &super::DocKind) -> String {
    if mod_name.is_empty() || *kind == super::DocKind::Module || name.is_empty() {
        name
    } else {
        format!("{mod_name}.{name}")
    }
}

fn collect_d_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let first = lines[start].trim();
    let after = first[3..].trim();

    if let Some(content) = after.strip_suffix("+/") {
        out.push(content.trim().to_string());
        return (out, start);
    }
    if !after.is_empty() {
        out.push(after.to_string());
    }

    let mut i = start + 1;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.ends_with("+/") {
            let content = t
                .strip_suffix("+/")
                .unwrap_or("")
                .trim_start_matches('+')
                .trim();
            if !content.is_empty() {
                out.push(content.to_string());
            }
            return (out, i);
        }
        let content = t
            .strip_prefix("+ ")
            .or_else(|| t.strip_prefix('+'))
            .unwrap_or(t);
        out.push(content.to_string());
        i += 1;
    }
    (out, i.saturating_sub(1))
}

fn detect_d_symbol(line: &str) -> (String, DocKind) {
    let t = line.trim();
    let (name, kind) = detect_c_symbol(t);
    if kind != DocKind::Unknown {
        return (name, kind);
    }
    if let Some(r) = t.strip_prefix("interface ") {
        return (first_ident(r), DocKind::Interface);
    }
    if let Some(r) = t.strip_prefix("module ") {
        return (first_ident(r), DocKind::Module);
    }
    (name, kind)
}
