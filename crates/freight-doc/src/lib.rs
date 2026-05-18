/// Documentation extraction and rendering.
///
/// Supports C/C++ (Doxygen `/** */`, `/*! */`, `///`), Rust (`///`, `/** */`),
/// Fortran (`!>` / `!!`), D (`/++`, `/**`, `///`) and Ada (`--!` / `---`).
pub mod extract;
pub mod markdown;
pub mod render_md;
pub mod render_tui;
pub mod util;
#[cfg(feature = "clang")]
pub mod extract_clang;

use std::path::Path;
use extract::DocSet;

/// Render `set` into `out_dir` as GitHub-Flavored Markdown.
pub fn render(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    render_md::render_markdown(set, out_dir)
}
