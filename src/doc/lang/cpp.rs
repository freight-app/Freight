use super::common::{
    build_item, collect_c_block, collect_line_block, first_ident, func_name_before_paren,
    item_has_content, next_decl_sym,
};
use super::{DocExtractor, DocItem, DocKind, DocLanguage, TagKind};
use std::path::Path;

pub struct CExtractor;
pub struct CppExtractor;

impl DocExtractor for CExtractor {
    fn extensions(&self) -> &[&str] {
        &["c", "h"]
    }
    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_c_style(src, path, &DocLanguage::C)
    }
}

impl DocExtractor for CppExtractor {
    fn extensions(&self) -> &[&str] {
        &[
            "cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "cppm", "ixx",
            "mpp", // C++20 module interfaces
            "cu", "cuh", "hip", "sycl", "ispc",
        ]
    }
    fn extract(&self, path: &Path, src: &str) -> Vec<DocItem> {
        extract_c_style(src, path, &DocLanguage::Cpp)
    }
}

pub(super) fn extract_c_style(src: &str, file: &Path, lang: &DocLanguage) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    let mut brace_depth: usize = 0;
    let mut ns_stack: Vec<(usize, String)> = Vec::new();
    let mut pending_ns: Option<String> = None;
    let mut class_stack: Vec<(usize, String)> = Vec::new();
    let mut pending_class: Option<String> = None;

    // C++20 module unit declared by `export module name;` or `export module name:part;`
    let mut module_unit: Option<String> = None;

    // Doxygen group tracking: @defgroup/@addtogroup/@{/@}
    let mut group_stack: Vec<String> = Vec::new();
    let mut pending_group: Option<String> = None;

    while i < lines.len() {
        let t = lines[i].trim();

        if (t.starts_with("/**") && !t.starts_with("/***/")) || t.starts_with("/*!") {
            let (block, end) = collect_c_block(&lines, i);
            apply_group_directives(&block, &mut group_stack, &mut pending_group);
            if !is_pure_group_block(&block) {
                if let Some(mut item) = build_file_doc_item(&block, file, i + 1, lang.clone()) {
                    item.meta.group =
                        ingroup_from_tags(&item).or_else(|| group_stack.last().cloned());
                    if item_has_content(&item) {
                        items.push(item);
                    }
                } else {
                    let sym = c_decl_sym(&lines, end + 1);
                    if !is_c_conditional_directive(&sym) {
                        let (name, kind) = detect_c_symbol(&sym);
                        let scope = current_cpp_scope(&ns_stack, &class_stack);
                        let mut item = build_item(
                            block,
                            qualify_name(&name, &scope),
                            kind.clone(),
                            file,
                            i + 1,
                            lang.clone(),
                            sym,
                        );
                        if matches!(kind, DocKind::Function | DocKind::Variable) {
                            item.meta.parent = class_stack
                                .last()
                                .map(|(_, p)| simple_scope_name(p).to_string());
                        }
                        item.meta.group =
                            ingroup_from_tags(&item).or_else(|| group_stack.last().cloned());
                        if item_has_content(&item) {
                            items.push(item);
                        }
                    }
                }
            }
            i = end + 1;
            continue;
        }

        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            apply_group_directives(&block, &mut group_stack, &mut pending_group);
            if !is_pure_group_block(&block) {
                if let Some(mut item) = build_file_doc_item(&block, file, i + 1, lang.clone()) {
                    item.meta.group =
                        ingroup_from_tags(&item).or_else(|| group_stack.last().cloned());
                    if item_has_content(&item) {
                        items.push(item);
                    }
                } else {
                    let sym = c_decl_sym(&lines, end + 1);
                    if !is_c_conditional_directive(&sym) {
                        let (name, kind) = detect_c_symbol(&sym);
                        let scope = current_cpp_scope(&ns_stack, &class_stack);
                        let mut item = build_item(
                            block,
                            qualify_name(&name, &scope),
                            kind.clone(),
                            file,
                            i + 1,
                            lang.clone(),
                            sym,
                        );
                        if matches!(kind, DocKind::Function | DocKind::Variable) {
                            item.meta.parent = class_stack
                                .last()
                                .map(|(_, p)| simple_scope_name(p).to_string());
                        }
                        item.meta.group =
                            ingroup_from_tags(&item).or_else(|| group_stack.last().cloned());
                        if item_has_content(&item) {
                            items.push(item);
                        }
                    }
                }
            }
            i = end + 1;
            continue;
        }

        if !t.starts_with("//") && !t.starts_with("/*") && !t.starts_with('*') {
            // C++20 module unit declaration (not preceded by a doc block).
            // `export module name;` or `export module name:partition;`
            // "export" is a qualifier, so strip_c_qualifiers converts it to `module ...`.
            {
                let stripped = strip_c_qualifiers(t);
                if let Some(rest) = stripped.strip_prefix("module ") {
                    let raw = rest.trim_end_matches(';').trim();
                    if !raw.is_empty() && !raw.starts_with(':') {
                        let name = raw.replace(':', ".");
                        module_unit = Some(name.clone());
                        push_namespace_source_item(&mut items, &name, file, i + 1, lang.clone(), t);
                        i += 1;
                        continue;
                    }
                }
            }

            // Match `namespace foo` or `export namespace foo`
            let ns_rest = t.strip_prefix("namespace").or_else(|| {
                t.strip_prefix("export ")
                    .map(str::trim_start)
                    .and_then(|r| r.strip_prefix("namespace"))
            });
            if let Some(rest) = ns_rest {
                let rest_ok =
                    rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '{');
                if rest_ok {
                    let name = first_ident(rest.trim_start());
                    if !name.is_empty() {
                        let path = match ns_stack.last() {
                            Some((_, p)) => format!("{p}::{name}"),
                            None => name,
                        };
                        push_namespace_source_item(&mut items, &path, file, i + 1, lang.clone(), t);
                        if t.contains('{') {
                            let opens = t.chars().filter(|&c| c == '{').count();
                            let closes = t.chars().filter(|&c| c == '}').count();
                            brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                            ns_stack.push((brace_depth, path));
                        } else {
                            pending_ns = Some(path);
                        }
                        i += 1;
                        continue;
                    }
                }
            }

            if matches!(lang, DocLanguage::Cpp) {
                if let Some((name, kind)) = class_decl_name(t) {
                    let scope = current_cpp_scope(&ns_stack, &class_stack);
                    let path = qualify_name(&name, &scope);
                    if t.contains('{') {
                        let opens = t.chars().filter(|&c| c == '{').count();
                        let closes = t.chars().filter(|&c| c == '}').count();
                        brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                        class_stack.push((brace_depth, path));
                    } else if matches!(kind, DocKind::Class | DocKind::Struct) {
                        pending_class = Some(path);
                    }
                    i += 1;
                    continue;
                }
            }

            if pending_ns.is_some() && t.contains('{') {
                let path = pending_ns.take().unwrap();
                let opens = t.chars().filter(|&c| c == '{').count();
                let closes = t.chars().filter(|&c| c == '}').count();
                brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                ns_stack.push((brace_depth, path));
                while ns_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                    ns_stack.pop();
                }
                while class_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                    class_stack.pop();
                }
                i += 1;
                continue;
            }

            if pending_class.is_some() && t.contains('{') {
                let path = pending_class.take().unwrap();
                let opens = t.chars().filter(|&c| c == '{').count();
                let closes = t.chars().filter(|&c| c == '}').count();
                brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                class_stack.push((brace_depth, path));
                while class_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                    class_stack.pop();
                }
                i += 1;
                continue;
            }

            let opens = t.chars().filter(|&c| c == '{').count();
            let closes = t.chars().filter(|&c| c == '}').count();
            brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
            while ns_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                ns_stack.pop();
            }
            while class_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                class_stack.pop();
            }
        }

        i += 1;
    }

    // Tag every item from a module partition with "cxx-module:lm.vec" so the
    // TUI can group them under the partition node rather than the flat namespace.
    if let Some(mu) = &module_unit {
        if mu.contains('.') {
            let attr = format!("cxx-module:{mu}");
            for item in items.iter_mut() {
                // Don't retag the module declaration item itself.
                if matches!(item.kind, DocKind::Module) && item.name == *mu {
                    continue;
                }
                item.meta.attrs.push(attr.clone());
            }
        }
    }

    items
}

