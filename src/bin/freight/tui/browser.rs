use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

use freight_core::registry::{PackageInfo};
use freight_core::registry::repos::{repo_by_name, registries_in_order};
use freight_core::toolchain::cache::GlobalConfig;

const SEARCH_DEBOUNCE_MS: u64 = 350;
const PAGE_SIZE: usize = 20;

pub struct BrowserResult {
    pub name: String,
    pub version: String,
}

struct App {
    // Search
    query: String,
    cursor: usize,

    // Results
    results: Vec<PackageInfo>,
    list_state: ListState,
    total: usize,
    offset: usize,

    // Detail
    detail: Option<PackageInfo>,
    scroll: u16,

    // State
    loading: bool,
    last_keystroke: Instant,
    needs_search: bool,
    repo: Option<String>,
    error: Option<String>,

    // Layout tracking for mouse hit-testing
    list_area: Rect,
}

impl App {
    fn new(repo: Option<String>) -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            results: Vec::new(),
            list_state: ListState::default(),
            total: 0,
            offset: 0,
            detail: None,
            scroll: 0,
            loading: false,
            last_keystroke: Instant::now(),
            needs_search: true,
            repo,
            error: None,
            list_area: Rect::default(),
        }
    }

    fn selected_index(&self) -> Option<usize> {
        self.list_state.selected()
    }

    fn select(&mut self, idx: usize) {
        if self.results.is_empty() { return; }
        let idx = idx.min(self.results.len() - 1);
        self.list_state.select(Some(idx));
        self.detail = Some(self.results[idx].clone());
        self.scroll = 0;
    }

    fn move_up(&mut self) {
        let idx = self.list_state.selected().unwrap_or(0);
        if idx > 0 { self.select(idx - 1); }
    }

    fn move_down(&mut self) {
        let idx = self.list_state.selected().unwrap_or(0);
        self.select(idx + 1);
    }

    fn do_search(&mut self) {
        self.loading = true;
        self.error = None;
        let q = self.query.clone();
        let offset = self.offset;
        let repo = self.repo.clone();

        let config = GlobalConfig::load();
        let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = match repo.as_deref() {
            Some(name) => match repo_by_name(name, &config) {
                Ok(r) => vec![r],
                Err(e) => { self.error = Some(e.to_string()); self.loading = false; return; }
            },
            None => registries_in_order(&config),
        };

        let mut found = false;
        for r in &repos {
            match r.search(&q) {
                Ok(mut infos) => {
                    // Apply client-side pagination
                    self.total = infos.len();
                    let start = offset.min(infos.len());
                    infos = infos.into_iter().skip(start).take(PAGE_SIZE).collect();
                    self.results = infos;
                    self.loading = false;
                    found = true;

                    // Re-select or default to first
                    if !self.results.is_empty() {
                        let sel = self.list_state.selected().unwrap_or(0).min(self.results.len() - 1);
                        self.select(sel);
                    } else {
                        self.list_state.select(None);
                        self.detail = None;
                    }
                    break;
                }
                Err(e) => {
                    self.error = Some(format!("{e}"));
                }
            }
        }
        if !found && self.error.is_none() {
            self.results.clear();
            self.detail = None;
            self.list_state.select(None);
        }
        self.loading = false;
        self.needs_search = false;
    }

    fn selected_package(&self) -> Option<BrowserResult> {
        let info = self.detail.as_ref()?;
        Some(BrowserResult {
            name: info.name.clone(),
            version: info.latest.clone(),
        })
    }
}

