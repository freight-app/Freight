use std::path::PathBuf;

use clap::Parser;
use freight_doc::extract::{extract_dir, DocSet};
use freight_doc::render;

/// Extract doc comments from source files and render documentation.
#[derive(Parser)]
#[command(
    name = "freight-doc",
    about = "Multi-language doc comment extractor — outputs Markdown",
)]
struct Cli {
    /// Source directories to scan (default: current directory)
    #[arg(value_name = "DIR")]
    dirs: Vec<PathBuf>,

    /// Output directory for the generated documentation
    #[arg(short, long, value_name = "DIR", default_value = "target/doc")]
    out: PathBuf,

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

    if let Err(e) = render(&set, &cli.out) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    println!("✓ {total} items [md] → {}", cli.out.join("index.md").display());
}
