use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

use freight_core::registry::repos::{registries_in_order, repo_by_name};
use freight_core::registry::PackageInfo;
use freight_core::toolchain::cache::GlobalConfig;

use super::common::{enter_tui, leave_tui};

const SEARCH_DEBOUNCE_MS: u64 = 350;
const README_DEBOUNCE_MS: u64 = 250;
const RESULT_WINDOW_SIZE: usize = 100;
const WIDE_VERSION_PANEL_WIDTH: u16 = 150;

enum BrowserResponse {
    Search {
        id: usize,
        cache_key: String,
        packages: Result<Vec<PackageInfo>, String>,
    },
    Readme {
        id: usize,
        name: String,
        readme: Option<String>,
        error: Option<String>,
    },
    /// Full package details (all versions) fetched after selection.
    PackageDetail {
        id: usize,
        name: String,
        info: Option<PackageInfo>,
    },
}

enum PageSelection {
    First,
    Last,
    Preserve,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Packages,
    Details,
    Versions,
}


struct App {
    // Search
    query: String,
    cursor: usize,

    // Results
    all_results: Vec<PackageInfo>,
    search_cache: HashMap<String, Vec<PackageInfo>>,
    results: Vec<PackageInfo>,
    list_state: ListState,
    total: usize,
    offset: usize,

    // Detail
    detail: Option<PackageInfo>,
    readme: Option<String>,
    readme_cache: HashMap<String, Option<String>>,
    pending_readme: Option<String>,
    versions_cache: HashMap<String, Vec<freight_core::registry::PackageVersion>>,
    pending_detail: Option<String>,
    last_selection: Instant,
    scroll: u16,
    version_scroll: u16,
    focus: FocusPane,

    // State
    loading: bool,
    last_keystroke: Instant,
    needs_search: bool,
    repo: Option<String>,
    error: Option<String>,
    tx: Sender<BrowserResponse>,
    rx: Receiver<BrowserResponse>,
    search_request_id: usize,
    active_search_id: Option<usize>,
    readme_request_id: usize,
    active_readme_id: Option<usize>,
    detail_request_id: usize,
    active_detail_id: Option<usize>,

    // Layout tracking for mouse hit-testing
    list_area: Rect,
    detail_area: Rect,
    versions_area: Rect,

    // Names of packages already in the project's [dependencies].
    installed: std::collections::HashSet<String>,

    // Project manifest path and whether we're adding to dev-deps.
    manifest_path: Option<std::path::PathBuf>,
    dev: bool,

    // Last install outcome — shown in the status bar.
    status: Option<String>,
}

