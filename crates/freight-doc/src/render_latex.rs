use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::extract::{DocItem, DocSet, DocTag, TagKind};
use super::markdown::{tex_escape, to_latex};

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
    compile_pdf(&out_dir.join("docs.tex"), out_dir)
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

    doc.push_str(PREAMBLE);
    let _ = writeln!(doc, r"\title{{Project Documentation}}");
    let _ = writeln!(doc, r"\date{{\today}}");
    let _ = writeln!(doc, r"\begin{{document}}");
    let _ = writeln!(doc, r"\maketitle");
    let _ = writeln!(doc, r"\begin{{abstract}}");
    let _ = writeln!(doc, "{total} documented items across {} source files.", by_file.len());
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
    let _ = writeln!(doc, r"\section{{\texttt{{{}}}}}", tex_escape(rel));
    let _ = writeln!(doc, r"\textbf{{Language:}} {lang}\\[4pt]");
    let _ = writeln!(doc);
    for item in items {
        render_item(doc, item);
    }
}

fn render_item(doc: &mut String, item: &DocItem) {
    let display = if item.name.is_empty() {
        "(anonymous)".to_string()
    } else {
        format!(r"\texttt{{{}}}", tex_escape(&item.name))
    };

    let _ = writeln!(
        doc,
        r"\subsection*{{{kind} {display}}}",
        kind = tex_escape(item.kind.label()),
    );

    // Phantom label for hyperref cross-references
    if !item.name.is_empty() {
        let label = label_name(&item.name, item.line);
        let _ = writeln!(doc, r"\phantomsection\label{{{label}}}");
    }

    if item.line > 0 {
        let _ = writeln!(doc, r"\textcolor{{gray}}{{\small line {}}}\par", item.line);
    }

    if !item.signature.is_empty() {
        let sig = item.signature.trim_end_matches('{').trim();
        let _ = writeln!(doc, "\\begin{{verbatim}}\n{sig}\n\\end{{verbatim}}");
    }

    if !item.brief.is_empty() {
        // Brief rendered as bold paragraph — markdown + math supported
        let rendered = to_latex(&item.brief);
        let _ = writeln!(doc, r"\textbf{{{rendered}}}\\[2pt]");
    }
    if !item.body.is_empty() {
        let rendered = to_latex(&item.body);
        let _ = writeln!(doc, r"{rendered}\\[2pt]");
    }

    // Parameters table
    let params: Vec<&DocTag> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    if !params.is_empty() {
        doc.push_str("\\begin{tabular}{@{} l p{0.7\\linewidth} @{}}\n");
        doc.push_str("\\toprule\n");
        doc.push_str("\\textbf{Parameter} & \\textbf{Description} \\\\ \\midrule\n");
        for tag in &params {
            let pname = tag.name.as_deref().unwrap_or("—");
            let desc = to_latex(&tag.text);
            let _ = writeln!(doc, r"\texttt{{{}}} & {} \\", tex_escape(pname), desc);
        }
        doc.push_str("\\bottomrule\n");
        doc.push_str("\\end{tabular}\\\\[6pt]\n");
    }

    // Non-param tags
    for tag in &item.tags {
        if matches!(tag.kind, TagKind::Param | TagKind::Brief) { continue; }
        let _ = writeln!(
            doc,
            r"\textbf{{{}:}} {}\\",
            tex_escape(tag.kind.label()),
            to_latex(&tag.text),
        );
    }

    let _ = writeln!(doc, r"\medskip\hrule\medskip");
    let _ = writeln!(doc);
}

// ── PDF compilation ───────────────────────────────────────────────────────────

fn compile_pdf(tex_path: &Path, out_dir: &Path) -> std::io::Result<()> {
    for compiler in &["xelatex", "pdflatex"] {
        if which(compiler) {
            // Run twice so hyperref / ToC cross-references resolve
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
\usepackage[normalem]{ulem}
\usepackage[colorlinks=true,linkcolor=blue,urlcolor=blue]{hyperref}
\usepackage{booktabs}
\usepackage{longtable}
\usepackage{xcolor}
\usepackage{geometry}
\usepackage{parskip}
\geometry{margin=2.5cm}
\setlength{\parindent}{0pt}

";
