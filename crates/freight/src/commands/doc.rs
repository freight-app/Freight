use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use freight_core::manifest::types::{Dependency, Manifest};
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::freight_home;
use freight_doc::extract::{extract_dir, DocSet};
use freight_doc::{render, OutputFormat};

use crate::output::{print_error, print_status, print_success, print_warning};

// ── freight doc ─────────────────────────────────────────────────────────────────

pub fn cmd_doc(format: Option<&str>) {
    if let Some(format) = format {
        generate_docs(format);
    } else if let Err(e) = open_dependency_tui() {
        print_error(&format!("failed to open dependency docs: {e}"));
    }
}

fn generate_docs(format: &str) {
    let cwd = std::env::current_dir().expect("cannot read cwd");
    let project_dir = find_manifest_dir(&cwd).unwrap_or_else(|| cwd.clone());
    let out_dir = project_dir.join("target").join("doc");

    let mut source_dirs: Vec<PathBuf> = Vec::new();

    match load_manifest(&project_dir) {
        Ok(manifest) => {
            // Library source + header dirs
            if let Some(lib) = &manifest.lib {
                for s in &lib.srcs {
                    let d = project_dir.join(s);
                    let dir = if d.is_dir() {
                        d
                    } else {
                        d.parent()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| project_dir.clone())
                    };
                    if dir.is_dir() && !source_dirs.contains(&dir) {
                        source_dirs.push(dir);
                    }
                }
                for hdr in &lib.hdrs {
                    if let Some(parent) = project_dir.join(hdr).parent().map(PathBuf::from) {
                        if parent.is_dir() && !source_dirs.contains(&parent) {
                            source_dirs.push(parent);
                        }
                    }
                }
            }
            // Binary source dirs — take the parent directory of the src path
            for bin in &manifest.bins {
                let abs = project_dir.join(&bin.src);
                let dir = if abs.is_dir() {
                    abs
                } else {
                    abs.parent()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| project_dir.clone())
                };
                if dir.is_dir() && !source_dirs.contains(&dir) {
                    source_dirs.push(dir);
                }
            }
            // Default fallback: src/
            if source_dirs.is_empty() {
                let src = project_dir.join("src");
                if src.is_dir() {
                    source_dirs.push(src);
                }
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
            source_dirs.push(if src.is_dir() {
                src
            } else {
                project_dir.clone()
            });
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
        print_warning(
            "no documented items found — add doc comments (///, /**, !>, …) to your sources",
        );
        return;
    }

    let total = all_items.len();
    let combined = DocSet {
        items: all_items,
        source_root: project_dir,
    };

    let all_formats = format.eq_ignore_ascii_case("all");
    let fmt = if all_formats {
        None
    } else {
        Some(OutputFormat::from_str(format).unwrap_or_else(|| {
            print_error(&format!(
                "unknown format {format:?} — expected md, json, msgpack, or all"
            ));
            std::process::exit(1);
        }))
    };

    let render_one = |f: &OutputFormat, dir: &PathBuf| {
        let (label, index_file) = match f {
            OutputFormat::Markdown => ("md", "index.md"),
            OutputFormat::Json => ("json", "docs.json"),
            OutputFormat::MsgPack => ("msgpack", "docs.msgpack"),
        };
        match render(&combined, dir, f) {
            Ok(()) => print_success(&format!(
                "{total} items [{label}] → {}",
                dir.join(index_file).display()
            )),
            Err(e) => print_error(&format!("failed to write docs [{label}]: {e}")),
        }
    };

    if all_formats {
        for f in &[
            OutputFormat::Markdown,
            OutputFormat::Json,
            OutputFormat::MsgPack,
        ] {
            let sub = match f {
                OutputFormat::Markdown => "md",
                OutputFormat::Json => "json",
                OutputFormat::MsgPack => "msgpack",
            };
            render_one(f, &out_dir.join(sub));
        }
    } else if let Some(fmt) = fmt {
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
    println!(
        "  Install : sudo cp {}/*.1 /usr/local/share/man/man1/",
        out.display()
    );
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

// ── freight doc dependency browser ────────────────────────────────────────────

#[derive(Debug, Clone)]
struct DocDependency {
    name: String,
    scope: &'static str,
    kind: String,
    version: String,
    source: String,
    path: Option<PathBuf>,
    docs: Vec<PathBuf>,
}

fn open_dependency_tui() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd);
    let deps = collect_doc_dependencies(&project_dir);

    if deps.is_empty() {
        print_warning("no installed local or global dependencies found");
        println!("hint: add dependencies to freight.toml and run `freight fetch`, or use `freight doc --format md` to generate API docs");
        return Ok(());
    }

    if !io::stdout().is_terminal() {
        print_dependency_table(&deps);
        return Ok(());
    }

    run_dependency_tui(&deps)
}