impl App {
    fn new(
        repo: Option<String>,
        tx: Sender<BrowserResponse>,
        rx: Receiver<BrowserResponse>,
        installed: std::collections::HashSet<String>,
        manifest_path: Option<std::path::PathBuf>,
        dev: bool,
    ) -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            all_results: Vec::new(),
            search_cache: HashMap::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            total: 0,
            offset: 0,
            detail: None,
            readme: None,
            readme_cache: HashMap::new(),
            pending_readme: None,
            versions_cache: HashMap::new(),
            pending_detail: None,
            last_selection: Instant::now(),
            scroll: 0,
            version_scroll: 0,
            focus: FocusPane::Packages,
            loading: false,
            last_keystroke: Instant::now(),
            needs_search: true,
            repo,
            error: None,
            tx,
            rx,
            search_request_id: 0,
            active_search_id: None,
            readme_request_id: 0,
            active_readme_id: None,
            detail_request_id: 0,
            active_detail_id: None,
            list_area: Rect::default(),
            detail_area: Rect::default(),
            versions_area: Rect::default(),
            installed,
            manifest_path,
            dev,
            status: None,
        }
    }

    fn select(&mut self, idx: usize) {
        if self.results.is_empty() {
            return;
        }
        let idx = idx.min(self.results.len() - 1);
        self.list_state.select(Some(idx));
        let mut detail = self.results[idx].clone();

        // Immediately populate versions from cache if available.
        if let Some(cached) = self.versions_cache.get(&detail.name) {
            detail.versions = cached.clone();
        }
        self.readme = self.readme_cache.get(&detail.name).cloned().flatten();
        self.pending_readme = if self.readme.is_some() {
            None
        } else {
            Some(detail.name.clone())
        };
        self.pending_detail = if self.versions_cache.contains_key(&detail.name) {
            None
        } else {
            Some(detail.name.clone())
        };
        self.last_selection = Instant::now();
        self.detail = Some(detail);
        self.scroll = 0;
        self.version_scroll = 0;
    }

    fn load_pending_readme(&mut self) {
        if self.last_selection.elapsed() < Duration::from_millis(README_DEBOUNCE_MS) {
            return;
        }
        let Some(name) = self.pending_readme.take() else {
            return;
        };
        if self.readme_cache.contains_key(&name) {
            self.readme = self.readme_cache.get(&name).cloned().flatten();
            return;
        }

        self.readme_request_id += 1;
        let id = self.readme_request_id;
        self.active_readme_id = Some(id);
        spawn_readme_request(self.tx.clone(), id, self.repo.clone(), name);
    }

    fn load_pending_detail(&mut self) {
        if self.last_selection.elapsed() < Duration::from_millis(README_DEBOUNCE_MS) {
            return;
        }
        let Some(name) = self.pending_detail.take() else {
            return;
        };
        if self.versions_cache.contains_key(&name) {
            if let Some(detail) = &mut self.detail {
                if detail.name == name {
                    detail.versions = self.versions_cache[&name].clone();
                }
            }
            return;
        }

        self.detail_request_id += 1;
        let id = self.detail_request_id;
        self.active_detail_id = Some(id);
        spawn_detail_request(self.tx.clone(), id, self.repo.clone(), name);
    }

    fn move_up(&mut self) {
        let idx = self.list_state.selected().unwrap_or(0);
        if idx > 0 {
            self.select(idx - 1);
        } else if self.offset >= RESULT_WINDOW_SIZE {
            self.offset -= RESULT_WINDOW_SIZE;
            self.update_window(PageSelection::Last);
        }
    }

    fn move_down(&mut self) {
        let idx = self.list_state.selected().unwrap_or(0);
        if idx + 1 < self.results.len() {
            self.select(idx + 1);
        } else if self.offset + self.results.len() < self.total {
            self.offset += RESULT_WINDOW_SIZE;
            self.update_window(PageSelection::First);
        }
    }

    fn do_search(&mut self) {
        let cache_key = self.search_cache_key();
        if let Some(packages) = self.search_cache.get(&cache_key).cloned() {
            self.error = None;
            self.loading = false;
            self.needs_search = false;
            self.active_search_id = None;
            self.apply_search_results(packages, PageSelection::Preserve);
            return;
        }

        self.loading = true;
        self.error = None;
        self.needs_search = false;
        self.search_request_id += 1;
        let id = self.search_request_id;
        self.active_search_id = Some(id);
        spawn_search_request(
            self.tx.clone(),
            id,
            self.repo.clone(),
            self.query.clone(),
            cache_key,
        );
    }

    fn process_responses(&mut self) {
        while let Ok(response) = self.rx.try_recv() {
            match response {
                BrowserResponse::Search {
                    id,
                    cache_key,
                    packages,
                } => {
                    if self.active_search_id != Some(id) {
                        continue;
                    }
                    self.active_search_id = None;
                    if cache_key != self.search_cache_key() {
                        self.loading = false;
                        continue;
                    }
                    self.loading = false;
                    match packages {
                        Ok(packages) => {
                            self.error = None;
                            self.search_cache.insert(cache_key, packages.clone());
                            self.apply_search_results(packages, PageSelection::Preserve);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.all_results.clear();
                            self.results.clear();
                            self.total = 0;
                            self.detail = None;
                            self.readme = None;
                            self.pending_readme = None;
                            self.list_state.select(None);
                        }
                    }
                }
                BrowserResponse::Readme {
                    id,
                    name,
                    readme,
                    error,
                } => {
                    if self.active_readme_id != Some(id) {
                        continue;
                    }
                    self.active_readme_id = None;
                    if let Some(error) = error {
                        self.error = Some(error);
                        continue;
                    }
                    self.readme_cache.insert(name.clone(), readme.clone());
                    if self
                        .detail
                        .as_ref()
                        .is_some_and(|detail| detail.name == name)
                    {
                        self.readme = readme;
                    }
                }
                BrowserResponse::PackageDetail { id, name, info } => {
                    if self.active_detail_id != Some(id) {
                        continue;
                    }
                    self.active_detail_id = None;
                    if let Some(info) = info {
                        self.versions_cache.insert(name.clone(), info.versions.clone());
                        if let Some(detail) = &mut self.detail {
                            if detail.name == name {
                                detail.versions = info.versions;
                                detail.latest = info.latest;
                            }
                        }
                    }
                }
            }
        }
    }

    fn apply_search_results(&mut self, packages: Vec<PackageInfo>, selection: PageSelection) {
        self.all_results = packages;
        self.total = self.all_results.len();
        self.offset = self.offset.min(self.total.saturating_sub(1));
        self.offset -= self.offset % RESULT_WINDOW_SIZE;
        self.update_window(selection);
    }

    fn update_window(&mut self, selection: PageSelection) {
        let start = self.offset.min(self.all_results.len());
        self.results = self
            .all_results
            .iter()
            .skip(start)
            .take(RESULT_WINDOW_SIZE)
            .cloned()
            .collect();

        if !self.results.is_empty() {
            let sel = match selection {
                PageSelection::First => 0,
                PageSelection::Last => self.results.len() - 1,
                PageSelection::Preserve => self
                    .list_state
                    .selected()
                    .unwrap_or(0)
                    .min(self.results.len() - 1),
            };
            self.select(sel);
        } else {
            self.detail = None;
            self.readme = None;
            self.pending_readme = None;
            self.list_state.select(None);
        }
    }

    fn search_cache_key(&self) -> String {
        format!(
            "{}::{}",
            self.repo.as_deref().unwrap_or_default(),
            self.query
        )
    }

    fn scroll_focused_up(&mut self) {
        match self.focus {
            FocusPane::Packages => self.move_up(),
            FocusPane::Details => self.scroll = self.scroll.saturating_sub(3),
            FocusPane::Versions => self.version_scroll = self.version_scroll.saturating_sub(3),
        }
    }

    fn scroll_focused_down(&mut self) {
        match self.focus {
            FocusPane::Packages => self.move_down(),
            FocusPane::Details => self.scroll = self.scroll.saturating_add(3),
            FocusPane::Versions => self.version_scroll = self.version_scroll.saturating_add(3),
        }
    }

    fn page_focused_up(&mut self) {
        match self.focus {
            FocusPane::Packages => {
                if self.offset >= RESULT_WINDOW_SIZE {
                    self.offset -= RESULT_WINDOW_SIZE;
                    self.update_window(PageSelection::Last);
                }
            }
            FocusPane::Details => self.scroll = self.scroll.saturating_sub(10),
            FocusPane::Versions => self.version_scroll = self.version_scroll.saturating_sub(10),
        }
    }

    fn page_focused_down(&mut self) {
        match self.focus {
            FocusPane::Packages => {
                if self.offset + RESULT_WINDOW_SIZE < self.total {
                    self.offset += RESULT_WINDOW_SIZE;
                    self.update_window(PageSelection::First);
                }
            }
            FocusPane::Details => self.scroll = self.scroll.saturating_add(10),
            FocusPane::Versions => self.version_scroll = self.version_scroll.saturating_add(10),
        }
    }

    fn focus_at(&mut self, x: u16, y: u16) {
        if contains(self.list_area, x, y) {
            self.focus = FocusPane::Packages;
        } else if contains(self.versions_area, x, y) {
            self.focus = FocusPane::Versions;
        } else if contains(self.detail_area, x, y) {
            self.focus = FocusPane::Details;
        }
    }

    /// Add the currently highlighted package to freight.toml without leaving the browser.
    fn install_selected(&mut self) {
        use freight_core::dep_cmds::{manifest_add_dep, manifest_remove_dep};
        use freight_core::manifest::types::Dependency;

        let Some(info) = &self.detail else { return };
        let name = info.name.clone();
        let version = info.latest.clone();

        let Some(ref manifest_path) = self.manifest_path else {
            self.error = Some("no freight.toml found in current directory".into());
            return;
        };

        if self.installed.contains(&name) {
            // Already installed — remove it.
            match manifest_remove_dep(manifest_path, &name) {
                Ok(_) => {
                    let section = if self.dev { "dev-dependencies" } else { "dependencies" };
                    self.status = Some(format!("✗ removed `{name}` from [{section}]"));
                    self.error = None;
                    self.installed.remove(&name);
                }
                Err(e) => {
                    self.error = Some(format!("remove failed: {e}"));
                }
            }
        } else {
            // Not installed — add it.
            let dep = Dependency::Simple(version.clone());
            match manifest_add_dep(manifest_path, &name, &dep, self.dev) {
                Ok(()) => {
                    let section = if self.dev { "dev-dependencies" } else { "dependencies" };
                    self.status = Some(format!("✓ added `{name}@{version}` to [{section}]"));
                    self.error = None;
                    self.installed.insert(name);
                }
                Err(e) => {
                    self.error = Some(format!("add failed: {e}"));
                }
            }
        }
    }
}

