/// libclang-backed C/C++ extractor.
///
/// Enabled by the `clang` feature.  When active, `extract_file` routes all
/// `.c`, `.h`, `.cpp`, `.hpp`, etc. files through this module instead of the
/// heuristic scanner.  The result is accurate: member functions, constructors,
/// destructors, operators, templates with full parameter lists, access
/// specifiers, and method qualifiers are all correctly extracted.
///
/// The `clang` feature uses the `runtime` loading strategy: libclang is opened
/// via `dlopen` / `LoadLibrary` at first use.  No libclang is required to
/// build the binary; if it is absent at runtime `Clang::new()` returns an
/// error and the extractor falls back to the heuristic scanner automatically.
use std::path::Path;

use crate::extract::{
    Access, DocItem, DocKind, DocLanguage, DocMeta,
};

/// Parse `path` via libclang and return documented items.
///
/// Only entities with non-empty attached doc comments are returned.
/// System-header entities are skipped.
pub fn extract_file_clang(path: &Path) -> Vec<DocItem> {
    use clang::{Clang, Index};

    let clang = match Clang::new() {
        Ok(c)  => c,
        Err(e) => {
            eprintln!("freight-doc: libclang unavailable ({e}); falling back to heuristic extractor");
            return crate::extract::extract_file_heuristic(path);
        }
    };

    let index = Index::new(&clang, false, false);

    let ext  = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let lang = crate::extract::lang_from_ext(ext);

    // `.h` is ambiguous — many C++ projects use it for C++ headers.  Parse
    // everything as C++ unless the extension is unambiguously C (`.c`).
    // C declarations are valid C++ for our purposes (we only need the decls).
    let lang_args: &[&str] = if ext == "c" {
        &["-std=c11", "-w", "-x", "c-header"]
    } else {
        &["-std=c++17", "-w", "-x", "c++-header"]
    };

    let tu = match index.parser(path).arguments(lang_args).parse() {
        Ok(tu) => tu,
        Err(e) => {
            eprintln!("freight-doc: clang parse error for {}: {e:?}", path.display());
            return Vec::new();
        }
    };

    let mut items = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Iterate TU top-level children directly instead of passing the TU entity
    // itself through visit_entity.  This avoids a version skew: the clang Rust
    // crate maps EntityKind::TranslationUnit to 300, but libclang ≥ 20 uses
    // CXCursor_TranslationUnit = 350, causing the TU entity to come back as
    // EntityKind::NotImplemented and preventing any recursion.
    for child in tu.get_entity().get_children() {
        visit_entity(child, &mut items, &mut seen, path, &lang);
    }

    // libclang does not associate doc comments with preprocessor macros.
    // Supplement with any Macro items found by the heuristic extractor.
    let heuristic = crate::extract::extract_file_heuristic(path);
    for item in heuristic {
        if matches!(item.kind, DocKind::Macro) && !seen.contains(&item.name) {
            seen.insert(item.name.clone());
            items.push(item);
        }
    }

    items
}

fn visit_entity(
    entity: clang::Entity<'_>,
    out:    &mut Vec<DocItem>,
    seen:   &mut std::collections::HashSet<String>,
    file:   &Path,
    lang:   &DocLanguage,
) {
    use clang::EntityKind;

    // Skip system headers.
    if entity.is_in_system_header() { return; }

    let kind = entity.get_kind();

    // Recurse into containers (namespaces, classes).
    let is_container = matches!(
        kind,
        EntityKind::Namespace
        | EntityKind::ClassDecl
        | EntityKind::ClassTemplate
        | EntityKind::StructDecl
    );

    // Map to our DocKind.
    let doc_kind: Option<DocKind> = match kind {
        EntityKind::FunctionDecl | EntityKind::FunctionTemplate => Some(DocKind::Function),
        EntityKind::Constructor                                  => Some(DocKind::Function),
        EntityKind::Destructor                                   => Some(DocKind::Function),
        EntityKind::Method                                       => Some(DocKind::Function),
        EntityKind::ConversionFunction                           => Some(DocKind::Function),
        EntityKind::ClassDecl | EntityKind::ClassTemplate        => Some(DocKind::Class),
        EntityKind::StructDecl                                   => Some(DocKind::Struct),
        EntityKind::EnumDecl                                     => Some(DocKind::Enum),
        EntityKind::TypedefDecl | EntityKind::TypeAliasDecl      => Some(DocKind::Typedef),
        EntityKind::VarDecl | EntityKind::FieldDecl              => Some(DocKind::Variable),
        _                                                        => None,
    };

    if let Some(dk) = doc_kind {
        // Only extract entities with a doc comment.
        if let Some(raw_comment) = entity.get_comment() {
            if !raw_comment.trim().is_empty() {
                let qname = qualified_name(&entity);

                if !qname.is_empty() && !seen.contains(&qname) {
                    seen.insert(qname.clone());

                    let meta = build_meta(&entity, kind);
                    let sig  = entity.get_display_name().unwrap_or_default();
                    let line = entity.get_location()
                        .and_then(|l| l.get_file_location().line.try_into().ok())
                        .unwrap_or(0);
                    let item = build_doc_item(raw_comment, qname, dk, file, line, lang.clone(), sig, meta);
                    if crate::extract::item_has_content(&item) {
                        out.push(item);
                    }
                }
            }
        }
    }

    if is_container {
        for child in entity.get_children() {
            visit_entity(child, out, seen, file, lang);
        }
    }
}