fn collect_doc_dependencies(project_dir: &Path) -> Vec<DocDependency> {
    let mut deps = Vec::new();
    if let Ok(manifest) = load_manifest(project_dir) {
        collect_manifest_dependencies(project_dir, &manifest, "local", false, &mut deps);
        collect_manifest_dependencies(project_dir, &manifest, "local", true, &mut deps);
    }
    collect_global_dependencies(&mut deps);
    deps.sort_by(|a, b| (a.scope, &a.name).cmp(&(b.scope, &b.name)));
    deps.dedup_by(|a, b| a.scope == b.scope && a.name == b.name && a.path == b.path);
    deps
}

fn collect_manifest_dependencies(
    project_dir: &Path,
    manifest: &Manifest,
    scope: &'static str,
    dev: bool,
    out: &mut Vec<DocDependency>,
) {
    let deps: Vec<(String, Dependency)> = if dev {
        manifest
            .dev_dependencies
            .iter()
            .map(|(name, dep)| (name.clone(), dep.clone()))
            .collect()
    } else {
        manifest.effective_dependencies().into_iter().collect()
    };
    for (name, dep) in deps {
        let mut item = dependency_summary(project_dir, &name, &dep, scope);
        if dev {
            item.scope = "local-dev";
        }
        out.push(item);
    }
}