pub(crate) fn detect_c_symbol(line: &str) -> (String, DocKind) {
    let t = strip_c_qualifiers(line.trim());

    if let Some(r) = t.strip_prefix("struct ") {
        return (first_ident(r), DocKind::Struct);
    }
    if let Some(r) = t.strip_prefix("class ") {
        return (first_ident(r), DocKind::Class);
    }
    if let Some(r) = t.strip_prefix("enum class ") {
        return (first_ident(r), DocKind::Enum);
    }
    if let Some(r) = t.strip_prefix("enum ") {
        return (first_ident(r), DocKind::Enum);
    }
    if let Some(r) = t.strip_prefix("namespace ") {
        return (first_ident(r), DocKind::Module);
    }
    // C++20: `export module name;` or `export module name:partition;`
    // "export" is already stripped by strip_c_qualifiers.
    if let Some(r) = t.strip_prefix("module ") {
        let raw = r.trim_end_matches(';').trim();
        // Skip private-fragment marker (`module :private;`)
        if !raw.is_empty() && !raw.starts_with(':') {
            let name = raw.replace(':', ".");
            return (name, DocKind::Module);
        }
    }
    if let Some(r) = t.strip_prefix("#define ") {
        return (first_ident(r), DocKind::Macro);
    }
    if t.starts_with("typedef ") {
        let candidate = t
            .trim_end_matches(';')
            .trim_end()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or("")
            .to_string();
        if !candidate.is_empty()
            && !matches!(candidate.as_str(), "struct" | "union" | "enum" | "class")
        {
            return (candidate, DocKind::Typedef);
        }
        return (String::new(), DocKind::Typedef);
    }
    if let Some(rest) = t.strip_prefix("using ") {
        let candidate = first_ident(rest);
        if !candidate.is_empty() {
            return (candidate, DocKind::Typedef);
        }
        return (String::new(), DocKind::Typedef);
    }
    if t.starts_with("template") {
        return (String::new(), DocKind::Unknown);
    }
    if let Some(name) = func_name_before_paren(t) {
        return (name, DocKind::Function);
    }
    (String::new(), DocKind::Unknown)
}

