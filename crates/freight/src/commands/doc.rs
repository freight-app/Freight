use std::io::{self, IsTerminal};
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

// ── ratatui TUI ───────────────────────────────────────────────────────────────

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};

#[derive(Debug)]
enum Mode {
    /// Browsing the dependency list.
    List,
    /// Reading a dep's docs; `scroll` is the vertical line offset.
    DocView { content: Vec<String>, scroll: u16 },
}

struct DocApp<'a> {
    deps: &'a [DocDependency],
    selected: usize,
    list_offset: usize,
    visible_list_rows: usize,
    mode: Mode,
}

impl<'a> DocApp<'a> {
    fn new(deps: &'a [DocDependency]) -> Self {
        Self {
            deps,
            selected: 0,
            list_offset: 0,
            visible_list_rows: 0,
            mode: Mode::List,
        }
    }

    fn open_doc_view(&mut self) {
        let dep = &self.deps[self.selected];
        let content = load_doc_content(dep);
        self.mode = Mode::DocView { content, scroll: 0 };
    }
}

fn run_dependency_tui(deps: &[DocDependency]) -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_doc_app(&mut terminal, deps);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen,
    )?;
    terminal.show_cursor()?;
    result
}

fn run_doc_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    deps: &[DocDependency],
) -> anyhow::Result<()> {
    let mut app = DocApp::new(deps);

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            match &app.mode {
                Mode::List => {
                    draw_list(frame, &mut app, area);
                }
                Mode::DocView { content, scroll } => {
                    draw_doc_view(frame, &deps[app.selected], content, *scroll, area);
                }
            }
        })?;

        match event::read()? {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                let quit = match &app.mode {
                    Mode::List => handle_list_key(&mut app, key),
                    Mode::DocView { .. } => handle_doc_view_key(&mut app, key),
                };
                if quit {
                    break;
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                if let Mode::List = app.mode {
                    // List block starts at row 2 (below title bar), list content at row 3.
                    let list_start_row: u16 = 3;
                    if mouse.row >= list_start_row {
                        let clicked = app.list_offset + (mouse.row - list_start_row) as usize;
                        if clicked < deps.len() {
                            app.selected = clicked;
                            app.open_doc_view();
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Returns `true` when the app should quit entirely.
fn handle_list_key(app: &mut DocApp<'_>, key: KeyEvent) -> bool {
    match key {
        KeyEvent { code: KeyCode::Char('q'), .. }
        | KeyEvent { code: KeyCode::Esc, .. }
        | KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. } => {
            return true;
        }
        KeyEvent { code: KeyCode::Up, .. }
        | KeyEvent { code: KeyCode::Char('k'), .. } => {
            app.selected = app.selected.saturating_sub(1);
        }
        KeyEvent { code: KeyCode::Down, .. }
        | KeyEvent { code: KeyCode::Char('j'), .. } => {
            app.selected = (app.selected + 1).min(app.deps.len().saturating_sub(1));
        }
        KeyEvent { code: KeyCode::PageUp, .. } => {
            app.selected = app.selected.saturating_sub(app.visible_list_rows.max(1));
        }
        KeyEvent { code: KeyCode::PageDown, .. } => {
            app.selected = (app.selected + app.visible_list_rows.max(1))
                .min(app.deps.len().saturating_sub(1));
        }
        KeyEvent { code: KeyCode::Home, .. }
        | KeyEvent { code: KeyCode::Char('g'), .. } => {
            app.selected = 0;
        }
        KeyEvent { code: KeyCode::End, .. }
        | KeyEvent { code: KeyCode::Char('G'), .. } => {
            app.selected = app.deps.len().saturating_sub(1);
        }
        KeyEvent { code: KeyCode::Enter, .. } => {
            app.open_doc_view();
        }
        _ => {}
    }
    false
}

/// Returns `true` when the app should quit entirely.
fn handle_doc_view_key(app: &mut DocApp<'_>, key: KeyEvent) -> bool {
    let (content_len, scroll) = match &mut app.mode {
        Mode::DocView { content, scroll } => (content.len() as u16, scroll),
        Mode::List => return false,
    };
    match key {
        KeyEvent { code: KeyCode::Char('q'), .. }
        | KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. } => {
            return true;
        }
        KeyEvent { code: KeyCode::Esc, .. }
        | KeyEvent { code: KeyCode::Backspace, .. } => {
            app.mode = Mode::List;
        }
        KeyEvent { code: KeyCode::Up, .. }
        | KeyEvent { code: KeyCode::Char('k'), .. } => {
            *scroll = scroll.saturating_sub(1);
        }
        KeyEvent { code: KeyCode::Down, .. }
        | KeyEvent { code: KeyCode::Char('j'), .. } => {
            *scroll = (*scroll + 1).min(content_len.saturating_sub(1));
        }
        KeyEvent { code: KeyCode::PageUp, .. } => {
            *scroll = scroll.saturating_sub(20);
        }
        KeyEvent { code: KeyCode::PageDown, .. } | KeyEvent { code: KeyCode::Char(' '), .. } => {
            *scroll = (*scroll + 20).min(content_len.saturating_sub(1));
        }
        KeyEvent { code: KeyCode::Home, .. }
        | KeyEvent { code: KeyCode::Char('g'), .. } => {
            *scroll = 0;
        }
        KeyEvent { code: KeyCode::End, .. }
        | KeyEvent { code: KeyCode::Char('G'), .. } => {
            *scroll = content_len.saturating_sub(1);
        }
        _ => {}
    }
    false
}

fn draw_list(
    frame: &mut ratatui::Frame,
    app: &mut DocApp<'_>,
    area: ratatui::layout::Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(4)])
        .split(area);

    let help = Paragraph::new(
        "↑/↓ j/k navigate  PgUp/PgDn jump  Enter/click → open docs  q quit",
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(chunks[1]);

    // ── left: dep list ──
    let items: Vec<ListItem> = app.deps.iter().map(|d| {
        let scope_color = match d.scope {
            "local"      => Color::Cyan,
            "local-dev"  => Color::Blue,
            "global"     => Color::DarkGray,
            _            => Color::White,
        };
        let line = Line::from(vec![
            Span::styled(format!("[{}] ", d.scope), Style::default().fg(scope_color)),
            Span::raw(&d.name),
        ]);
        ListItem::new(line)
    }).collect();

    let mut state = ListState::default().with_offset(app.list_offset);
    state.select(Some(app.selected));
    let list = List::new(items)
        .block(Block::default()
            .title(format!("Dependencies ({})", app.deps.len()))
            .borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD))
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, body[0], &mut state);
    app.list_offset = state.offset();
    app.visible_list_rows = body[0].height.saturating_sub(2) as usize;

    // ── right: dep details ──
    let dep = &app.deps[app.selected];
    let has_docs = !dep.docs.is_empty() || dep.path.is_some();
    let open_hint = if has_docs {
        "Press Enter or click to open docs"
    } else {
        "No docs available — run `freight doc --format md` to generate"
    };
    let mut detail_lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("Name:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&dep.name),
        ]),
        Line::from(vec![
            Span::styled("Kind:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&dep.kind),
        ]),
        Line::from(vec![
            Span::styled("Version: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&dep.version),
        ]),
        Line::from(vec![
            Span::styled("Source:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&dep.source),
        ]),
        Line::from(vec![
            Span::styled("Path:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(dep.path.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not installed on disk".into())),
        ]),
        Line::raw(""),
        Line::from(Span::styled("Doc files:", Style::default().add_modifier(Modifier::BOLD))),
    ];
    if dep.docs.is_empty() {
        detail_lines.push(Line::from(Span::styled(
            "  (none found)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for doc in &dep.docs {
            detail_lines.push(Line::from(format!("  {}", doc.display())));
        }
    }
    detail_lines.push(Line::raw(""));
    detail_lines.push(Line::from(Span::styled(
        open_hint,
        Style::default().fg(if has_docs { Color::Yellow } else { Color::DarkGray }),
    )));

    let details = Paragraph::new(detail_lines)
        .block(Block::default().title("Details").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(details, body[1]);
}

fn draw_doc_view(
    frame: &mut ratatui::Frame,
    dep: &DocDependency,
    content: &[String],
    scroll: u16,
    area: ratatui::layout::Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(4)])
        .split(area);

    let total = content.len();
    let help = Paragraph::new(format!(
        "↑/↓ j/k scroll  PgUp/PgDn page  g/G top/bottom  Esc/Backspace ← list  q quit   [{}/{}]",
        scroll.saturating_add(1).min(total as u16),
        total,
    ))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[0]);

    let lines: Vec<Line> = content.iter().map(|l| Line::raw(l.as_str())).collect();
    let para = Paragraph::new(lines)
        .block(Block::default()
            .title(format!("docs: {}", dep.name))
            .borders(Borders::ALL))
        .scroll((scroll, 0));
    frame.render_widget(para, chunks[1]);
}

