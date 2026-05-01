use std::path::Path;

use serde::Serialize;

use super::extract::{DocSet, DocTag, TagKind, DocLanguage, DocKind};

// ── Serialisable mirror types ─────────────────────────────────────────────────

#[derive(Serialize)]
struct JsonDoc<'a> {
    items: Vec<JsonItem<'a>>,
}

#[derive(Serialize)]
struct JsonItem<'a> {
    file:      String,
    line:      usize,
    lang:      &'static str,
    kind:      &'static str,
    name:      &'a str,
    signature: &'a str,
    brief:     &'a str,
    body:      &'a str,
    tags:      Vec<JsonTag<'a>>,
}

#[derive(Serialize)]
struct JsonTag<'a> {
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    text: &'a str,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Write `docs.json` to `out_dir`.
pub fn render_json(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;

    let items: Vec<JsonItem> = set.items.iter().map(|item| {
        let file = item.file
            .strip_prefix(&set.source_root)
            .unwrap_or(&item.file)
            .to_string_lossy()
            .into_owned();

        JsonItem {
            file,
            line:      item.line,
            lang:      lang_str(&item.lang),
            kind:      kind_str(&item.kind),
            name:      &item.name,
            signature: &item.signature,
            brief:     &item.brief,
            body:      &item.body,
            tags:      item.tags.iter().map(tag_json).collect(),
        }
    }).collect();

    let doc = JsonDoc { items };
    let json = serde_json::to_string_pretty(&doc)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    std::fs::write(out_dir.join("docs.json"), json)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn tag_json(tag: &DocTag) -> JsonTag<'_> {
    JsonTag {
        kind: tag_kind_str(&tag.kind),
        name: tag.name.as_deref(),
        text: &tag.text,
    }
}

fn lang_str(lang: &DocLanguage) -> &'static str {
    match lang {
        DocLanguage::C       => "c",
        DocLanguage::Cpp     => "cpp",
        DocLanguage::Rust    => "rust",
        DocLanguage::Fortran => "fortran",
        DocLanguage::D       => "d",
        DocLanguage::Ada     => "ada",
        DocLanguage::Unknown => "unknown",
    }
}

fn kind_str(kind: &DocKind) -> &'static str {
    match kind {
        DocKind::Function   => "function",
        DocKind::Struct     => "struct",
        DocKind::Class      => "class",
        DocKind::Enum       => "enum",
        DocKind::Typedef    => "typedef",
        DocKind::Variable   => "variable",
        DocKind::Macro      => "macro",
        DocKind::Module     => "module",
        DocKind::Subroutine => "subroutine",
        DocKind::Interface  => "interface",
        DocKind::Unknown    => "unknown",
    }
}

fn tag_kind_str(kind: &TagKind) -> &'static str {
    match kind {
        TagKind::Brief      => "brief",
        TagKind::Param      => "param",
        TagKind::Return     => "returns",
        TagKind::Note       => "note",
        TagKind::See        => "see",
        TagKind::Since      => "since",
        TagKind::Deprecated => "deprecated",
        TagKind::Example    => "example",
        TagKind::Warning    => "warning",
        TagKind::Other(_)   => "other",
    }
}