fn build_file_doc_item(
    block: &[String],
    file: &Path,
    line: usize,
    lang: DocLanguage,
) -> Option<DocItem> {
    if !block.iter().any(|line| is_file_directive(line)) {
        return None;
    }
    Some(build_item(
        block.to_vec(),
        String::new(),
        DocKind::Module,
        file,
        line,
        lang,
        String::new(),
    ))
}

fn push_namespace_source_item(
    items: &mut Vec<DocItem>,
    name: &str,
    file: &Path,
    line: usize,
    lang: DocLanguage,
    signature: &str,
) {
    if items
        .iter()
        .any(|item| item.kind == DocKind::Module && item.name == name && item.file == file)
    {
        return;
    }
    items.push(build_item(
        Vec::new(),
        name.to_string(),
        DocKind::Module,
        file,
        line,
        lang,
        signature.to_string(),
    ));
}

fn is_file_directive(line: &str) -> bool {
    let t = line.trim_start();
    t == "@file"
        || t == "\\file"
        || t.starts_with("@file ")
        || t.starts_with("\\file ")
        || t.starts_with("@file\t")
        || t.starts_with("\\file\t")
}

fn c_decl_sym(lines: &[&str], from: usize) -> String {
    let first = next_decl_sym(lines, from);
    let mut out = first.to_string();
    let typedef_decl = is_typedef_decl(first);
    let aggregate_typedef = is_multiline_aggregate_typedef(first);
    let mut paren_depth = paren_delta(first);
    let mut brace_depth = signed_brace_delta(first);
    if c_decl_complete(
        first,
        paren_depth,
        brace_depth,
        typedef_decl,
        aggregate_typedef,
    ) {
        return out;
    }

    let mut i = first_decl_index(lines, from).map_or(from, |idx| idx + 1);
    while let Some(line) = lines.get(i) {
        let t = line.trim();
        if t.is_empty() {
            i += 1;
            continue;
        }
        out.push('\n');
        out.push_str(t);
        paren_depth += paren_delta(t);
        brace_depth += signed_brace_delta(t);
        if c_decl_complete(t, paren_depth, brace_depth, typedef_decl, aggregate_typedef) {
            break;
        }
        i += 1;
    }
    out
}

fn c_decl_complete(
    line: &str,
    paren_depth: isize,
    brace_depth: isize,
    typedef_decl: bool,
    aggregate_typedef: bool,
) -> bool {
    let t = line.trim_end();
    if typedef_decl {
        return paren_depth <= 0 && brace_depth <= 0 && t.ends_with(';');
    }
    if aggregate_typedef {
        return brace_depth <= 0 && t.ends_with(';');
    }
    paren_depth <= 0 && (t.ends_with(';') || t.contains('{'))
}

fn is_typedef_decl(line: &str) -> bool {
    strip_c_qualifiers(line.trim_start()).starts_with("typedef ")
}

fn first_decl_index(lines: &[&str], from: usize) -> Option<usize> {
    let mut i = from;
    loop {
        let t = lines.get(i)?.trim();
        if t.is_empty() || t.starts_with("template") {
            i += 1;
            continue;
        }
        return Some(i);
    }
}

fn is_multiline_aggregate_typedef(line: &str) -> bool {
    let t = strip_c_qualifiers(line.trim_start());
    t.starts_with("typedef ")
        && ["struct", "union", "enum"]
            .iter()
            .any(|kw| t.contains(&format!(" {kw}")) || t.contains(&format!(" {kw} ")))
        && t.contains('{')
        && brace_delta(t) > 0
}