/// Build a fully-qualified `::` name by walking semantic parents.
///
/// Uses `get_semantic_parent() == None` as the TU sentinel, which is stable
/// across libclang versions regardless of CXCursor_TranslationUnit's numeric
/// value (300 in older versions, 350 in libclang ≥ 20).
fn qualified_name(entity: &clang::Entity<'_>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = entity.clone();
    loop {
        match cur.get_semantic_parent() {
            // No parent means this IS the TU — stop without adding its name.
            None => break,
            Some(parent) => {
                if let Some(n) = cur.get_name() {
                    if !n.is_empty() { parts.push(n); }
                }
                // If the parent itself has no parent it's the TU — stop after
                // adding cur's name (already done above).
                if parent.get_semantic_parent().is_none() { break; }
                cur = parent;
            }
        }
    }
    parts.reverse();
    parts.join("::")
}

/// Build `DocMeta` from libclang entity attributes.
fn build_meta(entity: &clang::Entity<'_>, kind: clang::EntityKind) -> DocMeta {
    use clang::{Accessibility, EntityKind};

    let access = entity.get_accessibility().map(|a| match a {
        Accessibility::Public    => Access::Public,
        Accessibility::Protected => Access::Protected,
        Accessibility::Private   => Access::Private,
    });

    let parent = entity.get_semantic_parent().and_then(|p| {
        matches!(p.get_kind(), EntityKind::ClassDecl | EntityKind::StructDecl | EntityKind::ClassTemplate)
            .then(|| p.get_name().unwrap_or_default())
            .filter(|n| !n.is_empty())
    });

    let template_params = entity.get_template()
        .map(|t| t.get_children())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            use clang::EntityKind as EK;
            matches!(c.get_kind(), EK::TemplateTypeParameter | EK::NonTypeTemplateParameter | EK::TemplateTemplateParameter)
                .then(|| c.get_display_name().unwrap_or_default())
                .filter(|s| !s.is_empty())
        })
        .collect();

    let mut attrs = Vec::new();
    match kind {
        EntityKind::Constructor        => attrs.push("constructor".into()),
        EntityKind::Destructor         => attrs.push("destructor".into()),
        EntityKind::ConversionFunction => attrs.push("operator".into()),
        _ => {}
    }
    if entity.get_name().map_or(false, |n| n.starts_with("operator")) {
        if !attrs.contains(&"operator".to_string()) { attrs.push("operator".into()); }
    }
    if entity.is_pure_virtual_method() { attrs.push("pure".into()); attrs.push("virtual".into()); }
    else if entity.is_virtual_method() { attrs.push("virtual".into()); }
    if entity.is_const_method()        { attrs.push("const".into()); }

    DocMeta { template_params, access, parent, attrs }
}

fn build_doc_item(
    raw:    String,
    name:   String,
    kind:   DocKind,
    file:   &Path,
    line:   usize,
    lang:   DocLanguage,
    sig:    String,
    meta:   DocMeta,
) -> DocItem {
    let lines: Vec<String> = strip_comment_markers(&raw);
    let mut item = crate::extract::build_item(lines, name, kind, file, line, lang, sig);
    item.meta = meta;
    item
}

fn strip_comment_markers(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("/**") || t.starts_with("/*!") {
            let inner = t[3..].trim_start_matches('*').trim();
            if !inner.is_empty() && inner != "/" { out.push(inner.to_string()); }
            continue;
        }
        if t == "*/" || t == "/*" { continue; }
        if let Some(r) = t.strip_prefix("* ") { out.push(r.to_string()); continue; }
        if t == "*" { out.push(String::new()); continue; }
        if let Some(r) = t.strip_prefix('*') { out.push(r.to_string()); continue; }
        if let Some(r) = t.strip_prefix("/// ") { out.push(r.to_string()); continue; }
        if t == "///" { out.push(String::new()); continue; }
        out.push(t.to_string());
    }
    out
}
