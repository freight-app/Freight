use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use super::lang::{DocItem, DocKind, DocLanguage, DocSet, DocTag, TagKind};

/// Write the documentation as a set of inter-linked Markdown files.
///
/// Output structure:
/// ```text
/// <out_dir>/
///   index.md             - package overview, namespace/module listing
///   symbols.md           - alphabetical cross-language symbol index
///   namespace/<ns>.md    - one page per C++ namespace (:: -> __, lower-case)
///   class/<cls>.md       - one page per C++ class/struct
///   module/<mod>.md      - one page per Fortran MODULE
///   <file_slug>.md       - fallback per-file pages for items not in any scope
/// ```
///
/// Math: `$...$` and `$$...$$` are passed through verbatim.
pub fn render_markdown(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    std::fs::create_dir_all(out_dir.join("namespace"))?;
    std::fs::create_dir_all(out_dir.join("class"))?;
    std::fs::create_dir_all(out_dir.join("module"))?;
    std::fs::create_dir_all(out_dir.join("group"))?;

    let (groups, module_by_file) = group_items(&set.items, &set.source_root);
    let sym_idx = SymbolIndex::build(&set.items, &set.source_root, &module_by_file);

    // Namespace pages
    for (ns, items) in &groups.namespaces {
        let page = format!("namespace/{}.md", ns_slug(ns));
        let content = render_namespace_page(ns, items, &sym_idx, &page);
        std::fs::write(out_dir.join(&page), content)?;
    }

    // Class pages
    for (cls, items) in &groups.classes {
        let page = format!("class/{}.md", class_slug(cls));
        let content = render_class_page(cls, items, &sym_idx, &page);
        std::fs::write(out_dir.join(&page), content)?;
    }

    // Fortran module pages
    for (mod_name, items) in &groups.modules {
        let page = format!("module/{}.md", mod_name.to_ascii_lowercase());
        let content = render_module_page(mod_name, items, &sym_idx, &page);
        std::fs::write(out_dir.join(&page), content)?;
    }

    // Doxygen group pages
    for (g, items) in &groups.groups {
        let slug = group_slug(g);
        let page = format!("group/{slug}.md");
        let content = render_group_page(g, items, &sym_idx, &page);
        std::fs::write(out_dir.join(&page), content)?;
    }

    // Per-file fallback pages
    for (rel, items) in &groups.files {
        let slug = rel_to_slug(rel);
        let content = render_file_page(rel, &slug, items);
        std::fs::write(out_dir.join(format!("{slug}.md")), content)?;
    }

    // Global index and symbol list
    let mut by_file: BTreeMap<String, Vec<&DocItem>> = BTreeMap::new();
    for item in &set.items {
        let rel = item
            .file
            .strip_prefix(&set.source_root)
            .unwrap_or(&item.file)
            .to_string_lossy()
            .into_owned();
        by_file.entry(rel).or_default().push(item);
    }
    std::fs::write(out_dir.join("index.md"), render_index(&groups))?;
    std::fs::write(out_dir.join("symbols.md"), render_symbol_index(&by_file))?;

    Ok(())
}

// ── Item grouping ─────────────────────────────────────────────────────────────

struct Groups<'a> {
    /// C++/Rust namespace (or module) qualified path → items at that scope.
    namespaces: BTreeMap<String, Vec<&'a DocItem>>,
    /// Qualified class/struct name → items belonging to that class.
    classes: BTreeMap<String, Vec<&'a DocItem>>,
    /// Fortran module name → all items from that module's source file.
    modules: BTreeMap<String, Vec<&'a DocItem>>,
    /// Doxygen @defgroup/@addtogroup group name → items in that group.
    groups: BTreeMap<String, Vec<&'a DocItem>>,
    /// Fallback: source-relative path → items not assigned to any scope.
    files: BTreeMap<String, Vec<&'a DocItem>>,
}

