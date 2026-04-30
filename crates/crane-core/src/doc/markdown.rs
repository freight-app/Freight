//! Markdown-aware rendering helpers for doc comments.
//!
//! Doc comments can contain both Markdown formatting (bold, italic, code spans,
//! lists, tables, links, headings) and LaTeX math (`$...$`, `$$...$$`,
//! `\(...\)`, `\[...\]`).  Naively passing such text through a Markdown parser
//! would corrupt the math — e.g. `$a_i$` loses its subscript when `_` triggers
//! emphasis.
//!
//! The strategy used here:
//! 1. **`protect_math`** — scan the raw text and replace every math region with
//!    an opaque ASCII placeholder (`@CRANEMATHn@`).  The original delimiters
//!    and content are stored in a side-list.
//! 2. Run the placeholder-substituted text through the target renderer
//!    (pulldown-cmark for HTML, a custom event loop for LaTeX).
//! 3. **`restore_math`** — replace placeholders back with the original math.
//!    Because the placeholder contains only `@`, alphanumerics, and nothing
//!    that HTML-escaping or LaTeX-escaping would touch, it survives both passes
//!    intact.

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

// ── Math protection ───────────────────────────────────────────────────────────

pub struct MathRegion {
    pub placeholder: String,
    pub raw: String, // original text including delimiters
}

/// Extract all math regions from `text`, replacing each with `@CRANEMATHn@`.
///
/// Recognised forms (in priority order):
/// - `$$...$$`   — display math (may span multiple lines)
/// - `\[...\]`   — display math (may span multiple lines)
/// - `\(...\)`   — inline math (single line)
/// - `$...$`     — inline math; opening `$` must not be followed by whitespace
///                 and the region must close on the same line, to avoid
///                 treating currency notation like `$5` as math.
pub fn protect_math(text: &str) -> (String, Vec<MathRegion>) {
    let mut regions: Vec<MathRegion> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        macro_rules! next2 {
            ($a:expr, $b:expr) => {
                chars.get(i) == Some(&$a) && chars.get(i + 1) == Some(&$b)
            };
        }

        // $$...$$  display math
        if next2!('$', '$') {
            let start = i;
            i += 2;
            while i + 1 < n && !next2!('$', '$') { i += 1; }
            let end = if i + 1 < n { i + 2 } else { n };
            push_region(&mut out, &mut regions, &chars[start..end]);
            i = end;

        // \[...\]  display math
        } else if next2!('\\', '[') {
            let start = i;
            i += 2;
            while i + 1 < n && !next2!('\\', ']') { i += 1; }
            let end = if i + 1 < n { i + 2 } else { n };
            push_region(&mut out, &mut regions, &chars[start..end]);
            i = end;

        // \(...\)  inline math
        } else if next2!('\\', '(') {
            let start = i;
            i += 2;
            while i + 1 < n && !next2!('\\', ')') && chars[i] != '\n' { i += 1; }
            let end = if i + 1 < n { i + 2 } else { n };
            push_region(&mut out, &mut regions, &chars[start..end]);
            i = end;

        // $...$  inline math — only when opening $ is not followed by whitespace
        // and the closing $ is on the same line.
        } else if chars[i] == '$'
            && chars.get(i + 1).map(|c| !c.is_whitespace() && *c != '$').unwrap_or(false)
        {
            let start = i;
            i += 1;
            // Collect until closing $ or end-of-line
            while i < n && chars[i] != '$' && chars[i] != '\n' { i += 1; }
            if i < n && chars[i] == '$' {
                let end = i + 1;
                push_region(&mut out, &mut regions, &chars[start..end]);
                i = end;
            } else {
                // No closing $ on this line — not math, emit literally
                let frag: String = chars[start..i].iter().collect();
                out.push_str(&frag);
            }

        } else {
            out.push(chars[i]);
            i += 1;
        }
    }

    (out, regions)
}