fn spawn_search_request(
    tx: Sender<BrowserResponse>,
    id: usize,
    repo: Option<String>,
    query: String,
    cache_key: String,
) {
    thread::spawn(move || {
        let packages = search_packages(repo.as_deref(), &query).map_err(|e| e.to_string());
        let _ = tx.send(BrowserResponse::Search {
            id,
            cache_key,
            packages,
        });
    });
}

fn spawn_readme_request(
    tx: Sender<BrowserResponse>,
    id: usize,
    repo: Option<String>,
    name: String,
) {
    thread::spawn(move || {
        let response = match fetch_package_readme(repo.as_deref(), &name) {
            Ok(readme) => BrowserResponse::Readme {
                id,
                name,
                readme,
                error: None,
            },
            Err(error) => BrowserResponse::Readme {
                id,
                name,
                readme: None,
                error: Some(error.to_string()),
            },
        };
        let _ = tx.send(response);
    });
}

fn spawn_detail_request(
    tx: Sender<BrowserResponse>,
    id: usize,
    repo: Option<String>,
    name: String,
) {
    thread::spawn(move || {
        let info = fetch_full_package_info(repo.as_deref(), &name).ok().flatten();
        let _ = tx.send(BrowserResponse::PackageDetail { id, name, info });
    });
}

