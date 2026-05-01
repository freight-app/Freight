use std::path::PathBuf;

use clap::Parser;
use freight_doc::extract::{extract_dir, DocSet};
use freight_doc::{render, OutputFormat};

/// Extract doc comments from source files and render documentation.
#[derive(Parser)]
#[command(
    name = "freight-doc",
    about = "Multi-language doc comment extractor — outputs Markdown, JSON, or MessagePack",
    long_about = "\
Scans source directories for documented items in C/C++ (Doxygen /** */, ///),
Rust (///), Fortran (!> / !!), D (/++), and Ada (--!) and renders them.

Output formats
  md       GitHub-Flavored Markdown with cross-document links (default)
  json     Single docs.json — structured data for websites and tooling
  msgpack  Single docs.msgpack — same schema as JSON, binary-encoded
  all      All three formats in sub-directories md/, json/, and msgpack/",
)]
struct Cli {
    /// Source directories to scan (default: current directory)
    #[arg(value_name = "DIR")]
    dirs: Vec<PathBuf>,

    /// Output directory for the generated documentation
    #[arg(short, long, value_name = "DIR", default_value = "target/doc")]
    out: PathBuf,

    /// Output format: md | json | msgpack | all
    #[arg(short, long, value_name = "FORMAT", default_value = "md")]
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
                "error: unknown format {:?} — expected md, json, msgpack, or all",
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
    for fmt in &[OutputFormat::Markdown, OutputFormat::Json, OutputFormat::MsgPack] {
        let sub = match fmt {
            OutputFormat::Markdown => "md",
            OutputFormat::Json     => "json",
            OutputFormat::MsgPack  => "msgpack",
        };
        let out_dir = base_out.join(sub);
        if let Err(e) = render(set, &out_dir, fmt) {
            eprintln!("error [{sub}]: {e}");
            std::process::exit(1);
        }
        let (label, filename) = format_label(fmt);
        println!("✓ {total} items [{label}] → {}", out_dir.join(filename).display());
    }
}

fn format_label(fmt: &OutputFormat) -> (&'static str, &'static str) {
    match fmt {
        OutputFormat::Markdown => ("md",      "index.md"),
        OutputFormat::Json     => ("json",    "docs.json"),
        OutputFormat::MsgPack  => ("msgpack", "docs.msgpack"),
    }
}