fn dependency_summary(
    project_dir: &Path,
    name: &str,
    dep: &Dependency,
    scope: &'static str,
) -> DocDependency {
    let (kind, version, source, path) = match dep {
        Dependency::Simple(version) => {
            let dir = project_dir.join(".deps").join(name);
            (
                "registry".to_string(),
                version.clone(),
                "freight.dev".to_string(),
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if d.system.is_some() => {
            let source = d
                .pkg_config
                .as_deref()
                .or(d.system.as_deref())
                .unwrap_or("system")
                .to_string();
            (
                "system".to_string(),
                d.version.clone().unwrap_or_else(|| "*".into()),
                source,
                None,
            )
        }
        Dependency::Detailed(d) if d.path.is_some() => {
            let rel = d.path.as_deref().unwrap_or_default();
            let dir = project_dir.join(rel);
            (
                "path".to_string(),
                manifest_version(&dir)
                    .unwrap_or_else(|| d.version.clone().unwrap_or_else(|| "*".into())),
                rel.to_string(),
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if d.git.is_some() => {
            let dir = project_dir.join(".deps").join(name);
            let source = d.git.clone().unwrap_or_default();
            (
                "git".to_string(),
                git_ref(d),
                source,
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if d.url.is_some() => {
            let dir = project_dir.join(".deps").join(name);
            let source = d.url.clone().unwrap_or_default();
            (
                "url".to_string(),
                d.version.clone().unwrap_or_else(|| "*".into()),
                source,
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) => {
            let dir = project_dir.join(".deps").join(name);
            (
                "registry".to_string(),
                d.version.clone().unwrap_or_else(|| "*".into()),
                "freight.dev".to_string(),
                dir.exists().then_some(dir),
            )
        }
    };
    let docs = path.as_deref().map(find_doc_files).unwrap_or_default();
    DocDependency {
        name: name.to_string(),
        scope,
        kind,
        version,
        source,
        path,
        docs,
    }
}

fn collect_global_dependencies(out: &mut Vec<DocDependency>) {
    let Some(home) = freight_home() else {
        return;
    };
    for root in [
        home.join("deps"),
        home.join("registry"),
        home.join("registry").join("src"),
    ] {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = manifest_version(&dir).unwrap_or_else(|| "installed".into());
            let docs = find_doc_files(&dir);
            out.push(DocDependency {
                name,
                scope: "global",
                kind: "cached".into(),
                version,
                source: root.display().to_string(),
                path: Some(dir),
                docs,
            });
        }
    }
}

fn manifest_version(dir: &Path) -> Option<String> {
    load_manifest(dir).ok().map(|m| m.package.version)
}

fn git_ref(d: &freight_core::manifest::types::DetailedDep) -> String {
    d.rev
        .as_deref()
        .or(d.tag.as_deref())
        .or(d.branch.as_deref())
        .or(d.version.as_deref())
        .unwrap_or("*")
        .to_string()
}

fn find_doc_files(dir: &Path) -> Vec<PathBuf> {
    let candidates = [
        dir.join("target/doc/index.md"),
        dir.join("target/doc/index.html"),
        dir.join("docs/index.md"),
        dir.join("README.md"),
        dir.join("README"),
    ];
    candidates.into_iter().filter(|p| p.exists()).collect()
}

fn print_dependency_table(deps: &[DocDependency]) {
    println!("freight dependency docs");
    for dep in deps {
        let location = dep
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not installed on disk".into());
        println!(
            "- [{}] {} {} ({}) — {}",
            dep.scope, dep.name, dep.version, dep.kind, location
        );
    }
}

fn run_dependency_tui(deps: &[DocDependency]) -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;

    let mut selected = 0usize;
    let mut scroll = 0usize;
    loop {
        let (cols, rows) = terminal_size();
        let list_width = (cols / 3).max(28).min(cols.saturating_sub(30));
        let visible = rows.saturating_sub(5) as usize;
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll.saturating_add(visible.max(1)) {
            scroll = selected.saturating_sub(visible.saturating_sub(1));
        }
        draw_dependency_tui(&mut stdout, deps, selected, scroll, list_width, cols, rows)?;

        match read_key()? {
            UiKey::Quit => break,
            UiKey::Down => selected = (selected + 1).min(deps.len() - 1),
            UiKey::Up => selected = selected.saturating_sub(1),
            UiKey::PageDown => selected = (selected + visible.max(1)).min(deps.len() - 1),
            UiKey::PageUp => selected = selected.saturating_sub(visible.max(1)),
            UiKey::Home => selected = 0,
            UiKey::End => selected = deps.len() - 1,
            UiKey::Ignore => {}
        }
    }
    Ok(())
}

struct TerminalGuard {
    saved_stty: Option<String>,
}

impl TerminalGuard {
    fn enter(stdout: &mut io::Stdout) -> anyhow::Result<Self> {
        let saved_stty = std::process::Command::new("stty")
            .arg("-g")
            .output()
            .ok()
            .and_then(|out| {
                out.status
                    .success()
                    .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            });
        let _ = std::process::Command::new("stty")
            .args(["raw", "-echo"])
            .status();
        write!(stdout, "\x1b[?1049h\x1b[?25l")?;
        stdout.flush()?;
        Ok(Self { saved_stty })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\x1b[?25h\x1b[?1049l");
        let _ = stdout.flush();
        if let Some(saved) = &self.saved_stty {
            let _ = std::process::Command::new("stty").arg(saved).status();
        } else {
            let _ = std::process::Command::new("stty").args(["sane"]).status();
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum UiKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Quit,
    Ignore,
}

fn read_key() -> io::Result<UiKey> {
    let mut stdin = io::stdin();
    let mut byte = [0_u8; 1];
    stdin.read_exact(&mut byte)?;
    Ok(match byte[0] {
        b'q' | 27 => {
            if byte[0] == 27 {
                let mut seq = [0_u8; 2];
                if stdin.read(&mut seq)? == 2 && seq[0] == b'[' {
                    match seq[1] {
                        b'A' => UiKey::Up,
                        b'B' => UiKey::Down,
                        b'H' => UiKey::Home,
                        b'F' => UiKey::End,
                        b'5' | b'6' => {
                            let mut tilde = [0_u8; 1];
                            let _ = stdin.read(&mut tilde);
                            if seq[1] == b'5' {
                                UiKey::PageUp
                            } else {
                                UiKey::PageDown
                            }
                        }
                        _ => UiKey::Quit,
                    }
                } else {
                    UiKey::Quit
                }
            } else {
                UiKey::Quit
            }
        }
        b'j' => UiKey::Down,
        b'k' => UiKey::Up,
        b'g' => UiKey::Home,
        b'G' => UiKey::End,
        b' ' => UiKey::PageDown,
        _ => UiKey::Ignore,
    })
}

fn terminal_size() -> (u16, u16) {
    if let Ok(out) = std::process::Command::new("stty").arg("size").output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut parts = text
                .split_whitespace()
                .filter_map(|p| p.parse::<u16>().ok());
            if let (Some(rows), Some(cols)) = (parts.next(), parts.next()) {
                return (cols, rows);
            }
        }
    }
    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let rows = std::env::var("LINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    (cols, rows)
}

fn draw_dependency_tui(
    stdout: &mut io::Stdout,
    deps: &[DocDependency],
    selected: usize,
    scroll: usize,
    list_width: u16,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    write!(stdout, "\x1b[2J\x1b[H")?;
    write!(stdout, "freight doc — dependency documentation browser\r\n")?;
    write!(stdout, "↑/↓ or j/k scroll  PgUp/PgDn jump  q quit\r\n\r\n")?;
    write!(
        stdout,
        "{:<width$}  Details\r\n",
        "Dependencies",
        width = list_width as usize
    )?;

    let max_rows = rows.saturating_sub(5) as usize;
    let detail = detail_lines(&deps[selected]);
    for line_idx in 0..max_rows {
        let dep_idx = scroll + line_idx;
        let list_text = if let Some(dep) = deps.get(dep_idx) {
            let marker = if dep_idx == selected { "›" } else { " " };
            truncate(
                &format!("{marker} [{}] {}", dep.scope, dep.name),
                list_width.saturating_sub(1) as usize,
            )
        } else {
            String::new()
        };
        let detail_text = detail.get(line_idx).map(String::as_str).unwrap_or("");
        if dep_idx == selected {
            write!(
                stdout,
                "\x1b[7m{:<width$}\x1b[0m  {}\r\n",
                list_text,
                truncate(detail_text, cols.saturating_sub(list_width + 3) as usize),
                width = list_width as usize
            )?;
        } else {
            write!(
                stdout,
                "{:<width$}  {}\r\n",
                list_text,
                truncate(detail_text, cols.saturating_sub(list_width + 3) as usize),
                width = list_width as usize
            )?;
        }
    }
    stdout.flush()
}

fn detail_lines(dep: &DocDependency) -> Vec<String> {
    let mut lines = vec![
        format!("Name:    {}", dep.name),
        format!("Scope:   {}", dep.scope),
        format!("Kind:    {}", dep.kind),
        format!("Version: {}", dep.version),
        format!("Source:  {}", dep.source),
        format!(
            "Path:    {}",
            dep.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not installed on disk".into())
        ),
        String::new(),
        "Documentation files:".into(),
    ];
    if dep.docs.is_empty() {
        lines.push("  No README or generated docs found. Run `freight doc --format md` in the dependency to generate API docs.".into());
    } else {
        lines.extend(dep.docs.iter().map(|p| format!("  {}", p.display())));
    }
    lines
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}
