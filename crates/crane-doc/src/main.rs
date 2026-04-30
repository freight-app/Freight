use std::path::PathBuf;

use clap::Parser;
use crane_core::doc::extract::{extract_dir, DocSet};
use crane_core::doc::{render, OutputFormat};

/// Extract doc comments from source files and render a documentation site.
#[derive(Parser)]
#[command(
    name = "crane-doc",
    about = "Multi-language doc comment extractor — outputs HTML, Markdown, LaTeX, or PDF",
    long_about = "\
Scans source directories for documented items in C/C++ (Doxygen /** */, ///),
Rust (///), Fortran (!> / !!), D (/++), and Ada (--!) and renders them.

Output formats
  html      Self-contained HTML with MathJax (default)
  md        GitHub-Flavored Markdown with cross-document links
  latex     LaTeX source only (docs.tex)
  pdf       LaTeX source + compiled PDF (requires xelatex or pdflatex)
  all       All four formats in sub-directories html/, md/, latex/, pdf/

Math support
  $...$  and  $$...$$  pass through verbatim in all formats so MathJax,
  KaTeX, or LaTeX renderers render them without modification.",
)]
struct Cli {
    /// Source directories to scan (default: current directory)
    #[arg(value_name = "DIR")]
    dirs: Vec<PathBuf>,

    /// Output directory for the generated site
    #[arg(short, long, value_name = "DIR", default_value = "target/doc")]
    out: PathBuf,

    /// Output format: html | md | latex | pdf | all
    #[arg(short, long, value_name = "FORMAT", default_value = "html")]
    format: String,

    /// List extracted items without writing any files
    #[arg(long)]
    dry_run: bool,
}

fn main() {
    let cli = Cli::parse();

    let scan_dirs: Vec<PathBuf> = if cli.dirs.is_empty() {
        vec![std::env::current_dir().expect("cannot read cwd")]
    } else {
        cli.dirs
    };

    let mut all_items = Vec::new();
    let source_root = scan_dirs[0].clone();

    for dir in &scan_dirs {
        if !dir.is_dir() {
            eprintln!("warning: skipping missing directory: {}", dir.display());
            continue;
        }
        eprintln!("  Scanning {}", dir.display());
        all_items.extend(extract_dir(dir).items);
    }

    if all_items.is_empty() {
        eprintln!("warning: no documented items found");
        eprintln!("  Add doc comments (///, /** */, !>, --!, …) to your sources");
        std::process::exit(1);
    }

    let total = all_items.len();

    if cli.dry_run {
        println!("{total} documented items found:");
        for item in &all_items {
            let rel = item.file
                .strip_prefix(&source_root)
                .unwrap_or(&item.file)
                .display()
                .to_string();
            println!(
                "  [{lang}] {kind} {name} ({rel}:{line})",
                lang = item.lang.label(),
                kind = item.kind.label(),
                name = if item.name.is_empty() { "(anonymous)" } else { &item.name },
                line = item.line,
            );
        }
        return;
    }

    let set = DocSet { items: all_items, source_root };

    if cli.format.eq_ignore_ascii_case("all") {
        run_all_formats(&set, &cli.out, total);
    } else {
        let fmt = OutputFormat::from_str(&cli.format).unwrap_or_else(|| {
            eprintln!(
                "error: unknown format {:?} — expected html, md, latex, pdf, or all",
                cli.format
            );
            std::process::exit(1);
        });
        run_format(&set, &cli.out, &fmt, total);
    }
}

fn run_format(set: &DocSet, out_dir: &PathBuf, fmt: &OutputFormat, total: usize) {
    if let Err(e) = render(set, out_dir, fmt) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    let (label, filename) = format_label(fmt);
    println!("✓ {total} items [{label}] → {}", out_dir.join(filename).display());
}

fn run_all_formats(set: &DocSet, base_out: &PathBuf, total: usize) {
    for fmt in &[OutputFormat::Html, OutputFormat::Markdown, OutputFormat::Latex, OutputFormat::Pdf] {
        let (label, sub) = match fmt {
            OutputFormat::Html     => ("html",   "html"),
            OutputFormat::Markdown => ("md",     "md"),
            OutputFormat::Latex    => ("latex",  "latex"),
            OutputFormat::Pdf      => ("pdf",    "pdf"),
        };
        let out_dir = base_out.join(sub);
        match render(set, &out_dir, fmt) {
            Ok(()) => {
                let (_, filename) = format_label(fmt);
                println!("✓ {total} items [{label}] → {}", out_dir.join(filename).display());
            }
            Err(e) if fmt == &OutputFormat::Pdf => {
                // PDF is best-effort — missing LaTeX is a warning, not a hard failure
                eprintln!("warning: PDF skipped — {e}");
            }
            Err(e) => {
                eprintln!("error [{label}]: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn format_label(fmt: &OutputFormat) -> (&'static str, &'static str) {
    match fmt {
        OutputFormat::Html     => ("html",  "index.html"),
        OutputFormat::Markdown => ("md",    "index.md"),
        OutputFormat::Latex    => ("latex", "docs.tex"),
        OutputFormat::Pdf      => ("pdf",   "docs.pdf"),
    }
}