fn fetch_full_package_info(repo: Option<&str>, name: &str) -> anyhow::Result<Option<PackageInfo>> {
    let config = GlobalConfig::load();
    let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = match repo {
        Some(repo_name) => vec![repo_by_name(repo_name, &config)?],
        None => registries_in_order(&config),
    };
    for r in &repos {
        if let Ok(Some(info)) = r.lookup(name, None) {
            return Ok(Some(info));
        }
    }
    Ok(None)
}

fn search_packages(repo: Option<&str>, query: &str) -> anyhow::Result<Vec<PackageInfo>> {
    let config = GlobalConfig::load();
    let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = match repo {
        Some(name) => vec![repo_by_name(name, &config)?],
        None => registries_in_order(&config),
    };

    let mut last_error = None;
    for r in &repos {
        match r.search(query) {
            Ok(infos) => return Ok(infos),
            Err(e) => last_error = Some(e),
        }
    }

    match last_error {
        Some(e) => Err(e.into()),
        None => Ok(Vec::new()),
    }
}

/// Read the current project's freight.toml and return all dependency names.
/// Silently returns an empty set when no manifest is found or it can't be parsed.
fn load_installed_deps() -> std::collections::HashSet<String> {
    use freight_core::manifest::{find_manifest_dir, load_manifest};
    let mut set = std::collections::HashSet::new();
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(dir) = find_manifest_dir(&cwd) else { return set };
    let Ok(manifest) = load_manifest(&dir) else { return set };
    for name in manifest.dependencies.keys() {
        set.insert(name.clone());
    }
    for name in manifest.dev_dependencies.keys() {
        set.insert(name.clone());
    }
    set
}

