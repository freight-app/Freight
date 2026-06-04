use super::common::{build_item, ci_ident_after, item_has_content, next_non_blank};
use super::{DocExtractor, DocItem, DocKind, DocLanguage};
use std::path::Path;

pub struct AdaExtractor;

impl DocExtractor for AdaExtractor {
    fn extensions(&self) -> &[&str] {
        &["ads", "adb"]
    }
    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_ada(src, path)
    }
}

pub(super) fn extract_ada(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    // Pre-scan for the outermost package name so we can qualify all names.
    let pkg_name: String = lines
        .iter()
        .find_map(|l| {
            let t = l.trim();
            let up = t.to_ascii_uppercase();
            if up.starts_with("PACKAGE ") && !up.starts_with("PACKAGE BODY") {
                let name = ci_ident_after(t, "package ");
                if !name.is_empty() {
                    return Some(name);
                }
            }
            None
        })
        .unwrap_or_default();

    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("--!") || t.starts_with("---") {
            let (block, end) = collect_ada_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_ada_symbol(sym);
            // Qualify with the package name; skip for the package declaration itself.
            let qualified = if !pkg_name.is_empty() && kind != DocKind::Module && !name.is_empty() {
                format!("{pkg_name}.{name}")
            } else {
                name
            };
            let item = build_item(
                block,
                qualified,
                kind,
                file,
                end + 2,
                DocLanguage::Ada,
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

fn collect_ada_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("--!") || t.starts_with("---") {
            out.push(t[3..].trim_start().to_string());
            i += 1;
        } else {
            break;
        }
    }
    (out, i.saturating_sub(1))
}

fn detect_ada_symbol(line: &str) -> (String, DocKind) {
    let t = line.trim();
    let up = t.to_ascii_uppercase();
    if up.starts_with("PROCEDURE ") {
        return (ci_ident_after(t, "procedure "), DocKind::Subroutine);
    }
    if up.starts_with("FUNCTION ") {
        return (ci_ident_after(t, "function "), DocKind::Function);
    }
    if up.starts_with("PACKAGE ") {
        return (ci_ident_after(t, "package "), DocKind::Module);
    }
    if up.starts_with("TYPE ") {
        return (ci_ident_after(t, "type "), DocKind::Typedef);
    }
    (String::new(), DocKind::Unknown)
}
