use super::common::{build_item, ci_ident_after, first_ident, item_has_content, next_non_blank};
use super::{DocExtractor, DocItem, DocKind, DocLanguage};
use std::path::Path;

pub struct FortranExtractor;

impl DocExtractor for FortranExtractor {
    fn extensions(&self) -> &[&str] {
        &["f", "f90", "f95", "f03", "f08", "F90", "for", "ftn"]
    }
    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_fortran(src, path)
    }
}

pub(super) fn extract_fortran(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;
    let mut module_name = String::new();

    while i < lines.len() {
        let t = lines[i].trim();
        let up = t.to_ascii_uppercase();

        if up.starts_with("MODULE ")
            && !up.starts_with("MODULE SUBROUTINE ")
            && !up.starts_with("MODULE FUNCTION ")
            && !up.starts_with("MODULE PROCEDURE ")
        {
            module_name = ci_ident_after(t, "module ").to_ascii_lowercase();
        } else if up.starts_with("END MODULE") || up == "END MODULE" {
            module_name.clear();
        }

        if t.starts_with("!>") {
            let (block, end) = collect_fortran_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_fortran_symbol(sym);

            let (name, kind) = if kind == DocKind::Unknown && !module_name.is_empty() {
                if let Some(var_name) = detect_fortran_variable(sym) {
                    (var_name, DocKind::Variable)
                } else {
                    (name, kind)
                }
            } else {
                (name, kind)
            };

            // Qualify items inside a module so the tree groups them correctly,
            // but don't re-qualify the module declaration itself.
            let qualified = if !module_name.is_empty() && kind != DocKind::Module && !name.is_empty() {
                format!("{}.{}", module_name, name)
            } else {
                name
            };

            let item = build_item(
                block,
                qualified,
                kind,
                file,
                end + 2,
                DocLanguage::Fortran,
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

fn detect_fortran_variable(line: &str) -> Option<String> {
    let up = line.trim_start().to_ascii_uppercase();
    let is_type = [
        "INTEGER",
        "REAL",
        "DOUBLE PRECISION",
        "COMPLEX",
        "LOGICAL",
        "CHARACTER",
        "TYPE(",
    ]
    .iter()
    .any(|k| up.starts_with(k));
    if !is_type {
        return None;
    }
    let name = line
        .find("::")
        .map(|p| first_ident(line[p + 2..].trim_start()))
        .filter(|n| !n.is_empty())?;
    Some(name)
}

fn collect_fortran_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("!>") || t.starts_with("!!") {
            out.push(t[2..].trim_start().to_string());
            i += 1;
        } else {
            break;
        }
    }
    (out, i.saturating_sub(1))
}

fn detect_fortran_symbol(line: &str) -> (String, DocKind) {
    let t = line.trim();
    let up = t.to_ascii_uppercase();
    let up_sp = up.replace('(', " ").replace(')', " ");
    let tokens: Vec<&str> = up_sp.split_whitespace().collect();

    if up.starts_with("MODULE SUBROUTINE ") {
        return (ci_ident_after(t, "module subroutine "), DocKind::Subroutine);
    }
    if up.starts_with("MODULE FUNCTION ") {
        return (ci_ident_after(t, "module function "), DocKind::Function);
    }

    if let Some(pos) = tokens.iter().position(|w| *w == "SUBROUTINE") {
        if let Some(name_tok) = tokens.get(pos + 1) {
            let orig = original_token_at(t, pos + 1);
            return (first_ident(orig.unwrap_or(name_tok)), DocKind::Subroutine);
        }
    }
    if let Some(pos) = tokens.iter().position(|w| *w == "FUNCTION") {
        if let Some(name_tok) = tokens.get(pos + 1) {
            let orig = original_token_at(t, pos + 1);
            return (first_ident(orig.unwrap_or(name_tok)), DocKind::Function);
        }
    }

    if up.starts_with("MODULE ") && !up.starts_with("MODULE PROCEDURE") {
        return (ci_ident_after(t, "module "), DocKind::Module);
    }
    if up.contains("::") {
        let before = &up[..up.find("::").unwrap()];
        if before.trim() == "TYPE" {
            if let Some(after) = t.split("::").nth(1) {
                return (first_ident(after.trim()), DocKind::Struct);
            }
        }
    }
    (String::new(), DocKind::Unknown)
}

fn original_token_at(line: &str, n: usize) -> Option<&str> {
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|s| !s.is_empty())
        .nth(n)
}