fn fetch_package_readme(repo: Option<&str>, name: &str) -> anyhow::Result<Option<String>> {
    let config = GlobalConfig::load();
    let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = match repo {
        Some(repo_name) => vec![repo_by_name(repo_name, &config)?],
        None => registries_in_order(&config),
    };

    Ok(repos.iter().find_map(|r| r.fetch_readme(name)))
}

pub fn run_package_browser(repo: Option<&str>, dev: bool) -> anyhow::Result<()> {
    let mut terminal = enter_tui()?;
    // Enable mouse capture on top of the standard alternate-screen setup.
    execute!(terminal.backend_mut(), EnableMouseCapture)?;

    let result = run_loop(&mut terminal, repo, dev);

    execute!(terminal.backend_mut(), DisableMouseCapture)?;
    leave_tui(&mut terminal)?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    repo: Option<&str>,
    dev: bool,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel();

    // Collect names of deps already in the project's freight.toml.
    let installed = load_installed_deps();
    let manifest_path = freight_core::manifest::find_manifest_dir(
        &std::env::current_dir().unwrap_or_default(),
    )
    .map(|d| d.join("freight.toml"));

    let mut app = App::new(repo.map(String::from), tx, rx, installed, manifest_path, dev);
    app.do_search(); // initial load (empty query = all packages)

    loop {
        terminal.draw(|f| render(f, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => match (key.code, key.modifiers) {
                    // Quit
                    (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    // Install selected package (stay in browser)
                    (KeyCode::Enter, _) => {
                        app.install_selected();
                    }
                    // Navigation
                    (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => app.move_up(),
                    (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                        app.move_down()
                    }
                    // Scroll the focused panel.
                    (KeyCode::PageUp, _) => app.page_focused_up(),
                    (KeyCode::PageDown, _) => app.page_focused_down(),
                    // Pagination
                    (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => {
                        if app.offset + RESULT_WINDOW_SIZE < app.total {
                            app.offset += RESULT_WINDOW_SIZE;
                            app.update_window(PageSelection::First);
                        }
                    }
                    (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                        if app.offset >= RESULT_WINDOW_SIZE {
                            app.offset -= RESULT_WINDOW_SIZE;
                            app.update_window(PageSelection::Last);
                        }
                    }
                    // Search input
                    (KeyCode::Char(c), KeyModifiers::NONE)
                    | (KeyCode::Char(c), KeyModifiers::SHIFT) => {
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
                    // Scroll whichever panel the pointer is over.
                    MouseEventKind::ScrollUp => {
                        app.focus_at(mouse.column, mouse.row);
                        app.scroll_focused_up();
                    }
                    MouseEventKind::ScrollDown => {
                        app.focus_at(mouse.column, mouse.row);
                        app.scroll_focused_down();
                    }
                    MouseEventKind::Moved => app.focus_at(mouse.column, mouse.row),
                    // Click in the list panel → select that row
                    MouseEventKind::Down(MouseButton::Left) => {
                        app.focus_at(mouse.column, mouse.row);
                        let area = app.list_area;
                        let x = mouse.column;
                        let y = mouse.row;
                        if x >= area.x
                            && x < area.x + area.width
                            && y > area.y
                            && y < area.y + area.height - 1
                        {
                            // row 0 of list is area.y + 1 (inside border)
                            let row = (y - area.y - 1) as usize;
                            if row < app.results.len() {
                                app.select(row);
                            }
                        }
                    }
                    // Double-click → install without leaving
                    MouseEventKind::Down(MouseButton::Middle) => {
                        app.install_selected();
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

        app.load_pending_readme();
        app.load_pending_detail();
        app.process_responses();
    }
}

fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Outer layout: search bar (3) + body + status bar (1)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
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
                .title(Span::styled(
                    " Search packages ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
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
    if area.width >= WIDE_VERSION_PANEL_WIDTH {
        let detail_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
            .split(chunks[1]);
        render_detail(f, app, detail_chunks[0], false);
        render_versions(f, app, detail_chunks[1]);
    } else {
        app.versions_area = Rect::default();
        render_detail(f, app, chunks[1], true);
    }
}

fn render_list(f: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = area;
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|pkg| {
            let already = app.installed.contains(&pkg.name);
            let checkbox = Span::styled(
                if already { "[✓] " } else { "[ ] " },
                Style::default().fg(if already { Color::Green } else { Color::DarkGray }),
            );
            let name = Span::styled(
                format!("{:<30}", truncate(&pkg.name, 28)),
                Style::default()
                    .fg(if already { Color::Green } else { Color::Cyan })
                    .add_modifier(Modifier::BOLD),
            );
            let ver = Span::styled(
                format!(" {}", pkg.latest),
                Style::default().fg(Color::DarkGray),
            );
            ListItem::new(Line::from(vec![checkbox, name, ver]))
        })
        .collect();

    let title = if app.loading {
        " Loading… ".to_string()
    } else if app.total == 0 {
        " Packages (0) ".to_string()
    } else {
        let start = app.offset + 1;
        let end = (app.offset + app.results.len()).min(app.total);
        format!(" Packages {start}-{end} of {} ", app.total)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border_style(app.focus == FocusPane::Packages))
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

fn render_detail(f: &mut Frame, app: &mut App, area: Rect, include_versions: bool) {
    app.detail_area = area;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(app.focus == FocusPane::Details))
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

    let mut lines = build_detail_lines(app, pkg, inner.width as usize, include_versions);

    let para = Paragraph::new(std::mem::take(&mut lines))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(para, inner);
}

fn build_detail_lines<'a>(
    app: &'a App,
    pkg: &'a PackageInfo,
    width: usize,
    include_versions: bool,
) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(
            &pkg.name,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(&pkg.latest, Style::default().fg(Color::Green)),
    ]));
    lines.push(Line::raw(""));

    // Description
    if let Some(desc) = &pkg.description {
        for line in textwrap(desc, width) {
            lines.push(Line::styled(line, Style::default().fg(Color::Gray)));
        }
        lines.push(Line::raw(""));
    }

    if include_versions {
        lines.extend(version_lines(pkg));
        lines.push(Line::raw(""));
    }

    // Dependencies of latest version
    if let Some(latest_ver) = pkg.versions.iter().find(|v| v.version == pkg.latest) {
        if !latest_ver.dependencies.is_empty() {
            lines.push(Line::styled(
                "Dependencies:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
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

    if let Some(readme) = &app.readme {
        if !readme.trim().is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "README:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            for raw_line in readme.lines() {
                if raw_line.trim().is_empty() {
                    lines.push(Line::raw(""));
                } else {
                    for line in textwrap(raw_line.trim(), width) {
                        lines.push(Line::styled(line, Style::default().fg(Color::Gray)));
                    }
                }
            }
        }
    }

    lines
}

fn render_versions(f: &mut Frame, app: &mut App, area: Rect) {
    app.versions_area = area;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(panel_border_style(app.focus == FocusPane::Versions))
        .title(Span::styled(
            " Versions ",
            Style::default().fg(Color::White),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(pkg) = &app.detail else {
        return;
    };

    let para = Paragraph::new(version_lines(pkg))
        .wrap(Wrap { trim: false })
        .scroll((app.version_scroll, 0));
    f.render_widget(para, inner);
}

fn version_lines(pkg: &PackageInfo) -> Vec<Line<'_>> {
    if pkg.versions.is_empty() {
        return vec![Line::styled(
            "No versions returned.",
            Style::default().fg(Color::DarkGray),
        )];
    }

    pkg.versions
        .iter()
        .map(|version| {
            let style = if version.version == pkg.latest {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::styled(version.version.as_str(), style)
        })
        .collect()
}

fn render_statusbar(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some(err) = &app.error {
        Span::styled(format!("⚠ {err}"), Style::default().fg(Color::Red))
    } else if let Some(status) = &app.status {
        Span::styled(status.clone(), Style::default().fg(Color::Green))
    } else {
        let focus = match app.focus {
            FocusPane::Packages => "packages",
            FocusPane::Details => "details",
            FocusPane::Versions => "versions",
        };
        Span::styled(
            format!(
                " focus: {focus}   ↑↓/jk navigate   wheel/PgUp/PgDn scroll   Enter add   Esc close   ←→ page"
            ),
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(Paragraph::new(Line::from(content)), area);
}

fn panel_border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

fn textwrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
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
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}