fn group_items<'a>(
    items: &'a [DocItem],
    source_root: &Path,
) -> (Groups<'a>, HashMap<String, String>) {
    // First pass: map source file → Fortran module name.
    let mut module_by_file: HashMap<String, String> = HashMap::new();
    for item in items {
        if item.lang == DocLanguage::Fortran
            && item.kind == DocKind::Module
            && !item.name.is_empty()
        {
            let rel = rel_path(item, source_root);
            module_by_file.insert(rel, item.name.clone());
        }
    }

    let mut groups = Groups {
        namespaces: BTreeMap::new(),
        classes: BTreeMap::new(),
        modules: BTreeMap::new(),
        groups: BTreeMap::new(),
        files: BTreeMap::new(),
    };

    for item in items {
        let is_cpp = matches!(item.lang, DocLanguage::C | DocLanguage::Cpp);
        // Rust uses `::` for mod qualification, same routing as C++ namespaces.
        let has_ns_scope = is_cpp || item.lang == DocLanguage::Rust;

        // Record Doxygen group membership (can co-exist with namespace routing).
        if let Some(ref g) = item.meta.group {
            groups.groups.entry(g.clone()).or_default().push(item);
        }

        // Class member (populated by libclang extractor).
        if is_cpp && item.meta.parent.is_some() {
            let parent_qualified = parent_qualified_name(item);
            groups
                .classes
                .entry(parent_qualified)
                .or_default()
                .push(item);
            continue;
        }

        // C++ class/struct definition — goes on both its class page and namespace page.
        if is_cpp
            && matches!(item.kind, DocKind::Class | DocKind::Struct)
            && item.name.contains("::")
        {
            groups
                .classes
                .entry(item.name.clone())
                .or_default()
                .insert(0, item);
            let ns = &item.name[..item.name.rfind("::").unwrap()];
            groups
                .namespaces
                .entry(ns.to_string())
                .or_default()
                .push(item);
            continue;
        }

        // C++ or Rust items with a scope qualifier (`::`) → namespace/module page.
        if has_ns_scope && item.name.contains("::") {
            let ns = &item.name[..item.name.rfind("::").unwrap()];
            groups
                .namespaces
                .entry(ns.to_string())
                .or_default()
                .push(item);
            continue;
        }

        // Fortran: route to module page if the file contains a MODULE declaration.
        if item.lang == DocLanguage::Fortran {
            let rel = rel_path(item, source_root);
            if let Some(mod_name) = module_by_file.get(&rel) {
                groups
                    .modules
                    .entry(mod_name.clone())
                    .or_default()
                    .push(item);
                continue;
            }
        }

        // Fallback: per-file page.
        let rel = rel_path(item, source_root);
        groups.files.entry(rel).or_default().push(item);
    }

    (groups, module_by_file)
}

fn rel_path(item: &DocItem, source_root: &Path) -> String {
    item.file
        .strip_prefix(source_root)
        .unwrap_or(&item.file)
        .to_string_lossy()
        .into_owned()
}

/// Extract the fully-qualified parent class name for a class member.
fn parent_qualified_name(item: &DocItem) -> String {
    if item.name.contains("::") {
        item.name[..item.name.rfind("::").unwrap()].to_string()
    } else {
        item.meta.parent.as_deref().unwrap_or("").to_string()
    }
}

// ── SymbolIndex ───────────────────────────────────────────────────────────────

struct SymbolIndex {
    /// qualified_name → (page path relative to doc root, anchor).
    map: HashMap<String, (String, String)>,
}

impl SymbolIndex {
    fn build(
        items: &[DocItem],
        source_root: &Path,
        module_by_file: &HashMap<String, String>,
    ) -> Self {
        let mut map = HashMap::new();
        for item in items {
            if item.name.is_empty() {
                continue;
            }
            let page = page_path_for(item, source_root, module_by_file);
            let anchor = item_anchor_simple(item);
            map.insert(item.name.clone(), (page, anchor));
        }
        Self { map }
    }

    /// Return a Markdown link `[name](rel_path#anchor)` resolved from `from_page`,
    /// or `None` when the symbol is unknown.
    fn link_for(&self, name: &str, from_page: &str) -> Option<String> {
        let (target_page, anchor) = self.map.get(name)?;
        let rel = relative_page(from_page, target_page);
        Some(format!("[{name}]({rel}#{anchor})"))
    }
}

/// Determine the output page path (relative to doc root) for a given item.
fn page_path_for(
    item: &DocItem,
    source_root: &Path,
    module_by_file: &HashMap<String, String>,
) -> String {
    let is_cpp = matches!(item.lang, DocLanguage::C | DocLanguage::Cpp);
    let has_ns_scope = is_cpp || item.lang == DocLanguage::Rust;

    if is_cpp && item.meta.parent.is_some() {
        let parent = parent_qualified_name(item);
        return format!("class/{}.md", class_slug(&parent));
    }
    if is_cpp && matches!(item.kind, DocKind::Class | DocKind::Struct) && item.name.contains("::") {
        return format!("class/{}.md", class_slug(&item.name));
    }
    if has_ns_scope && item.name.contains("::") {
        let ns = &item.name[..item.name.rfind("::").unwrap()];
        return format!("namespace/{}.md", ns_slug(ns));
    }
    if item.lang == DocLanguage::Fortran {
        let rel = rel_path(item, source_root);
        if let Some(mod_name) = module_by_file.get(&rel) {
            return format!("module/{}.md", mod_name.to_ascii_lowercase());
        }
    }
    let rel = rel_path(item, source_root);
    format!("{}.md", rel_to_slug(&rel))
}

