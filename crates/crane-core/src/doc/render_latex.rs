use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::extract::{DocItem, DocSet, DocTag, TagKind};

// ── Public entry points ───────────────────────────────────────────────────────

/// Write `docs.tex` to `out_dir` without compiling.
pub fn render_latex(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let tex = build_document(set);
    std::fs::write(out_dir.join("docs.tex"), tex)
}

/// Write `docs.tex` and compile to `docs.pdf` using xelatex or pdflatex.
///
/// The `.tex` file is always written. If no LaTeX compiler is found the
/// function returns an error; the caller should surface a helpful message.
pub fn render_pdf(set: &DocSet, out_dir: &Path) -> std::io::Result<()> {
    render_latex(set, out_dir)?;
    let tex_path = out_dir.join("docs.tex");
    compile_pdf(&tex_path, out_dir)
}

// ── LaTeX document builder ────────────────────────────────────────────────────

fn build_document(set: &DocSet) -> String {
    let mut by_file: BTreeMap<String, Vec<&DocItem>> = BTreeMap::new();
    for item in &set.items {
        let rel = item.file
            .strip_prefix(&set.source_root)
            .unwrap_or(&item.file)
            .to_string_lossy()
            .into_owned();
        by_file.entry(rel).or_default().push(item);
    }

    let total: usize = by_file.values().map(|v| v.len()).sum();

    let mut doc = String::new();

    // Preamble
    doc.push_str(PREAMBLE);

    // Title block
    let _ = writeln!(doc, r"\title{{Project Documentation}}");
    let _ = writeln!(doc, r"\date{{\today}}");
    let _ = writeln!(doc, r"\begin{{document}}");
    let _ = writeln!(doc, r"\maketitle");
    let _ = writeln!(doc, r"\begin{{abstract}}");
    let _ = writeln!(
        doc,
        r"{total} documented items across {} source files.",
        by_file.len()
    );
    let _ = writeln!(doc, r"\end{{abstract}}");
    let _ = writeln!(doc, r"\tableofcontents");
    let _ = writeln!(doc, r"\newpage");
    let _ = writeln!(doc);

    for (rel, items) in &by_file {
        render_file_section(&mut doc, rel, items);
    }

    doc.push_str(r"\end{document}");
    doc.push('\n');
    doc
}

fn render_file_section(doc: &mut String, rel: &str, items: &[&DocItem]) {
    let lang = items.first().map(|i| i.lang.label()).unwrap_or("Unknown");
    let _ = writeln!(
        doc,
        r"\section{{\texttt{{{}}}}}",
        latex_escape(rel)
    );
    let _ = writeln!(
        doc,
        r"\textbf{{Language:}} {lang}\\[4pt]",
    );
    let _ = writeln!(doc);

    for item in items {
        render_item(doc, item);
    }
}

fn render_item(doc: &mut String, item: &DocItem) {
    let display = if item.name.is_empty() {
        "(anonymous)".to_string()
    } else {
        format!(r"\texttt{{{}}}", latex_escape(&item.name))
    };

    let _ = writeln!(
        doc,
        r"\subsection*{{{kind} {display}}}",
        kind = latex_escape(item.kind.label()),
    );

    // Phantom label for hyperref cross-references
    if !item.name.is_empty() {
        let label = label_name(&item.name, item.line);
        let _ = writeln!(doc, r"\phantomsection\label{{{label}}}");
    }

    if item.line > 0 {
        let _ = writeln!(doc, r"\textcolor{{gray}}{{\small line {}}}\par", item.line);
    }

    if !item.brief.is_empty() {
        let _ = writeln!(doc, r"\textbf{{{}}}\\[2pt]", latex_text(&item.brief));
    }
    if !item.body.is_empty() {
        let _ = writeln!(doc, r"{}\\[2pt]", latex_text(&item.body));
    }

    // Parameters table
    let params: Vec<&DocTag> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    if !params.is_empty() {
        doc.push_str(r"\begin{tabular}{@{} l p{0.7\linewidth} @{}}");
        doc.push('\n');
        doc.push_str(r"\toprule");
        doc.push('\n');
        doc.push_str(r"\textbf{Parameter} & \textbf{Description} \\ \midrule");
        doc.push('\n');
        for tag in &params {
            let pname = tag.name.as_deref().unwrap_or("—");
            let _ = writeln!(
                doc,
                r"\texttt{{{}}} & {} \\",
                latex_escape(pname),
                latex_text(&tag.text),
            );
        }
        doc.push_str(r"\bottomrule");
        doc.push('\n');
        doc.push_str(r"\end{tabular}\\[6pt]");
        doc.push('\n');
    }

    // Non-param tags
    for tag in &item.tags {
        if matches!(tag.kind, TagKind::Param | TagKind::Brief) { continue; }
        let _ = writeln!(
            doc,
            r"\textbf{{{}:}} {}\\",
            latex_escape(tag.kind.label()),
            latex_text(&tag.text),
        );
    }

    let _ = writeln!(doc, r"\medskip\hrule\medskip");
    let _ = writeln!(doc);
}

