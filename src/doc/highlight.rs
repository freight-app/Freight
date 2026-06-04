use crate::doc::DocLanguage;
use ratatui::text::{Line, Span};

fn lang_fence(lang: &DocLanguage) -> &'static str {
    match lang {
        DocLanguage::C       => "c",
        DocLanguage::Cpp     => "cpp",
        DocLanguage::Rust    => "rust",
        DocLanguage::D       => "d",
        DocLanguage::Ada     => "ada",
        DocLanguage::Fortran => "fortran",
        DocLanguage::Zig     => "zig",
        DocLanguage::Unknown => "",
    }
}

/// Highlight a function signature using tui-markdown's syntect backend.
///
/// Wraps `sig` in a fenced code block, renders it with `tui_markdown`, and
/// returns the spans of the first non-empty line so callers can embed them
/// directly into a `Line`.
pub fn highlight_signature(sig: &str, lang: &DocLanguage) -> Vec<Span<'static>> {
    let fence = lang_fence(lang);
    let md = format!("```{fence}\n{sig}\n```\n");
    let text = tui_markdown::from_str(&md);
    text.lines
        .into_iter()
        .find(|l| !l.spans.is_empty() && l.spans.iter().any(|s| !s.content.trim().is_empty()))
        .map(|l| l.spans.into_iter()
            .map(|s| Span::styled(s.content.into_owned(), s.style))
            .collect())
        .unwrap_or_else(|| vec![Span::raw(sig.to_string())])
}