fn push_region(out: &mut String, regions: &mut Vec<MathRegion>, chars: &[char]) {
    let raw: String = chars.iter().collect();
    let placeholder = format!("@CRANEMATH{}@", regions.len());
    regions.push(MathRegion { placeholder: placeholder.clone(), raw });
    out.push_str(&placeholder);
}

/// Replace every placeholder back with its original math text.
pub fn restore_math(mut text: String, regions: &[MathRegion]) -> String {
    for r in regions {
        text = text.replace(&r.placeholder, &r.raw);
    }
    text
}

// ── Markdown → HTML ───────────────────────────────────────────────────────────

/// Render a markdown + math string to an HTML fragment.
///
/// Math regions are protected before passing to pulldown-cmark and restored
/// afterwards.  MathJax (loaded by the page template) renders them in the
/// browser.
pub fn to_html(text: &str) -> String {
    let (protected, regions) = protect_math(text);
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let parser = Parser::new_ext(&protected, opts);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    restore_math(html, &regions)
}

// ── Markdown → LaTeX ─────────────────────────────────────────────────────────

/// Render a markdown + math string to a LaTeX fragment.
///
/// Math regions are protected, then the surrounding Markdown is converted via
/// pulldown-cmark events.  Placeholders are replaced last so math content is
/// never LaTeX-escaped.
pub fn to_latex(text: &str) -> String {
    let (protected, regions) = protect_math(text);
    let result = md_events_to_latex(&protected);
    restore_math(result, &regions)
}

fn md_events_to_latex(text: &str) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(text, opts);

    let mut out = String::new();
    let mut in_code_block = false;
    let mut current_col: usize = 0;
    let mut in_table_head = false;

    for event in parser {
        match event {
            // ── Block structure ───────────────────────────────────────────
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => out.push_str("\n\n"),

            Event::Start(Tag::Heading { level, .. }) => match level {
                HeadingLevel::H1 | HeadingLevel::H2 => out.push_str(r"\paragraph{"),
                _ => out.push_str(r"\subparagraph{"),
            },
            Event::End(TagEnd::Heading(_)) => out.push_str("}\n"),

            Event::Start(Tag::BlockQuote(_)) => out.push_str("\n\\begin{quote}\n"),
            Event::End(TagEnd::BlockQuote(_)) => out.push_str("\\end{quote}\n"),

            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                out.push_str("\n\\begin{verbatim}\n");
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                out.push_str("\\end{verbatim}\n");
            }

            // ── Lists ─────────────────────────────────────────────────────
            Event::Start(Tag::List(None))    => out.push_str("\n\\begin{itemize}\n"),
            Event::Start(Tag::List(Some(_))) => out.push_str("\n\\begin{enumerate}\n"),
            Event::End(TagEnd::List(false))  => out.push_str("\\end{itemize}\n"),
            Event::End(TagEnd::List(true))   => out.push_str("\\end{enumerate}\n"),
            Event::Start(Tag::Item) => out.push_str("\\item "),
            Event::End(TagEnd::Item) => out.push('\n'),

            // ── Tables ────────────────────────────────────────────────────
            Event::Start(Tag::Table(aligns)) => {
                current_col = 0;
                let spec: String = aligns.iter().map(|a| match a {
                    Alignment::Left | Alignment::None => 'l',
                    Alignment::Center => 'c',
                    Alignment::Right  => 'r',
                }).collect();
                out.push_str(&format!(
                    "\n\\begin{{tabular}}{{@{{}} {spec} @{{}}}}\n\\toprule\n"
                ));
            }
            Event::End(TagEnd::Table) => out.push_str("\\bottomrule\n\\end{tabular}\n"),

            Event::Start(Tag::TableHead) => { in_table_head = true; current_col = 0; }
            Event::End(TagEnd::TableHead) => {
                in_table_head = false;
                out.push_str("\\\\\n\\midrule\n");
                current_col = 0;
            }
            Event::Start(Tag::TableRow) => current_col = 0,
            Event::End(TagEnd::TableRow) => {
                if !in_table_head { out.push_str("\\\\\n"); }
                current_col = 0;
            }
            Event::Start(Tag::TableCell) => {
                if current_col > 0 { out.push_str(" & "); }
            }
            Event::End(TagEnd::TableCell) => current_col += 1,

            // ── Inline formatting ─────────────────────────────────────────
            Event::Start(Tag::Strong) => out.push_str(r"\textbf{"),
            Event::End(TagEnd::Strong) => out.push('}'),

            Event::Start(Tag::Emphasis) => out.push_str(r"\emph{"),
            Event::End(TagEnd::Emphasis) => out.push('}'),

            Event::Start(Tag::Strikethrough) => out.push_str(r"\sout{"),
            Event::End(TagEnd::Strikethrough) => out.push('}'),

            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str(&format!(r"\href{{{}}}{{", latex_escape_url(&dest_url)));
            }
            Event::End(TagEnd::Link) => out.push('}'),

            // Images are unusual in doc comments; emit descriptive fallback
            Event::Start(Tag::Image { title, .. }) => {
                out.push_str(&format!("[image: {}]", tex_escape(title.as_ref())));
            }
            Event::End(TagEnd::Image) => {}

            // ── Leaf events ───────────────────────────────────────────────
            Event::Text(t) => {
                if in_code_block {
                    out.push_str(t.as_ref()); // verbatim environment — no escaping
                } else {
                    out.push_str(&tex_escape(t.as_ref()));
                }
            }
            Event::Code(c) => {
                out.push_str(&format!(r"\texttt{{{}}}", tex_escape(c.as_ref())));
            }
            Event::HardBreak => out.push_str("\\\\\n"),
            Event::SoftBreak => out.push(' '),
            Event::Rule => out.push_str("\n\\hrule\n"),
            Event::Html(h) => out.push_str(&tex_escape(h.as_ref())),

            _ => {}
        }
    }

    out
}