// ── Doc content loading ────────────────────────────────────────────────────────

fn load_doc_content(dep: &DocDependency) -> Vec<String> {
    // 1. Try extracting API docs from the dep's source directory.
    if let Some(dep_dir) = &dep.path {
        let src_dir = dep_dir.join("src");
        let scan_dir = if src_dir.is_dir() { src_dir } else { dep_dir.clone() };
        let doc_set = freight_doc::extract::extract_dir(&scan_dir);
        if !doc_set.items.is_empty() {
            return format_doc_items(&doc_set.items);
        }
    }

    // 2. Fall back to reading the first available doc file (README.md, index.md, …).
    for doc_path in &dep.docs {
        if let Ok(text) = std::fs::read_to_string(doc_path) {
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            if lines.is_empty() {
                lines.push("(empty file)".into());
            }
            return lines;
        }
    }

    vec![
        format!("No documentation found for '{}'.", dep.name),
        String::new(),
        "Run `freight doc --format md` inside the dependency directory".into(),
        "to generate API docs, or add a README.md.".into(),
    ]
}

fn format_doc_items(items: &[freight_doc::extract::DocItem]) -> Vec<String> {
    use freight_doc::extract::TagKind;
    let mut lines = Vec::new();
    for item in items {
        let sep = "─".repeat(60);
        lines.push(format!("── {} {} ({}) ──", item.kind.label(), item.name, item.lang.label()));
        if !item.signature.is_empty() {
            lines.push(format!("  {}", item.signature));
        }
        if !item.brief.is_empty() {
            lines.push(String::new());
            lines.push(format!("  {}", item.brief));
        }
        if !item.body.is_empty() {
            lines.push(String::new());
            for body_line in item.body.lines() {
                lines.push(format!("  {}", body_line));
            }
        }
        for tag in &item.tags {
            match &tag.kind {
                TagKind::Param => {
                    let name = tag.name.as_deref().unwrap_or("?");
                    lines.push(format!("  @param {}  {}", name, tag.text));
                }
                _ => {
                    lines.push(format!("  @{}  {}", tag.kind.label(), tag.text));
                }
            }
        }
        lines.push(sep);
        lines.push(String::new());
    }
    if lines.is_empty() {
        lines.push("(no documented items found in source)".into());
    }
    lines
}