/// Simple anchor for structured pages: `{kind}-{simple_lowercase_name}`.
fn item_anchor_simple(item: &DocItem) -> String {
    let simple = item
        .name
        .rfind("::")
        .map_or(item.name.as_str(), |p| &item.name[p + 2..]);
    let slug: String = simple
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("{}-{}", item.kind.label(), slug)
}

/// Compute a relative link from `from_page` to `to_page` (both relative to doc root).
fn relative_page(from_page: &str, to_page: &str) -> String {
    let from_dir = from_page.rfind('/').map(|p| &from_page[..p]).unwrap_or("");
    if from_dir.is_empty() {
        to_page.to_string()
    } else {
        // Both are at depth 1 (subdirectory pages) or from is depth 1, to is depth 0.
        let to_dir = to_page.rfind('/').map(|p| &to_page[..p]).unwrap_or("");
        if from_dir == to_dir {
            to_page
                .rfind('/')
                .map(|p| &to_page[p + 1..])
                .unwrap_or(to_page)
                .to_string()
        } else if to_dir.is_empty() {
            format!("../{to_page}")
        } else {
            format!("../{to_page}")
        }
    }
}

/// Resolve `@see` tag text to a Markdown link when the text matches a known symbol.
fn resolve_see(text: &str, sym: &SymbolIndex, from_page: &str) -> String {
    let candidate = text.trim();
    sym.link_for(candidate, from_page)
        .unwrap_or_else(|| candidate.to_string())
}

// ── Namespace pages ───────────────────────────────────────────────────────────

