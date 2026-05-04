use std::path::{Path, PathBuf};

use freight_doc::extract::{extract_dir, DocSet};
use freight_doc::{render, OutputFormat};
use freight_core::manifest::types::Dependency;
use freight_core::manifest::{find_manifest_dir, load_manifest};

use crate::output::{print_error, print_status, print_success, print_warning};

// ── freight doc ─────────────────────────────────────────────────────────────────

pub fn cmd_doc(format: &str) {
    let cwd = std::env::current_dir().expect("cannot read cwd");
    let project_dir = find_manifest_dir(&cwd).unwrap_or_else(|| cwd.clone());
    let out_dir = project_dir.join("target").join("doc");

    let mut source_dirs: Vec<PathBuf> = Vec::new();

    match load_manifest(&project_dir) {
        Ok(manifest) => {
            // Library source + include dirs
            if let Some(lib) = &manifest.lib {
                let d = project_dir.join(&lib.src);
                if d.is_dir() { source_dirs.push(d); }
                if let Some(inc) = &lib.inc {
                    let inc_dir = project_dir.join(inc);
                    if inc_dir.is_dir() && !source_dirs.contains(&inc_dir) {
                        source_dirs.push(inc_dir);
                    }
                }
            }
            // Binary source dirs — take the parent directory of the src path
            for bin in &manifest.bins {
                let abs = project_dir.join(&bin.src);
                let dir = if abs.is_dir() {
                    abs
                } else {
                    abs.parent().map(PathBuf::from).unwrap_or_else(|| project_dir.clone())
                };
                if dir.is_dir() && !source_dirs.contains(&dir) {
                    source_dirs.push(dir);
                }
            }
            // Default fallback: src/
            if source_dirs.is_empty() {
                let src = project_dir.join("src");
                if src.is_dir() { source_dirs.push(src); }
            }
            // Path dependencies
            for (name, dep) in &manifest.dependencies {
                if let Dependency::Detailed(d) = dep {
                    if let Some(rel) = &d.path {
                        let dep_dir = project_dir.join(rel);
                        if dep_dir.is_dir() {
                            print_status("     Dep", name);
                            source_dirs.push(dep_dir);
                        }
                    }
                }
            }
        }
        Err(_) => {
            let src = project_dir.join("src");
            source_dirs.push(if src.is_dir() { src } else { project_dir.clone() });
        }
    }

    if source_dirs.is_empty() {
        print_error("no source directories to scan");
        return;
    }

    let mut all_items = Vec::new();
    for dir in &source_dirs {
        if !dir.is_dir() {
            print_warning(&format!("skipping missing: {}", dir.display()));
            continue;
        }
        print_status("Scanning", &dir.display().to_string());
        all_items.extend(extract_dir(dir).items);
    }

    if all_items.is_empty() {
        print_warning("no documented items found — add doc comments (///, /**, !>, …) to your sources");
        return;
    }

    let total = all_items.len();
    let combined = DocSet { items: all_items, source_root: project_dir };

    let fmt = OutputFormat::from_str(format).unwrap_or_else(|| {
        print_error(&format!("unknown format {format:?} — expected md, json, msgpack, or all"));
        std::process::exit(1);
    });

    let render_one = |f: &OutputFormat, dir: &PathBuf| {
        let (label, index_file) = match f {
            OutputFormat::Markdown => ("md",      "index.md"),
            OutputFormat::Json     => ("json",    "docs.json"),
            OutputFormat::MsgPack  => ("msgpack", "docs.msgpack"),
        };
        match render(&combined, dir, f) {
            Ok(()) => print_success(&format!("{total} items [{label}] → {}", dir.join(index_file).display())),
            Err(e) => print_error(&format!("failed to write docs [{label}]: {e}")),
        }
    };

    if format.eq_ignore_ascii_case("all") {
        for f in &[OutputFormat::Markdown, OutputFormat::Json, OutputFormat::MsgPack] {
            let sub = match f {
                OutputFormat::Markdown => "md",
                OutputFormat::Json     => "json",
                OutputFormat::MsgPack  => "msgpack",
            };
            render_one(f, &out_dir.join(sub));
        }
    } else {
        render_one(&fmt, &out_dir);
    }
}

// ── freight man ─────────────────────────────────────────────────────────────────

pub fn cmd_man(out_dir: Option<&str>) {
    let out = out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target").join("man"));

    if let Err(e) = std::fs::create_dir_all(&out) {
        print_error(&format!("cannot create output dir: {e}"));
        return;
    }

    let cmd = crate::cli_command();
    let mut count = 0;
    gen_man_pages(&cmd, "freight", &out, &mut count);

    print_success(&format!("{count} man pages → {}", out.display()));
    println!("  Preview : man -l {}/freight.1", out.display());
    println!("  Install : sudo cp {}/*.1 /usr/local/share/man/man1/", out.display());
}

fn gen_man_pages(cmd: &clap::Command, prefix: &str, out_dir: &Path, count: &mut usize) {
    // clap::Command::name() requires 'static; Box::leak is acceptable in a
    // one-shot CLI that exits immediately after generating the pages.
    let static_name: &'static str = Box::leak(prefix.to_string().into_boxed_str());
    let page_cmd = cmd.clone().name(static_name);
    let man = clap_mangen::Man::new(page_cmd);
    let path = out_dir.join(format!("{prefix}.1"));

    match std::fs::File::create(&path) {
        Ok(mut f) => {
            if man.render(&mut f).is_ok() {
                print_status("Generate", &format!("{prefix}.1"));
                *count += 1;
            } else {
                print_warning(&format!("render failed for {prefix}.1"));
            }
        }
        Err(e) => print_warning(&format!("cannot write {}: {e}", path.display())),
    }

    for sub in cmd.get_subcommands() {
        gen_man_pages(sub, &format!("{prefix}-{}", sub.get_name()), out_dir, count);
    }
}