pub fn run_package_browser(repo: Option<&str>) -> anyhow::Result<Option<BrowserResult>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, repo);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    repo: Option<&str>,
) -> anyhow::Result<Option<BrowserResult>> {
    let mut app = App::new(repo.map(String::from));
    app.do_search(); // initial load (empty query = all packages)

    loop {
        terminal.draw(|f| render(f, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => match (key.code, key.modifiers) {
                    // Quit
                    (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        return Ok(None);
                    }
                    // Confirm selection
                    (KeyCode::Enter, _) => {
                        if let Some(pkg) = app.selected_package() {
                            return Ok(Some(pkg));
                        }
                    }
                    // Navigation
                    (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => app.move_up(),
                    (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => app.move_down(),
                    // Scroll detail panel
                    (KeyCode::PageUp, _) => app.scroll = app.scroll.saturating_sub(5),
                    (KeyCode::PageDown, _) => app.scroll = app.scroll.saturating_add(5),
                    // Pagination
                    (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => {
                        if app.offset + PAGE_SIZE < app.total {
                            app.offset += PAGE_SIZE;
                            app.do_search();
                        }
                    }
                    (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                        if app.offset >= PAGE_SIZE {
                            app.offset -= PAGE_SIZE;
                            app.do_search();
                        }
                    }
                    // Search input
                    (KeyCode::Char(c), KeyModifiers::NONE) | (KeyCode::Char(c), KeyModifiers::SHIFT) => {
                        app.query.insert(app.cursor, c);
                        app.cursor += c.len_utf8();
                        app.offset = 0;
                        app.last_keystroke = Instant::now();
                        app.needs_search = true;
                    }
                    (KeyCode::Backspace, _) => {
                        if app.cursor > 0 {
                            let prev = app.query[..app.cursor]
                                .char_indices()
                                .last()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            app.query.drain(prev..app.cursor);
                            app.cursor = prev;
                            app.offset = 0;
                            app.last_keystroke = Instant::now();
                            app.needs_search = true;
                        }
                    }
                    (KeyCode::Delete, _) => {
                        if app.cursor < app.query.len() {
                            app.query.remove(app.cursor);
                            app.offset = 0;
                            app.last_keystroke = Instant::now();
                            app.needs_search = true;
                        }
                    }
                    (KeyCode::Home, _) => app.cursor = 0,
                    (KeyCode::End, _) => app.cursor = app.query.len(),
                    _ => {}
                },
                Event::Mouse(mouse) => match mouse.kind {
                    // Scroll wheel anywhere
                    MouseEventKind::ScrollUp => app.move_up(),
                    MouseEventKind::ScrollDown => app.move_down(),
                    // Click in the list panel → select that row
                    MouseEventKind::Down(MouseButton::Left) => {
                        let area = app.list_area;
                        let x = mouse.column;
                        let y = mouse.row;
                        if x >= area.x && x < area.x + area.width
                            && y > area.y && y < area.y + area.height - 1
                        {
                            // row 0 of list is area.y + 1 (inside border)
                            let row = (y - area.y - 1) as usize;
                            if row < app.results.len() {
                                app.select(row);
                            }
                        }
                    }
                    // Double-click → select + confirm
                    MouseEventKind::Down(MouseButton::Middle) => {
                        if let Some(pkg) = app.selected_package() {
                            return Ok(Some(pkg));
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        // Debounced search trigger
        if app.needs_search
            && app.last_keystroke.elapsed() >= Duration::from_millis(SEARCH_DEBOUNCE_MS)
        {
            app.do_search();
        }
    }
}

fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Outer layout: search bar (3) + body + status bar (1)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    render_search(f, app, outer[0]);
    render_body(f, app, outer[1]);
    render_statusbar(f, app, outer[2]);
}

fn render_search(f: &mut Frame, app: &App, area: Rect) {
    let display = format!("{}_", app.query); // simple cursor indicator
    let p = Paragraph::new(display)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(" Search packages ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(p, area);
}

fn render_body(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_list(f, app, chunks[0]);
    render_detail(f, app, chunks[1]);
}

fn render_list(f: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = area;
    let items: Vec<ListItem> = app.results.iter().map(|pkg| {
        let name = Span::styled(
            format!("{:<30}", truncate(&pkg.name, 28)),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        );
        let ver = Span::styled(
            format!(" {}", pkg.latest),
            Style::default().fg(Color::DarkGray),
        );
        ListItem::new(Line::from(vec![name, ver]))
    }).collect();

    let title = if app.loading {
        " Loading… ".to_string()
    } else {
        format!(" Packages ({}/{}) ", app.results.len(), app.total)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(title, Style::default().fg(Color::White))),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(30, 50, 80))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Details ", Style::default().fg(Color::White)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(pkg) = &app.detail else {
        let hint = Paragraph::new("Select a package with ↑↓, press Enter to add.")
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true });
        f.render_widget(hint, inner);
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // Name + latest
    lines.push(Line::from(vec![
        Span::styled(&pkg.name, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(&pkg.latest, Style::default().fg(Color::Green)),
    ]));
    lines.push(Line::raw(""));

    // Description
    if let Some(desc) = &pkg.description {
        for line in textwrap(desc, inner.width as usize) {
            lines.push(Line::styled(line, Style::default().fg(Color::Gray)));
        }
        lines.push(Line::raw(""));
    }

    // All versions
    lines.push(Line::styled("Versions:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let ver_row: Vec<Span> = pkg.versions.iter().enumerate().map(|(i, v)| {
        let sep = if i == 0 { "" } else { "  " };
        let style = if v.version == pkg.latest {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        Span::styled(format!("{sep}{}", v.version), style)
    }).collect();
    lines.push(Line::from(ver_row));
    lines.push(Line::raw(""));

    // Dependencies of latest version
    if let Some(latest_ver) = pkg.versions.iter().find(|v| v.version == pkg.latest) {
        if !latest_ver.dependencies.is_empty() {
            lines.push(Line::styled("Dependencies:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
            let mut deps: Vec<_> = latest_ver.dependencies.iter().collect();
            deps.sort_by_key(|(k, _)| k.as_str());
            for (dep, ver) in deps {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(dep, Style::default().fg(Color::Cyan)),
                    Span::styled(format!("  {ver}"), Style::default().fg(Color::DarkGray)),
                ]));
            }
        }
    }

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(para, inner);
}

fn render_statusbar(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some(err) = &app.error {
        Span::styled(format!("⚠ {err}"), Style::default().fg(Color::Red))
    } else {
        Span::styled(
            " ↑↓ navigate   Enter select   Esc cancel   ←→ page   PgUp/PgDn scroll detail",
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(Paragraph::new(Line::from(content)), area);
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

fn textwrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![s.to_string()]; }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.len() + 1 + word.len() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() { lines.push(cur); }
    lines
}