fn brace_delta(line: &str) -> usize {
    let opens = line.chars().filter(|&c| c == '{').count();
    let closes = line.chars().filter(|&c| c == '}').count();
    opens.saturating_sub(closes)
}

fn signed_brace_delta(line: &str) -> isize {
    let opens = line.chars().filter(|&c| c == '{').count() as isize;
    let closes = line.chars().filter(|&c| c == '}').count() as isize;
    opens - closes
}

fn paren_delta(line: &str) -> isize {
    let opens = line.chars().filter(|&c| c == '(').count() as isize;
    let closes = line.chars().filter(|&c| c == ')').count() as isize;
    opens - closes
}

fn qualify_name(name: &str, ns: &str) -> String {
    if name.is_empty() || ns.is_empty() {
        name.to_string()
    } else {
        format!("{ns}::{name}")
    }
}

fn current_cpp_scope(ns_stack: &[(usize, String)], class_stack: &[(usize, String)]) -> String {
    class_stack
        .last()
        .or_else(|| ns_stack.last())
        .map(|(_, p)| p.clone())
        .unwrap_or_default()
}

fn simple_scope_name(path: &str) -> &str {
    path.rsplit_once("::").map(|(_, name)| name).unwrap_or(path)
}

fn class_decl_name(t: &str) -> Option<(String, DocKind)> {
    let t = strip_c_qualifiers(t.trim_start());
    if let Some(r) = t.strip_prefix("class ") {
        return Some((first_ident(r), DocKind::Class));
    }
    if let Some(r) = t.strip_prefix("struct ") {
        return Some((first_ident(r), DocKind::Struct));
    }
    None
}

fn is_c_conditional_directive(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("#if")
        || t.starts_with("#elif")
        || t.starts_with("#else")
        || t.starts_with("#endif")
}

fn strip_c_qualifiers(mut t: &str) -> &str {
    const QUALS: &[&str] = &[
        "static ",
        "inline ",
        "extern ",
        "explicit ",
        "virtual ",
        "constexpr ",
        "consteval ",
        "constinit ",
        "__inline ",
        "__inline__ ",
        "__forceinline ",
        "__global__ ",
        "__device__ ",
        "__host__ ",
        "__shared__ ",
        "__constant__ ",
        "__managed__ ",
        "task ",
        "export ",
        "unmasked ",
        "[[nodiscard]] ",
        "[[maybe_unused]] ",
    ];
    'outer: loop {
        for q in QUALS {
            if let Some(rest) = t.strip_prefix(q) {
                t = rest.trim_start();
                continue 'outer;
            }
        }
        break;
    }
    t
}

// ── Doxygen group helpers ─────────────────────────────────────────────────────

/// Process any `@defgroup`, `@addtogroup`, `@{`, `@}` lines in `block` and
/// update `group_stack` / `pending_group` accordingly.
fn apply_group_directives(
    block: &[String],
    group_stack: &mut Vec<String>,
    pending_group: &mut Option<String>,
) {
    for line in block {
        let t = line.trim();
        if t == "@{" || t == "{" {
            let g = pending_group.take().or_else(|| group_stack.last().cloned());
            if let Some(g) = g {
                group_stack.push(g);
            }
        } else if t == "@}" || t == "}" {
            group_stack.pop();
        } else if let Some(rest) = t
            .strip_prefix("@defgroup ")
            .or_else(|| t.strip_prefix("\\defgroup "))
            .or_else(|| t.strip_prefix("@addtogroup "))
            .or_else(|| t.strip_prefix("\\addtogroup "))
        {
            let name = first_ident(rest);
            if !name.is_empty() && pending_group.is_none() {
                *pending_group = Some(name);
            }
        }
    }
}

/// Return `true` when the block contains only group-control directives
/// (`@{`, `@}`, `@defgroup`, `@addtogroup`) and no real documentation.
fn is_pure_group_block(block: &[String]) -> bool {
    let non_empty: Vec<&str> = block
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if non_empty.is_empty() {
        return false;
    }
    non_empty.iter().all(|l| {
        *l == "@{"
            || *l == "@}"
            || *l == "{"
            || *l == "}"
            || l.starts_with("@defgroup ")
            || l.starts_with("\\defgroup ")
            || l.starts_with("@addtogroup ")
            || l.starts_with("\\addtogroup ")
            || *l == "@defgroup"
            || *l == "@addtogroup"
    })
}

/// Extract an explicit `@ingroup groupname` from an item's tag list.
fn ingroup_from_tags(item: &DocItem) -> Option<String> {
    item.tags
        .iter()
        .find(|t| t.kind == TagKind::Other("ingroup".to_string()))
        .map(|t| first_ident(&t.text))
        .filter(|s| !s.is_empty())
}