// ── PDF compilation ───────────────────────────────────────────────────────────

fn compile_pdf(tex_path: &Path, out_dir: &Path) -> std::io::Result<()> {
    // Run twice so hyperref and the table of contents resolve correctly
    for compiler in &["xelatex", "pdflatex"] {
        if which(compiler) {
            for _ in 0..2 {
                let status = std::process::Command::new(compiler)
                    .args(["-interaction=nonstopmode", "-output-directory"])
                    .arg(out_dir)
                    .arg(tex_path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()?;
                if !status.success() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("{compiler} exited with non-zero status — check docs.log"),
                    ));
                }
            }
            return Ok(());
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no LaTeX compiler found on PATH (tried xelatex, pdflatex)\n\
         Install TeX Live or MiKTeX, or use --format latex to get the .tex source",
    ))
}

fn which(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── LaTeX escaping ────────────────────────────────────────────────────────────

/// Escape text for LaTeX, preserving `$...$` and `$$...$$` math regions verbatim.
///
/// Iterates over Unicode scalar values so multi-byte UTF-8 characters (em-dash,
/// middle-dot, Greek letters, etc.) are never corrupted.
fn latex_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '$' {
            out.push_str(&latex_escape_char(c));
            continue;
        }

        if chars.peek() == Some(&'$') {
            // Display math $$...$$
            chars.next();
            out.push_str("$$");
            loop {
                match chars.next() {
                    None => break,
                    Some('$') if chars.peek() == Some(&'$') => {
                        chars.next();
                        out.push_str("$$");
                        break;
                    }
                    Some(mc) => out.push(mc),
                }
            }
        } else {
            // Inline math $...$
            out.push('$');
            loop {
                match chars.next() {
                    None => break,
                    Some('$') => { out.push('$'); break; }
                    Some(mc) => out.push(mc),
                }
            }
        }
    }
    out
}

/// Escape a single character for LaTeX body text (not math mode).
fn latex_escape_char(c: char) -> String {
    match c {
        '&'  => r"\&".into(),
        '%'  => r"\%".into(),
        '#'  => r"\#".into(),
        '_'  => r"\_".into(),
        '{'  => r"\{".into(),
        '}'  => r"\}".into(),
        '~'  => r"\textasciitilde{}".into(),
        '^'  => r"\textasciicircum{}".into(),
        '\\' => r"\textbackslash{}".into(),
        '<'  => r"\textless{}".into(),
        '>'  => r"\textgreater{}".into(),
        c    => c.to_string(),
    }
}

/// Escape text that should never contain math (identifiers, labels, paths).
fn latex_escape(s: &str) -> String {
    s.chars().map(latex_escape_char).collect()
}

fn label_name(name: &str, line: usize) -> String {
    let safe: String = name.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    format!("sym-{safe}-{line}")
}

// ── LaTeX preamble ────────────────────────────────────────────────────────────

const PREAMBLE: &str = r"\documentclass[11pt,a4paper]{article}
\usepackage[T1]{fontenc}
\usepackage[utf8]{inputenc}
\usepackage{amsmath,amssymb,amsthm}
\usepackage[colorlinks=true,linkcolor=blue,urlcolor=blue]{hyperref}
\usepackage{booktabs}
\usepackage{longtable}
\usepackage{xcolor}
\usepackage{geometry}
\usepackage{parskip}
\geometry{margin=2.5cm}
\setlength{\parindent}{0pt}

";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_special_chars() {
        assert_eq!(latex_escape("foo_bar"), r"foo\_bar");
        assert_eq!(latex_escape("A & B"), r"A \& B");
        assert_eq!(latex_escape("100%"), r"100\%");
    }

    #[test]
    fn math_passthrough_inline() {
        let s = "Euler's formula: $e^{i\\pi} + 1 = 0$";
        let out = latex_text(s);
        // Math region must pass through verbatim
        assert!(out.contains("$e^{i\\pi} + 1 = 0$"), "{out}");
        // Surrounding prose is preserved (apostrophe needs no escaping in LaTeX body)
        assert!(out.contains("Euler's formula"), "{out}");
    }

    #[test]
    fn math_passthrough_display() {
        let s = "The norm: $$\\|v\\|_2 = \\sqrt{\\sum x_i^2}$$";
        let out = latex_text(s);
        assert!(out.contains("$$"), "{out}");
        // Underscores inside math must NOT be escaped
        assert!(out.contains("x_i"), "{out}");
    }

    #[test]
    fn math_passthrough_display_outer_escape() {
        // Text before/after math should be escaped; math should not
        let s = "See formula_ref: $$x^2$$";
        let out = latex_text(s);
        assert!(out.contains(r"formula\_ref"), "{out}");
        assert!(out.contains("$$x^2$$"), "{out}");
    }
}
