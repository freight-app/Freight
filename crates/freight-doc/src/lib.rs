/// Documentation extraction and rendering.
///
/// Supports C/C++ (Doxygen `/** */`, `/*! */`, `///`), Rust (`///`, `/** */`),
/// Fortran (`!>` / `!!`), D (`/++`, `/**`, `///`) and Ada (`--!` / `---`).
pub mod extract;
pub mod markdown;
pub mod render_json;
pub mod render_md;

use std::path::Path;
use extract::DocSet;

/// Output format for the documentation renderer.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    /// GitHub-Flavored Markdown with cross-document links.
    Markdown,
    /// Single `docs.json` — easy to consume from a website or tooling.
    Json,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "json"            => Some(Self::Json),
            _ => None,
        }
    }
}

/// Render `set` into `out_dir` in the requested format.
pub fn render(set: &DocSet, out_dir: &Path, format: &OutputFormat) -> std::io::Result<()> {
    match format {
        OutputFormat::Markdown => render_md::render_markdown(set, out_dir),
        OutputFormat::Json     => render_json::render_json(set, out_dir),
    }
}
