pub mod lang;
pub mod markdown;
pub mod render_md;

// ── Public re-exports ────────────────────────────────────────────────────────

pub use lang::{
    extract_dir, extract_dir_with, extract_file, Access, DocExtractor, DocItem, DocKind,
    DocLanguage, DocMeta, DocSet, DocTag, ExtractorRegistry, TagKind,
};

// ── Entry points ─────────────────────────────────────────────────────────────

use std::path::Path;

/// Render `set` into `out_dir` as GitHub-Flavored Markdown.
pub fn render(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    render_md::render_markdown(set, out_dir)
}

/// Serialize a slice of `DocItem`s to a msgpack byte vector.
pub fn to_msgpack(items: &[DocItem]) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    rmp_serde::to_vec_named(items)
}

/// Deserialize a msgpack byte slice into a `Vec<DocItem>`.
pub fn from_msgpack(bytes: &[u8]) -> Result<Vec<DocItem>, rmp_serde::decode::Error> {
    rmp_serde::from_slice(bytes)
}

/// Resolve `@see` / `\see` / "See also" references in `item` to their target
/// [`DocItem`]s within `set`.
pub fn resolve_refs<'a>(item: &DocItem, set: &'a DocSet) -> Vec<&'a DocItem> {
    item.tags
        .iter()
        .filter(|t| t.kind == TagKind::See)
        .flat_map(|t| resolve_name(t.text.trim(), &set.items))
        .collect()
}

fn resolve_name<'a>(raw: &str, items: &'a [DocItem]) -> Vec<&'a DocItem> {
    let name = raw.trim_matches(|c| matches!(c, '[' | '`' | ']'));
    if name.is_empty() {
        return vec![];
    }
    if let Some(item) = items.iter().find(|i| i.name == name) {
        return vec![item];
    }
    let cc = format!("::{name}");
    let dot = format!(".{name}");
    if let Some(item) = items
        .iter()
        .find(|i| i.name.ends_with(&cc) || i.name.ends_with(&dot))
    {
        return vec![item];
    }
    let lower = name.to_ascii_lowercase();
    items
        .iter()
        .filter(|i| i.name.to_ascii_lowercase().contains(&lower))
        .collect()
}
