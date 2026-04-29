/// Documentation extraction and rendering.
///
/// Supports C/C++ (Doxygen `/** */`, `/*! */`, `///`), Rust (`///`, `/** */`),
/// Fortran (`!>` / `!!`), D (`/++`, `/**`, `///`) and Ada (`--!` / `---`).
pub mod extract;
pub mod markdown;
pub mod render;
pub mod render_md;
pub mod render_latex;

use std::path::Path;
use extract::DocSet;

/// Output format for the documentation renderer.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    /// Self-contained HTML with MathJax support.
    Html,
    /// GitHub-Flavored Markdown with cross-document links.
    Markdown,
    /// LaTeX source file (`docs.tex`) only.
    Latex,
    /// LaTeX source + compiled PDF (requires xelatex or pdflatex on PATH).
    Pdf,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "html"               => Some(Self::Html),
            "md" | "markdown"   => Some(Self::Markdown),
            "latex" | "tex"     => Some(Self::Latex),
            "pdf"               => Some(Self::Pdf),
            _ => None,
        }
    }
}

/// Render `set` into `out_dir` in the requested format.
pub fn render(set: &DocSet, out_dir: &Path, format: &OutputFormat) -> std::io::Result<()> {
    match format {
        OutputFormat::Html     => render::render_html(set, out_dir),
        OutputFormat::Markdown => render_md::render_markdown(set, out_dir),
        OutputFormat::Latex    => render_latex::render_latex(set, out_dir),
        OutputFormat::Pdf      => render_latex::render_pdf(set, out_dir),
    }
}