// ── LaTeX escaping helpers ────────────────────────────────────────────────────

/// Escape LaTeX special characters in body text.
///
/// **Does not touch math regions** — those are protected as placeholders before
/// this function sees the text, so `@CRANEMATHn@` passes through unchanged
/// (`@` is not a LaTeX special character in text mode).
pub fn tex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push_str(match c {
            '&'  => r"\&",
            '%'  => r"\%",
            '#'  => r"\#",
            '_'  => r"\_",
            '{'  => r"\{",
            '}'  => r"\}",
            '~'  => r"\textasciitilde{}",
            '^'  => r"\textasciicircum{}",
            '\\' => r"\textbackslash{}",
            '<'  => r"\textless{}",
            '>'  => r"\textgreater{}",
            _    => { out.push(c); continue; }
        });
    }
    out
}

fn latex_escape_url(url: &str) -> String {
    // In \href{}, only % and # need escaping; others are fine
    url.replace('%', r"\%").replace('#', r"\#")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── protect_math ──────────────────────────────────────────────────────────

    #[test]
    fn protect_inline_dollar() {
        let (out, regions) = protect_math("Use $x^2$ here");
        assert_eq!(out, "Use @CRANEMATH0@ here");
        assert_eq!(regions[0].raw, "$x^2$");
    }

    #[test]
    fn protect_display_dollar() {
        let (out, regions) = protect_math("See:\n$$E = mc^2$$\nEnd");
        assert_eq!(out, "See:\n@CRANEMATH0@\nEnd");
        assert_eq!(regions[0].raw, "$$E = mc^2$$");
    }

    #[test]
    fn protect_backslash_bracket() {
        let (out, regions) = protect_math(r"\[x + y\]");
        assert_eq!(out, "@CRANEMATH0@");
        assert_eq!(regions[0].raw, r"\[x + y\]");
    }

    #[test]
    fn protect_backslash_paren() {
        let (out, regions) = protect_math(r"See \(f(x)\)");
        assert_eq!(out, "See @CRANEMATH0@");
        assert_eq!(regions[0].raw, r"\(f(x)\)");
    }

    #[test]
    fn protect_no_math_for_currency() {
        // "$5 costs" — $ followed by digit, not whitespace, but no closing $
        // on the same line => not treated as math
        let (out, regions) = protect_math("costs $5 today");
        // $ followed by '5' which is not whitespace — we try to find a closing $
        // "5 today" has no $, so we fall back and emit the $ literally
        assert_eq!(regions.len(), 0, "should not find math region");
        assert!(out.contains('$'));
    }

    #[test]
    fn protect_multiple_regions() {
        let (out, regions) = protect_math("$a$ and $$b$$");
        assert_eq!(regions.len(), 2);
        assert!(out.contains("@CRANEMATH0@"));
        assert!(out.contains("@CRANEMATH1@"));
    }

    // ── restore_math ─────────────────────────────────────────────────────────

    #[test]
    fn restore_roundtrip() {
        let text = "The formula $x^2 + y^2 = r^2$ is Pythagoras.";
        let (protected, regions) = protect_math(text);
        let restored = restore_math(protected, &regions);
        assert_eq!(restored, text);
    }

    // ── to_html ───────────────────────────────────────────────────────────────

    #[test]
    fn html_bold_and_math() {
        let html = to_html("**Bold** and $x^2$");
        assert!(html.contains("<strong>Bold</strong>"), "{html}");
        assert!(html.contains("$x^2$"), "{html}");
        // Math must not be HTML-escaped
        assert!(!html.contains("&amp;"), "{html}");
    }

    #[test]
    fn html_code_span() {
        let html = to_html("Call `factorial(n)` to compute");
        assert!(html.contains("<code>factorial(n)</code>"), "{html}");
    }

    #[test]
    fn html_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let html = to_html(md);
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<th>"), "{html}");
    }

    #[test]
    fn html_display_math_preserved() {
        let html = to_html("$$\\sum_{i=0}^{n} x_i$$");
        assert!(html.contains("$$\\sum_{i=0}^{n} x_i$$"), "{html}");
    }

    // ── to_latex ──────────────────────────────────────────────────────────────

    #[test]
    fn latex_bold() {
        let tex = to_latex("**strong** text");
        assert!(tex.contains(r"\textbf{strong}"), "{tex}");
    }

    #[test]
    fn latex_code_span() {
        let tex = to_latex("Use `factorial`");
        assert!(tex.contains(r"\texttt{factorial}"), "{tex}");
    }

    #[test]
    fn latex_math_not_escaped() {
        // Underscores inside math must not become \_
        let tex = to_latex("The norm $\\|v\\|_2$");
        assert!(tex.contains("$\\|v\\|_2$"), "{tex}");
        // Underscores in text outside math must be escaped
        let tex2 = to_latex("my_var is a variable");
        assert!(tex2.contains(r"my\_var"), "{tex2}");
    }

    #[test]
    fn latex_display_math_preserved() {
        let tex = to_latex("$$x^2 + y^2$$");
        assert!(tex.contains("$$x^2 + y^2$$"), "{tex}");
    }

    #[test]
    fn latex_list() {
        let tex = to_latex("- item one\n- item two");
        assert!(tex.contains(r"\begin{itemize}"), "{tex}");
        assert!(tex.contains(r"\item"), "{tex}");
    }

    // ── tex_escape ────────────────────────────────────────────────────────────

    #[test]
    fn escape_special() {
        assert_eq!(tex_escape("a & b"), r"a \& b");
        assert_eq!(tex_escape("100%"), r"100\%");
        assert_eq!(tex_escape("a_b"), r"a\_b");
        assert_eq!(tex_escape("{x}"), r"\{x\}");
    }

    #[test]
    fn escape_unicode_passthrough() {
        // Unicode chars that are not LaTeX specials must pass through
        assert_eq!(tex_escape("α β γ"), "α β γ");
        assert_eq!(tex_escape("u · v"), "u · v");
    }
}