fn render_namespace_page(ns: &str, items: &[&DocItem], sym: &SymbolIndex, page: &str) -> String {
    let mut md = String::new();
    let depth_prefix = "../";

    let _ = writeln!(md, "# namespace `{ns}`\n");
    let _ = writeln!(
        md,
        "[← Index]({depth_prefix}index.md) | [All symbols]({depth_prefix}symbols.md)\n"
    );
    let _ = writeln!(md, "---\n");

    // Group into kinds
    let classes: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| matches!(i.kind, DocKind::Class | DocKind::Struct))
        .collect();
    let fns: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| matches!(i.kind, DocKind::Function | DocKind::Subroutine))
        .collect();
    let vars: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| {
            matches!(
                i.kind,
                DocKind::Variable | DocKind::Macro | DocKind::Typedef
            )
        })
        .collect();

    // Sub-namespaces: namespace entries whose name starts with `ns::` and has another `::` after.
    let sub_ns: Vec<String> = {
        let mut v: Vec<String> = Vec::new();
        // Check items for nested namespaces (names like ns::sub::foo).
        for item in items {
            if item.name.starts_with(&format!("{ns}::")) {
                let after = &item.name[ns.len() + 2..];
                if let Some(p) = after.find("::") {
                    let sub = format!("{ns}::{}", &after[..p]);
                    if !v.contains(&sub) {
                        v.push(sub);
                    }
                }
            }
        }
        v.sort();
        v
    };

    if !sub_ns.is_empty() {
        let _ = writeln!(md, "## Sub-namespaces\n");
        for sub in &sub_ns {
            let slug = ns_slug(sub);
            let _ = writeln!(md, "- [`{sub}`]({slug}.md)");
        }
        let _ = writeln!(md);
    }

    if !classes.is_empty() {
        let _ = writeln!(md, "## Types\n");
        md.push_str("| Name | Summary |\n|------|---------|\n");
        for item in &classes {
            let simple = item
                .name
                .rfind("::")
                .map(|p| &item.name[p + 2..])
                .unwrap_or(&item.name);
            let cls_slug = class_slug(&item.name);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(
                md,
                "| [{kind} `{simple}`](../class/{cls_slug}.md) | {brief} |",
                kind = item.kind.label()
            );
        }
        let _ = writeln!(md);
    }

    if !fns.is_empty() {
        let _ = writeln!(md, "## Functions\n");
        md.push_str("| Signature | Summary |\n|-----------|---------|\n");
        for item in &fns {
            let anchor = item_anchor_simple(item);
            let simple = item
                .name
                .rfind("::")
                .map(|p| &item.name[p + 2..])
                .unwrap_or(&item.name);
            let sig = if item.signature.is_empty() {
                simple.to_string()
            } else {
                item.display_signature()
            };
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{sig}`](#{anchor}) | {brief} |");
        }
        let _ = writeln!(md);
    }

    if !vars.is_empty() {
        let _ = writeln!(md, "## Variables / Types\n");
        md.push_str("| Name | Kind | Summary |\n|------|------|---------|\n");
        for item in &vars {
            let anchor = item_anchor_simple(item);
            let simple = item
                .name
                .rfind("::")
                .map(|p| &item.name[p + 2..])
                .unwrap_or(&item.name);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(
                md,
                "| [`{simple}`](#{anchor}) | {} | {brief} |",
                item.kind.label()
            );
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "---\n");
    let _ = writeln!(md, "## Detailed Documentation\n");

    let mut name_count: BTreeMap<String, usize> = BTreeMap::new();
    for item in fns.iter().chain(vars.iter()) {
        let anchor = item_anchor_simple(item);
        render_item(&mut md, item, &mut name_count, &anchor, sym, page);
    }

    md
}

// ── Class pages ───────────────────────────────────────────────────────────────

fn render_class_page(cls: &str, items: &[&DocItem], sym: &SymbolIndex, page: &str) -> String {
    let mut md = String::new();
    let depth_prefix = "../";

    let simple = cls.rfind("::").map(|p| &cls[p + 2..]).unwrap_or(cls);
    let ns = cls.rfind("::").map(|p| &cls[..p]);

    let _ = writeln!(md, "# class `{simple}`\n");
    let _ = writeln!(
        md,
        "[← Index]({depth_prefix}index.md) | [All symbols]({depth_prefix}symbols.md)"
    );
    if let Some(ns) = ns {
        let ns_slug = ns_slug(ns);
        let _ = writeln!(md, "**Namespace:** [`{ns}`](../namespace/{ns_slug}.md)");
    }
    let _ = writeln!(md, "\n---\n");

    // Class item (first element if kind is Class/Struct)
    let class_items: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| matches!(i.kind, DocKind::Class | DocKind::Struct) && i.name == cls)
        .collect();
    let members: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| !(matches!(i.kind, DocKind::Class | DocKind::Struct) && i.name == cls))
        .collect();

    if let Some(cls_item) = class_items.first() {
        if !cls_item.brief.is_empty() {
            let _ = writeln!(md, "{}\n", cls_item.brief);
        }
        if !cls_item.body.is_empty() {
            let _ = writeln!(md, "{}\n", cls_item.body);
        }
    }

    // Group members by kind/access
    let pub_fns: Vec<&DocItem> = members
        .iter()
        .copied()
        .filter(|i| matches!(i.kind, DocKind::Function | DocKind::Subroutine))
        .filter(|i| !matches!(i.meta.access, Some(super::lang::Access::Private)))
        .collect();
    let pub_vars: Vec<&DocItem> = members
        .iter()
        .copied()
        .filter(|i| matches!(i.kind, DocKind::Variable))
        .filter(|i| !matches!(i.meta.access, Some(super::lang::Access::Private)))
        .collect();

    if !pub_fns.is_empty() {
        let _ = writeln!(md, "## Public Member Functions\n");
        md.push_str("| Signature | Summary |\n|-----------|---------|\n");
        for item in &pub_fns {
            let anchor = item_anchor_simple(item);
            let simple_name = item
                .name
                .rfind("::")
                .map(|p| &item.name[p + 2..])
                .unwrap_or(&item.name);
            let sig = if item.signature.is_empty() {
                simple_name.to_string()
            } else {
                item.display_signature()
            };
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{sig}`](#{anchor}) | {brief} |");
        }
        let _ = writeln!(md);
    }

    if !pub_vars.is_empty() {
        let _ = writeln!(md, "## Public Data Members\n");
        md.push_str("| Name | Summary |\n|------|---------|\n");
        for item in &pub_vars {
            let anchor = item_anchor_simple(item);
            let simple_name = item
                .name
                .rfind("::")
                .map(|p| &item.name[p + 2..])
                .unwrap_or(&item.name);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{simple_name}`](#{anchor}) | {brief} |");
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "---\n");
    let _ = writeln!(md, "## Detailed Documentation\n");

    let mut name_count: BTreeMap<String, usize> = BTreeMap::new();
    for item in pub_fns.iter().chain(pub_vars.iter()).copied() {
        let anchor = item_anchor_simple(item);
        render_item(&mut md, item, &mut name_count, &anchor, sym, page);
    }

    md
}

// ── Fortran module pages ──────────────────────────────────────────────────────

fn render_module_page(mod_name: &str, items: &[&DocItem], sym: &SymbolIndex, page: &str) -> String {
    let mut md = String::new();
    let depth_prefix = "../";

    let _ = writeln!(md, "# module `{mod_name}`\n");
    let _ = writeln!(
        md,
        "[← Index]({depth_prefix}index.md) | [All symbols]({depth_prefix}symbols.md)\n"
    );
    let _ = writeln!(md, "---\n");

    // Module declaration item
    if let Some(mod_item) = items.iter().find(|i| i.kind == DocKind::Module) {
        if !mod_item.brief.is_empty() {
            let _ = writeln!(md, "{}\n", mod_item.brief);
        }
        if !mod_item.body.is_empty() {
            let _ = writeln!(md, "{}\n", mod_item.body);
        }
    }

    let subs: Vec<_> = items
        .iter()
        .filter(|i| matches!(i.kind, DocKind::Subroutine | DocKind::Function))
        .collect();
    let vars: Vec<_> = items
        .iter()
        .filter(|i| i.kind == DocKind::Variable)
        .collect();
    let types: Vec<_> = items
        .iter()
        .filter(|i| {
            matches!(
                i.kind,
                DocKind::Struct | DocKind::Typedef | DocKind::Interface
            )
        })
        .collect();

    if !subs.is_empty() {
        let _ = writeln!(md, "## Procedures\n");
        md.push_str("| Name | Summary |\n|------|---------|\n");
        for item in &subs {
            let anchor = item_anchor_simple(item);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{}`](#{anchor}) | {brief} |", item.name);
        }
        let _ = writeln!(md);
    }

    if !vars.is_empty() {
        let _ = writeln!(md, "## Module Variables\n");
        md.push_str("| Name | Summary |\n|------|---------|\n");
        for item in &vars {
            let anchor = item_anchor_simple(item);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{}`](#{anchor}) | {brief} |", item.name);
        }
        let _ = writeln!(md);
    }

    if !types.is_empty() {
        let _ = writeln!(md, "## Types\n");
        md.push_str("| Name | Summary |\n|------|---------|\n");
        for item in &types {
            let anchor = item_anchor_simple(item);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{}`](#{anchor}) | {brief} |", item.name);
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "---\n");
    let _ = writeln!(md, "## Detailed Documentation\n");

    let mut name_count: BTreeMap<String, usize> = BTreeMap::new();
    for item in subs.iter().chain(vars.iter()).chain(types.iter()) {
        let anchor = item_anchor_simple(item);
        render_item(&mut md, item, &mut name_count, &anchor, sym, page);
    }

    md
}

