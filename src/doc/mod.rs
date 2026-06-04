pub mod browser;
pub mod discover;
pub mod lang;
pub mod latex;
pub mod markdown;
pub mod render_md;
pub mod stdlib;

pub use browser::{browse, PackageDoc};
pub use discover::DocDependency;
pub use lang::{
    extract_dir, extract_dir_with, extract_file, Access, DocExtractor, DocItem, DocKind,
    DocLanguage, DocMeta, DocSet, DocTag, ExtractorRegistry, TagKind,
};
pub use stdlib::{collect_stdlib, StdlibMsg};

/// Serialize a slice of `DocItem`s to a msgpack byte vector.
pub fn to_msgpack(items: &[DocItem]) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    rmp_serde::to_vec_named(items)
}

/// Deserialize a msgpack byte slice into a `Vec<DocItem>`.
pub fn from_msgpack(bytes: &[u8]) -> Result<Vec<DocItem>, rmp_serde::decode::Error> {
    rmp_serde::from_slice(bytes)
}

/// Generate Markdown documentation for `set` into `out_dir`.
pub fn generate(set: DocSet, out_dir: &std::path::Path) -> anyhow::Result<()> {
    render_md::render_markdown(&set, out_dir).map_err(anyhow::Error::from)
}