// ── Doxygen group pages ───────────────────────────────────────────────────────

fn render_group_page(group: &str, items: &[&DocItem], sym: &SymbolIndex, page: &str) -> String {
    let mut md = String::new();
    let depth_prefix = "../";

    let _ = writeln!(md, "# group `{group}`\n");
    let _ = writeln!(
        md,
        "[← Index]({depth_prefix}index.md) | [All symbols]({depth_prefix}symbols.md)\n"
    );
    let _ = writeln!(md, "---\n");

    let fns: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| matches!(i.kind, DocKind::Function | DocKind::Subroutine))
        .collect();
    let types: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| {
            matches!(
                i.kind,
                DocKind::Class
                    | DocKind::Struct
                    | DocKind::Enum
                    | DocKind::Typedef
                    | DocKind::Interface
            )
        })
        .collect();
    let vars: Vec<&DocItem> = items
        .iter()
        .copied()
        .filter(|i| matches!(i.kind, DocKind::Variable | DocKind::Macro))
        .collect();

    if !types.is_empty() {
        let _ = writeln!(md, "## Types\n");
        md.push_str("| Name | Summary |\n|------|---------|\n");
        for item in &types {
            let anchor = item_anchor_simple(item);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(
                md,
                "| [{kind} `{name}`](#{anchor}) | {brief} |",
                kind = item.kind.label(),
                name = item.name
            );
        }
        let _ = writeln!(md);
    }

    if !fns.is_empty() {
        let _ = writeln!(md, "## Functions\n");
        md.push_str("| Signature | Summary |\n|-----------|---------|\n");
        for item in &fns {
            let anchor = item_anchor_simple(item);
            let sig = if item.signature.is_empty() {
                item.name.clone()
            } else {
                item.display_signature()
            };
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{sig}`](#{anchor}) | {brief} |");
        }
        let _ = writeln!(md);
    }

    if !vars.is_empty() {
        let _ = writeln!(md, "## Variables / Macros\n");
        md.push_str("| Name | Summary |\n|------|---------|\n");
        for item in &vars {
            let anchor = item_anchor_simple(item);
            let brief = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(md, "| [`{}`](#{anchor}) | {brief} |", item.name);
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "---\n");
    let _ = writeln!(md, "## Detailed Documentation\n");

    let mut name_count: BTreeMap<String, usize> = BTreeMap::new();
    for item in fns.iter().chain(vars.iter()).chain(types.iter()) {
        let anchor = item_anchor_simple(item);
        render_item(&mut md, item, &mut name_count, &anchor, sym, page);
    }
    md
}

// ── Index page ────────────────────────────────────────────────────────────────

fn render_index(groups: &Groups<'_>) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# Documentation\n");
    md.push_str(&nav_tabs("Files"));

    if !groups.namespaces.is_empty() {
        let _ = writeln!(md, "## Namespaces\n");
        md.push_str("| Namespace | Items |\n|-----------|------:|\n");
        for (ns, items) in &groups.namespaces {
            let slug = ns_slug(ns);
            let _ = writeln!(md, "| [`{ns}`](namespace/{slug}.md) | {} |", items.len());
        }
        let _ = writeln!(md);
    }

    if !groups.classes.is_empty() {
        let _ = writeln!(md, "## Classes\n");
        md.push_str("| Class | Items |\n|-------|------:|\n");
        for (cls, items) in &groups.classes {
            let slug = class_slug(cls);
            let _ = writeln!(md, "| [`{cls}`](class/{slug}.md) | {} |", items.len());
        }
        let _ = writeln!(md);
    }

    if !groups.modules.is_empty() {
        let _ = writeln!(md, "## Fortran Modules\n");
        md.push_str("| Module | Items |\n|--------|------:|\n");
        for (mod_name, items) in &groups.modules {
            let slug = mod_name.to_ascii_lowercase();
            let _ = writeln!(md, "| [`{mod_name}`](module/{slug}.md) | {} |", items.len());
        }
        let _ = writeln!(md);
    }

    if !groups.groups.is_empty() {
        let _ = writeln!(md, "## Groups\n");
        md.push_str("| Group | Items |\n|-------|------:|\n");
        for (g, items) in &groups.groups {
            let slug = group_slug(g);
            let _ = writeln!(md, "| [`{g}`](group/{slug}.md) | {} |", items.len());
        }
        let _ = writeln!(md);
    }

    if !groups.files.is_empty() {
        let _ = writeln!(md, "## Files\n");
        md.push_str("| File | Items |\n|------|------:|\n");
        for (rel, items) in &groups.files {
            let slug = rel_to_slug(rel);
            let _ = writeln!(md, "| [{rel}]({slug}.md) | {} |", items.len());
        }
        let _ = writeln!(md);
    }

    md
}

// ── Symbol index ──────────────────────────────────────────────────────────────

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
            by_kind
                .entry(kind_title(&item.kind))
                .or_default()
                .push((rel.as_str(), item));
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
            let summary = md_table_escape(if item.brief.is_empty() {
                "—"
            } else {
                &item.brief
            });
            let _ = writeln!(
                md,
                "| [{label} `{name}`]({slug}.md#{anchor}) | [{rel}]({slug}.md) | {line} | {summary} |",
                label = item.kind.label(),
                name  = md_table_escape(&item.name),
                line  = item.line,
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

// ── Per-file page (fallback) ──────────────────────────────────────────────────

fn render_file_page(rel: &str, slug: &str, items: &[&DocItem]) -> String {
    let _ = slug;
    let mut md = String::new();

    let _ = writeln!(md, "# `{rel}`\n");
    if let Some(lang) = items.first().map(|i| i.lang.label()) {
        let _ = writeln!(md, "**Language:** {lang}\n");
    }
    let _ = writeln!(md, "[← Index](index.md) | [All symbols](symbols.md)\n");
    let _ = writeln!(md, "---\n");

    let named: Vec<&&DocItem> = items.iter().filter(|i| !i.name.is_empty()).collect();
    if named.len() > 3 {
        let _ = writeln!(md, "## Contents\n");
        for item in &named {
            let anchor = symbol_anchor(item);
            let _ = writeln!(
                md,
                "- [{kind} `{name}`](#{anchor})",
                kind = item.kind.label(),
                name = item.name
            );
        }
        let _ = writeln!(md);
        md.push_str("---\n\n");
    }

    let dummy_sym = SymbolIndex {
        map: HashMap::new(),
    };
    let page = format!("{}.md", rel_to_slug(rel));
    let mut name_count: BTreeMap<String, usize> = BTreeMap::new();
    for item in items {
        let anchor = symbol_anchor(item);
        render_item(&mut md, item, &mut name_count, &anchor, &dummy_sym, &page);
    }

    md
}

// ── Item renderer ─────────────────────────────────────────────────────────────

fn render_item(
    md: &mut String,
    item: &DocItem,
    name_count: &mut BTreeMap<String, usize>,
    anchor: &str,
    sym: &SymbolIndex,
    page: &str,
) {
    let simple_name = item
        .name
        .rfind("::")
        .map(|p| &item.name[p + 2..])
        .unwrap_or(&item.name);
    let display = if item.name.is_empty() {
        "*(anonymous)*".to_string()
    } else {
        format!("`{simple_name}`")
    };

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
        let _ = writeln!(md, "```\n{}\n```\n", item.display_signature());
    }

    if !item.brief.is_empty() {
        let _ = writeln!(md, "{}\n", item.brief);
    }
    if !item.body.is_empty() {
        let _ = writeln!(md, "{}\n", item.body);
    }

    let params: Vec<&DocTag> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Param)
        .collect();
    if !params.is_empty() {
        render_parameter_table(md, &params);
    }

    for tag in &item.tags {
        if matches!(tag.kind, TagKind::Param | TagKind::Brief) {
            continue;
        }
        let text = if tag.kind == TagKind::See {
            resolve_see(&tag.text, sym, page)
        } else {
            tag.text.clone()
        };
        let _ = writeln!(md, "**{}:** {}\n", tag.kind.label(), text);
    }

    let _ = writeln!(md, "---\n");
}

fn render_parameter_table(md: &mut String, params: &[&DocTag]) {
    md.push_str(
        "<table style=\"border-collapse: collapse; margin: 0.75rem 0 1rem; font-size: 0.92em;\">\n",
    );
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
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

// ── Slugging / anchors ────────────────────────────────────────────────────────

fn ns_slug(ns: &str) -> String {
    ns.replace("::", "__").to_ascii_lowercase()
}

fn class_slug(name: &str) -> String {
    name.replace("::", "__").to_ascii_lowercase()
}

fn group_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// Stable anchor used by the symbol index (includes line for disambiguation).
fn symbol_anchor(item: &DocItem) -> String {
    let name = if item.name.is_empty() {
        "anonymous"
    } else {
        &item.name
    };
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
pub fn md_heading_anchor(kind: &str, name: &str) -> String {
    let raw = format!("{kind} {name}");
    let lowered: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    lowered
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::super::lang::{DocLanguage, DocMeta};
    use super::*;

    #[test]
    fn anchor_simple() {
        assert_eq!(md_heading_anchor("fn", "factorial"), "fn-factorial");
    }

    #[test]
    fn anchor_camel() {
        assert_eq!(
            md_heading_anchor("class", "OrderStatistics"),
            "class-orderstatistics"
        );
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
        assert!(
            md.contains("<table style=\"border-collapse: collapse"),
            "{md}"
        );
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
            lang: DocLanguage::C,
            signature: "int add(int a, int b);".into(),
            meta: DocMeta::default(),
        };
        let mut by_file = BTreeMap::new();
        by_file.insert("src/math.c".to_string(), vec![&item]);

        let md = render_symbol_index(&by_file);

        assert!(md.contains("**Symbols**"), "{md}");
        assert!(md.contains("## Functions"), "{md}");
        assert!(
            md.contains("[fn `add`](src_math_c.md#fn-add-line-7)"),
            "{md}"
        );
    }

    #[test]
    fn ns_slug_converts_colons() {
        assert_eq!(ns_slug("stats::algo"), "stats__algo");
    }

    #[test]
    fn item_anchor_simple_uses_last_component() {
        let item = DocItem {
            name: "stats::mean".into(),
            kind: DocKind::Function,
            brief: String::new(),
            body: String::new(),
            tags: Vec::new(),
            file: Path::new("stats.h").to_path_buf(),
            line: 1,
            lang: DocLanguage::Cpp,
            signature: String::new(),
            meta: DocMeta::default(),
        };
        assert_eq!(item_anchor_simple(&item), "fn-mean");
    }

    #[test]
    fn relative_page_same_dir() {
        assert_eq!(
            relative_page("namespace/stats.md", "namespace/stats__algo.md"),
            "stats__algo.md"
        );
    }

    #[test]
    fn relative_page_cross_dir() {
        assert_eq!(
            relative_page("namespace/stats.md", "class/stats__orderstatistics.md"),
            "../class/stats__orderstatistics.md"
        );
    }

    #[test]
    fn relative_page_to_root() {
        assert_eq!(
            relative_page("namespace/stats.md", "index.md"),
            "../index.md"
        );
    }

    #[test]
    fn relative_page_from_group() {
        assert_eq!(
            relative_page("group/io.md", "namespace/fs.md"),
            "../namespace/fs.md"
        );
        assert_eq!(relative_page("group/io.md", "index.md"), "../index.md");
        assert_eq!(relative_page("group/io.md", "group/net.md"), "net.md");
    }

    // ── group_slug ────────────────────────────────────────────────────────────

    #[test]
    fn group_slug_alphanumeric() {
        assert_eq!(group_slug("io"), "io");
        assert_eq!(group_slug("core"), "core");
    }

    #[test]
    fn group_slug_spaces_become_underscores() {
        assert_eq!(group_slug("File IO"), "file_io");
        assert_eq!(group_slug("my group"), "my_group");
    }

    #[test]
    fn group_slug_special_chars_become_underscores() {
        assert_eq!(group_slug("io::fs"), "io_fs");
        assert_eq!(group_slug("my-group"), "my_group");
    }

    #[test]
    fn group_slug_deduplicates_underscores() {
        // Double separators collapse to one.
        assert_eq!(group_slug("a::b"), "a_b");
        assert_eq!(group_slug("a  b"), "a_b");
    }

    // ── group_items routing ───────────────────────────────────────────────────

    fn make_item(name: &str, kind: DocKind, lang: DocLanguage, group: Option<&str>) -> DocItem {
        DocItem {
            name: name.into(),
            kind,
            brief: "Brief.".into(),
            body: String::new(),
            tags: Vec::new(),
            file: Path::new("src/lib.c").to_path_buf(),
            line: 1,
            lang,
            signature: String::new(),
            meta: DocMeta {
                group: group.map(str::to_string),
                ..Default::default()
            },
        }
    }

    #[test]
    fn grouped_item_appears_in_groups_bucket() {
        let item = make_item("fopen", DocKind::Function, DocLanguage::C, Some("io"));
        let items = vec![item];
        let (groups, _) = group_items(&items, Path::new("."));
        assert!(
            groups.groups.contains_key("io"),
            "item with meta.group='io' should appear in groups['io']"
        );
        assert_eq!(groups.groups["io"].len(), 1);
    }

    #[test]
    fn grouped_cpp_item_also_appears_in_namespace() {
        // An item with both a namespace qualifier AND a group should land in both buckets.
        let mut item = make_item(
            "io::fopen",
            DocKind::Function,
            DocLanguage::Cpp,
            Some("posix"),
        );
        item.file = Path::new("src/io.cpp").to_path_buf();
        let items = vec![item];
        let (groups, _) = group_items(&items, Path::new("."));
        assert!(
            groups.groups.contains_key("posix"),
            "grouped item should appear in groups['posix']"
        );
        assert!(
            groups.namespaces.contains_key("io"),
            "namespace-qualified item should also appear in namespaces['io']"
        );
    }

    #[test]
    fn ungrouped_item_has_no_group_entry() {
        let item = make_item("malloc", DocKind::Function, DocLanguage::C, None);
        let items = vec![item];
        let (groups, _) = group_items(&items, Path::new("."));
        assert!(
            groups.groups.is_empty(),
            "ungrouped item should not produce a groups entry"
        );
    }

    #[test]
    fn rust_scoped_item_routes_to_namespace_page() {
        let item = make_item("math::add", DocKind::Function, DocLanguage::Rust, None);
        let items = vec![item];
        let (groups, _) = group_items(&items, Path::new("."));
        assert!(
            groups.namespaces.contains_key("math"),
            "Rust item with '::' should route to namespaces; got namespaces: {:?}",
            groups.namespaces.keys().collect::<Vec<_>>()
        );
        assert!(
            groups.files.is_empty(),
            "Rust scoped item should not fall through to files"
        );
    }

    #[test]
    fn rust_unscoped_item_routes_to_file_page() {
        let item = make_item("free_fn", DocKind::Function, DocLanguage::Rust, None);
        let items = vec![item];
        let (groups, _) = group_items(&items, Path::new("."));
        assert!(groups.namespaces.is_empty());
        assert!(
            !groups.files.is_empty(),
            "unscoped Rust item should fall through to files"
        );
    }

    // ── render_group_page ─────────────────────────────────────────────────────

    #[test]
    fn group_page_contains_group_name() {
        let item = make_item("fopen", DocKind::Function, DocLanguage::C, Some("io"));
        let dummy_sym = SymbolIndex {
            map: HashMap::new(),
        };
        let md = render_group_page("io", &[&item], &dummy_sym, "group/io.md");
        assert!(md.contains("group `io`"), "{md}");
    }

    #[test]
    fn group_page_lists_function() {
        let item = make_item("fopen", DocKind::Function, DocLanguage::C, Some("io"));
        let dummy_sym = SymbolIndex {
            map: HashMap::new(),
        };
        let md = render_group_page("io", &[&item], &dummy_sym, "group/io.md");
        assert!(md.contains("## Functions"), "{md}");
        assert!(md.contains("fopen"), "{md}");
    }

    #[test]
    fn group_page_lists_type() {
        let item = make_item("FILE", DocKind::Struct, DocLanguage::C, Some("io"));
        let dummy_sym = SymbolIndex {
            map: HashMap::new(),
        };
        let md = render_group_page("io", &[&item], &dummy_sym, "group/io.md");
        assert!(md.contains("## Types"), "{md}");
        assert!(md.contains("FILE"), "{md}");
    }

    #[test]
    fn group_page_has_back_link() {
        let item = make_item("f", DocKind::Function, DocLanguage::C, Some("g"));
        let dummy_sym = SymbolIndex {
            map: HashMap::new(),
        };
        let md = render_group_page("g", &[&item], &dummy_sym, "group/g.md");
        assert!(
            md.contains("../index.md"),
            "group page should link back to index: {md}"
        );
    }
}
