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
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

// tui_markdown is declared at the crate root via Cargo.toml
use tui_markdown;

use freight::registry::repos::{registries_in_order, repo_by_name};
use freight::registry::{PackageInfo, PackageVersion};
use freight::toolchain::cache::GlobalConfig;

use super::common::{enter_tui, leave_tui};

const SEARCH_DEBOUNCE_MS: u64 = 350;
const README_DEBOUNCE_MS: u64 = 250;
const RESULT_WINDOW_SIZE: usize = 100;
/// Minimum width for the 3-column layout (list | README | info+versions).
/// Below this threshold the browser falls back to 2 columns.
const WIDE_THRESHOLD: u16 = 100;

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
    /// Full package details (all versions + owners) fetched after selection.
    PackageDetail {
        id: usize,
        name: String,
        info: Option<PackageInfo>,
        owners: Vec<String>,
    },
}

/// Which sub-view is shown in the Versions panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum VersionTab {
    #[default]
    Versions,
    Dependencies,
}

enum PageSelection {
    First,
    Last,
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    versions_cache: HashMap<String, Vec<freight::registry::PackageVersion>>,
    pending_detail: Option<String>,
    last_selection: Instant,
    scroll: u16,
    info_scroll: u16,
    version_scroll: u16,
    focus: FocusPane,

    // Version selection — which row in the sorted versions list is selected.
    version_list_state: ListState,

    // Which sub-view of the Versions panel is active.
    ver_tab: VersionTab,

    // Owners of the currently displayed package (fetched alongside detail).
    owners: Vec<String>,

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
    info_area: Rect,
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
            info_scroll: 0,
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
            version_list_state: ListState::default(),
            ver_tab: VersionTab::Versions,
            owners: Vec::new(),
            list_area: Rect::default(),
            info_area: Rect::default(),
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
        self.info_scroll = 0;
        self.version_scroll = 0;
        self.ver_tab = VersionTab::Versions;
        self.owners = Vec::new();
        // Reset version selection to the first (latest) entry.
        self.version_list_state = ListState::default();
        self.version_list_state.select(Some(0));
    }

    fn focus_left(&mut self) {
        self.focus = match self.focus {
            FocusPane::Packages => FocusPane::Packages,
            FocusPane::Details => FocusPane::Packages,
            FocusPane::Versions => FocusPane::Details,
        };
    }

    fn focus_right(&mut self) {
        self.focus = match self.focus {
            FocusPane::Packages => FocusPane::Details,
            FocusPane::Details if self.versions_area.width > 0 && self.versions_area.height > 0 => {
                FocusPane::Versions
            }
            FocusPane::Details => FocusPane::Details,
            FocusPane::Versions => FocusPane::Versions,
        };
    }

    fn next_focused_tab(&mut self) {
        if self.focus == FocusPane::Versions {
            self.next_version_tab();
        }
    }

    fn previous_focused_tab(&mut self) {
        if self.focus == FocusPane::Versions {
            self.previous_version_tab();
        }
    }

    fn next_version_tab(&mut self) {
        self.ver_tab = match self.ver_tab {
            VersionTab::Versions => VersionTab::Dependencies,
            VersionTab::Dependencies => VersionTab::Versions,
        };
    }

    fn previous_version_tab(&mut self) {
        self.next_version_tab();
    }

    /// The version string currently selected in the Versions panel.
    /// Falls back to `latest` if no explicit selection has been made.
    fn selected_version(&self) -> Option<String> {
        let pkg = self.detail.as_ref()?;
        if pkg.versions.is_empty() {
            return Some(pkg.latest.clone());
        }
        let mut sorted: Vec<&PackageVersion> = pkg.versions.iter().collect();
        sorted.sort_by(|a, b| cmp_version(&b.version, &a.version));
        let idx = self.version_list_state.selected().unwrap_or(0);
        Some(
            sorted
                .get(idx)
                .map(|v| v.version.clone())
                .unwrap_or_else(|| pkg.latest.clone()),
        )
    }

    fn version_count(&self) -> usize {
        self.detail.as_ref().map(|p| p.versions.len()).unwrap_or(0)
    }

    fn move_version_up(&mut self) {
        let cur = self.version_list_state.selected().unwrap_or(0);
        if cur > 0 {
            self.version_list_state.select(Some(cur - 1));
        }
    }

    fn move_version_down(&mut self) {
        let count = self.version_count();
        if count == 0 {
            return;
        }
        let cur = self.version_list_state.selected().unwrap_or(0);
        if cur + 1 < count {
            self.version_list_state.select(Some(cur + 1));
        }
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
                BrowserResponse::PackageDetail {
                    id,
                    name,
                    info,
                    owners,
                } => {
                    if self.active_detail_id != Some(id) {
                        continue;
                    }
                    self.active_detail_id = None;
                    // Update owners regardless of whether info is present.
                    if self.detail.as_ref().is_some_and(|d| d.name == name) {
                        self.owners = owners;
                    }
                    if let Some(info) = info {
                        self.versions_cache
                            .insert(name.clone(), info.versions.clone());
                        if let Some(detail) = &mut self.detail {
                            if detail.name == name {
                                detail.versions = info.versions;
                                detail.keywords = info.keywords;
                                detail.latest = info.latest.clone();
                                // Select the row that corresponds to `latest`.
                                let mut sorted: Vec<&PackageVersion> =
                                    detail.versions.iter().collect();
                                sorted.sort_by(|a, b| cmp_version(&b.version, &a.version));
                                let latest_idx = sorted
                                    .iter()
                                    .position(|v| v.version == info.latest)
                                    .unwrap_or(0);
                                self.version_list_state.select(Some(latest_idx));
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
            FocusPane::Details if self.info_area.width > 0 && self.info_area.height > 0 => {
                self.info_scroll = self.info_scroll.saturating_sub(3)
            }
            FocusPane::Details => self.scroll = self.scroll.saturating_sub(3),
            FocusPane::Versions => self.version_scroll = self.version_scroll.saturating_sub(3),
        }
    }

    fn scroll_focused_down(&mut self) {
        match self.focus {
            FocusPane::Packages => self.move_down(),
            FocusPane::Details if self.info_area.width > 0 && self.info_area.height > 0 => {
                self.info_scroll = self.info_scroll.saturating_add(3)
            }
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
            FocusPane::Details if self.info_area.width > 0 && self.info_area.height > 0 => {
                self.info_scroll = self.info_scroll.saturating_sub(10)
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
            FocusPane::Details if self.info_area.width > 0 && self.info_area.height > 0 => {
                self.info_scroll = self.info_scroll.saturating_add(10)
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
        } else if contains(self.info_area, x, y) {
            self.focus = FocusPane::Details;
        } else if contains(self.detail_area, x, y) {
            self.focus = FocusPane::Details;
        }
    }

    /// Add the currently highlighted package to freight.toml without leaving the browser.
    fn install_selected(&mut self) {
        use freight::dep_cmds::{manifest_add_dep, manifest_remove_dep};
        use freight::manifest::types::Dependency;

        let Some(info) = &self.detail else { return };
        let name = info.name.clone();
        let version = self
            .selected_version()
            .unwrap_or_else(|| info.latest.clone());

        let Some(ref manifest_path) = self.manifest_path else {
            self.error = Some("no freight.toml found in current directory".into());
            return;
        };

        if self.installed.contains(&name) {
            // Already installed — remove it.
            match manifest_remove_dep(manifest_path, &name) {
                Ok(_) => {
                    let section = if self.dev {
                        "dev-dependencies"
                    } else {
                        "dependencies"
                    };
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
                    let section = if self.dev {
                        "dev-dependencies"
                    } else {
                        "dependencies"
                    };
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
        let info = fetch_full_package_info(repo.as_deref(), &name)
            .ok()
            .flatten();
        let owners = fetch_package_owners(repo.as_deref(), &name);
        let _ = tx.send(BrowserResponse::PackageDetail {
            id,
            name,
            info,
            owners,
        });
    });
}

fn fetch_package_owners(repo: Option<&str>, name: &str) -> Vec<String> {
    use freight::registry::repos::{registries_in_order, repo_by_name};
    let config = freight::toolchain::cache::GlobalConfig::load();
    let repos: Vec<Box<dyn freight::registry::PackageRepo>> = match repo {
        Some(repo_name) => repo_by_name(repo_name, &config)
            .ok()
            .map(|r| vec![r])
            .unwrap_or_default(),
        None => registries_in_order(&config),
    };
    for r in &repos {
        let owners = r.fetch_owners(name);
        if !owners.is_empty() {
            return owners;
        }
    }
    vec![]
}

fn fetch_full_package_info(repo: Option<&str>, name: &str) -> anyhow::Result<Option<PackageInfo>> {
    let config = GlobalConfig::load();
    let repos: Vec<Box<dyn freight::registry::PackageRepo>> = match repo {
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
    let repos: Vec<Box<dyn freight::registry::PackageRepo>> = match repo {
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
    use freight::manifest::{find_manifest_dir, load_manifest};
    let mut set = std::collections::HashSet::new();
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(dir) = find_manifest_dir(&cwd) else {
        return set;
    };
    let Ok(manifest) = load_manifest(&dir) else {
        return set;
    };
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
    let repos: Vec<Box<dyn freight::registry::PackageRepo>> = match repo {
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
    let manifest_path =
        freight::manifest::find_manifest_dir(&std::env::current_dir().unwrap_or_default())
            .map(|d| d.join("freight.toml"));

    let mut app = App::new(
        repo.map(String::from),
        tx,
        rx,
        installed,
        manifest_path,
        dev,
    );
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
                    // Left/right move focus between panes. Tab switches tabs inside
                    // the currently focused pane.
                    (KeyCode::Left, _) => app.focus_left(),
                    (KeyCode::Right, _) => app.focus_right(),
                    (KeyCode::Tab, _) => app.next_focused_tab(),
                    (KeyCode::BackTab, _) => app.previous_focused_tab(),
                    // Also keep the old explicit toggle for the Versions pane.
                    (KeyCode::Char('t'), KeyModifiers::NONE)
                        if app.focus == FocusPane::Versions =>
                    {
                        app.next_version_tab();
                    }
                    // Navigation — Up/Down move selection in the focused pane.
                    // Vim aliases only fire when the search box is empty so that
                    // letters in package names (l, h, j, k) are not stolen.
                    (KeyCode::Up, _) => match app.focus {
                        FocusPane::Versions => app.move_version_up(),
                        _ => app.move_up(),
                    },
                    (KeyCode::Char('k'), KeyModifiers::NONE) if app.query.is_empty() => {
                        match app.focus {
                            FocusPane::Versions => app.move_version_up(),
                            _ => app.move_up(),
                        }
                    }
                    (KeyCode::Down, _) => match app.focus {
                        FocusPane::Versions => app.move_version_down(),
                        _ => app.move_down(),
                    },
                    (KeyCode::Char('j'), KeyModifiers::NONE) if app.query.is_empty() => {
                        match app.focus {
                            FocusPane::Versions => app.move_version_down(),
                            _ => app.move_down(),
                        }
                    }
                    // Scroll the focused panel.
                    (KeyCode::PageUp, _) => app.page_focused_up(),
                    (KeyCode::PageDown, _) => app.page_focused_down(),
                    // Pagination — only via vim aliases now; arrows are pane focus.
                    (KeyCode::Char('l'), KeyModifiers::NONE) if app.query.is_empty() => {
                        if app.offset + RESULT_WINDOW_SIZE < app.total {
                            app.offset += RESULT_WINDOW_SIZE;
                            app.update_window(PageSelection::First);
                        }
                    }
                    (KeyCode::Char('h'), KeyModifiers::NONE) if app.query.is_empty() => {
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
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan))
                .title(Span::styled(
                    " Search packages ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .style(Style::default().fg(Color::Reset));
    f.render_widget(p, area);
}

fn render_body(f: &mut Frame, app: &mut App, area: Rect) {
    if app.detail.is_some() && area.width >= WIDE_THRESHOLD {
        // Wide: 3 columns — package list | README | info + versions
        let [left, middle, right] = Layout::horizontal([
            Constraint::Percentage(30),
            Constraint::Percentage(46),
            Constraint::Percentage(24),
        ])
        .areas(area);
        render_list(f, app, left);
        render_readme_panel(f, app, middle);
        render_info_and_versions(f, app, right);
    } else {
        // Narrow: 2 columns — list | scrollable detail (info + inline versions + README)
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(area);
        app.info_area = Rect::default();
        app.versions_area = Rect::default();
        render_list(f, app, left);
        render_detail(f, app, right, true);
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
                Style::default().fg(if already {
                    Color::Green
                } else {
                    Color::DarkGray
                }),
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
                .border_type(BorderType::Rounded)
                .border_style(panel_border_style(app.focus == FocusPane::Packages))
                .title(Span::styled(title, Style::default().fg(Color::Reset))),
        )
        .highlight_style(Style::default())
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut app.list_state);
}

// ── README panel (wide layout, middle column) ────────────────────────────────

fn render_readme_panel(f: &mut Frame, app: &mut App, area: Rect) {
    // Track this area so mouse-scroll over it fires FocusPane::Details → app.scroll
    app.detail_area = area;

    let focused = app.focus == FocusPane::Details;
    let content = match &app.readme {
        Some(s) if !s.trim().is_empty() => s.as_str(),
        Some(_) => "*No README available.*",
        None if app.pending_readme.is_some() => "*Loading README…*",
        None => "*No README available.*",
    };

    let md = tui_markdown::from_str(content);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(panel_border_style(focused))
        .title(" README  (Tab to focus · PgUp/PgDn scroll) ");
    let para = Paragraph::new(md)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(para, area);
}

// ── Info + Versions panel (wide layout, right column) ───────────────────────

fn render_info_and_versions(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(pkg) = app.detail.as_ref() else {
        return;
    };
    app.detail_area = Rect::default();

    // Extract what we need before borrowing app mutably for render_versions.
    let name = pkg.name.clone();
    let latest = pkg.latest.clone();
    let n_versions = pkg.versions.len();
    let keywords = pkg.keywords.clone();
    let owners = app.owners.clone();
    let desc = pkg.description.clone();

    // Info pane = half the right column (min 6 rows so content fits).
    let info_h = (area.height / 2).max(6);
    let [info_area, ver_area] =
        Layout::vertical([Constraint::Length(info_h), Constraint::Min(4)]).areas(area);
    app.info_area = info_area;

    // ── Build info lines ─────────────────────────────────────────────────────
    // For label + long-value pairs we put the value on its own indented line so
    // ratatui's Wrap can break it naturally without clipping at the right edge.
    let heading = |s: &'static str| -> Span<'static> {
        Span::styled(
            format!("{s:<9}"),
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
    };

    let mut info_lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled(
                "Name     ".to_string(),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(name),
        ]),
        Line::from(vec![
            Span::styled(
                "Latest   ".to_string(),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(latest.clone(), Style::new().fg(Color::Green)),
        ]),
    ];

    // Description — full text, wrapped by ratatui.
    if let Some(d) = desc {
        info_lines.push(Line::raw(""));
        info_lines.push(Line::from(Span::styled(d, Style::new().fg(Color::Gray))));
    }

    // Keywords / categories — value on its own line so wrap stays within pane.
    if !keywords.is_empty() {
        info_lines.push(Line::raw(""));
        info_lines.push(Line::from(heading("Tags")));
        info_lines.push(Line::from(Span::styled(
            keywords.join("  "),
            Style::new().fg(Color::Yellow),
        )));
    }

    // Owners — same pattern.
    if !owners.is_empty() {
        info_lines.push(Line::raw(""));
        info_lines.push(Line::from(heading("Owners")));
        info_lines.push(Line::from(Span::styled(
            owners.join(", "),
            Style::new().fg(Color::Magenta),
        )));
    }

    // Version count.
    info_lines.push(Line::raw(""));
    info_lines.push(Line::from(vec![
        heading("Versions"),
        Span::raw(n_versions.to_string()),
    ]));

    let info_inner_height = info_area.height.saturating_sub(2);
    let content_height = rendered_line_count(&info_lines, info_area.width.saturating_sub(2), true);
    app.info_scroll = clamped_scroll(app.info_scroll, content_height, info_inner_height);

    let info = Paragraph::new(info_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(panel_border_style(app.focus == FocusPane::Details))
                .title(" Info "),
        )
        .wrap(Wrap { trim: true })
        .scroll((app.info_scroll, 0));
    f.render_widget(info, info_area);

    render_versions(f, app, ver_area);
}

// ── Detail panel (narrow layout) ─────────────────────────────────────────────

fn render_detail(f: &mut Frame, app: &mut App, area: Rect, include_versions: bool) {
    app.detail_area = area;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(panel_border_style(app.focus == FocusPane::Details))
        .title(Span::styled(" Details ", Style::default().fg(Color::Reset)));

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
    let focused = app.focus == FocusPane::Versions;

    // Tab header: "Versions" | "Dependencies"
    let tab_versions = if app.ver_tab == VersionTab::Versions {
        Span::styled(
            " Versions ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    } else {
        Span::styled(" Versions ", Style::default().fg(Color::DarkGray))
    };
    let tab_deps = if app.ver_tab == VersionTab::Dependencies {
        Span::styled(
            " Dependencies ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    } else {
        Span::styled(" Dependencies ", Style::default().fg(Color::DarkGray))
    };
    let tab_sep = Span::styled("│", Style::default().fg(Color::DarkGray));
    let title = Line::from(vec![tab_versions, tab_sep, tab_deps]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(panel_border_style(focused))
        .title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Clone package data so we can pass `app` mutably to render helpers.
    let Some(pkg) = app.detail.clone() else {
        return;
    };

    match app.ver_tab {
        VersionTab::Versions => render_version_list(f, app, &pkg, inner),
        VersionTab::Dependencies => render_deps_list(f, app, &pkg, inner),
    }
}

fn render_version_list(f: &mut Frame, app: &mut App, pkg: &PackageInfo, inner: Rect) {
    if pkg.versions.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled(
                "No versions returned.",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    let mut sorted: Vec<&PackageVersion> = pkg.versions.iter().collect();
    sorted.sort_by(|a, b| cmp_version(&b.version, &a.version));

    let selected_idx = app.version_list_state.selected().unwrap_or(0);

    // Available width for each row (minus 2 for radio + space).
    let row_w = inner.width.saturating_sub(2) as usize;

    let items: Vec<ListItem> = sorted
        .iter()
        .enumerate()
        .map(|(i, ver)| {
            let radio = if i == selected_idx { "◉" } else { "○" };
            let is_latest = ver.version == pkg.latest;

            // Left side: "version" or "version (latest)"
            let ver_label = if is_latest {
                format!("{} (latest)", ver.version)
            } else {
                ver.version.clone()
            };

            // Right side: download count — only show when non-zero.
            let dl_label = if ver.downloads > 0 {
                fmt_downloads(ver.downloads)
            } else {
                String::new()
            };

            // Pad ver_label to push dl_label to the right.
            let line = if dl_label.is_empty() {
                Line::from(vec![
                    Span::raw(format!("{radio} ")),
                    Span::styled(
                        ver_label,
                        if i == selected_idx {
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD)
                        } else if is_latest {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default().fg(Color::Gray)
                        },
                    ),
                ])
            } else {
                let gap = row_w
                    .saturating_sub(ver_label.chars().count())
                    .saturating_sub(dl_label.chars().count());
                let padding = " ".repeat(gap);
                Line::from(vec![
                    Span::raw(format!("{radio} ")),
                    Span::styled(
                        ver_label,
                        if i == selected_idx {
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD)
                        } else if is_latest {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default().fg(Color::Gray)
                        },
                    ),
                    Span::raw(padding),
                    Span::styled(dl_label, Style::default().fg(Color::DarkGray)),
                ])
            };

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");

    f.render_stateful_widget(list, inner, &mut app.version_list_state);
}

fn render_deps_list(f: &mut Frame, app: &mut App, pkg: &PackageInfo, inner: Rect) {
    // Show deps of the currently selected version.
    let mut sorted: Vec<&PackageVersion> = pkg.versions.iter().collect();
    sorted.sort_by(|a, b| cmp_version(&b.version, &a.version));
    let sel_idx = app.version_list_state.selected().unwrap_or(0);
    let sel_ver: Option<&PackageVersion> = sorted.get(sel_idx).copied();

    let deps = match sel_ver {
        Some(v) if !v.dependencies.is_empty() => {
            let mut d: Vec<(&String, &String)> = v.dependencies.iter().collect();
            d.sort_by_key(|(k, _)| k.as_str());
            d
        }
        Some(_) => {
            f.render_widget(
                Paragraph::new(Line::styled(
                    "No dependencies.",
                    Style::default().fg(Color::DarkGray),
                )),
                inner,
            );
            return;
        }
        None => {
            f.render_widget(
                Paragraph::new(Line::styled(
                    "Select a version first.",
                    Style::default().fg(Color::DarkGray),
                )),
                inner,
            );
            return;
        }
    };

    let max_name = deps
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(8)
        .min(24);
    let items: Vec<ListItem> = deps
        .into_iter()
        .map(|(dep, ver)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{dep:<width$}", width = max_name),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled("  @", Style::default().fg(Color::DarkGray)),
                Span::styled(ver.clone(), Style::default().fg(Color::Gray)),
            ]))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Compact inline version list used by `render_detail` in narrow-screen mode.
fn version_lines(pkg: &PackageInfo) -> Vec<Line<'_>> {
    if pkg.versions.is_empty() {
        return vec![Line::styled(
            "No versions returned.",
            Style::default().fg(Color::DarkGray),
        )];
    }
    let mut sorted: Vec<&PackageVersion> = pkg.versions.iter().collect();
    sorted.sort_by(|a, b| cmp_version(&b.version, &a.version));
    sorted
        .into_iter()
        .map(|ver| {
            let style = if ver.version == pkg.latest {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::styled(ver.version.as_str(), style)
        })
        .collect()
}

/// Format a download count compactly: 1234 → "1.2k", 1500000 → "1.5M".
fn fmt_downloads(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M ↓", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k ↓", n as f64 / 1_000.0)
    } else {
        format!("{n} ↓")
    }
}

fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let ta: Vec<&str> = a.split(['.', '-', '_']).collect();
    let tb: Vec<&str> = b.split(['.', '-', '_']).collect();
    for (sa, sb) in ta.iter().zip(tb.iter()) {
        let ord = match (sa.parse::<u64>(), sb.parse::<u64>()) {
            (Ok(na), Ok(nb)) => na.cmp(&nb),
            _ => sa.cmp(sb),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ta.len().cmp(&tb.len())
}

fn render_statusbar(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some(err) = &app.error {
        Span::styled(format!("⚠ {err}"), Style::default().fg(Color::Red))
    } else if let Some(status) = &app.status {
        Span::styled(status.clone(), Style::default().fg(Color::Green))
    } else {
        let hint = match app.focus {
            FocusPane::Versions => " ←/→ focus pane   Tab/t versions/deps   ↑↓ select version   Enter add   Esc close",
            _ => " ←/→ focus pane   ↑↓/jk navigate   h/l page   wheel/PgUp/PgDn scroll   Enter add   Esc close",
        };
        Span::styled(hint, Style::default().fg(Color::DarkGray))
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

fn clamped_scroll(scroll: u16, content_len: usize, viewport_height: u16) -> u16 {
    let max_scroll = content_len.saturating_sub(viewport_height as usize);
    scroll.min(max_scroll.min(u16::MAX as usize) as u16)
}

fn rendered_line_count(lines: &[Line<'_>], width: u16, trim: bool) -> usize {
    let width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            let text = if trim { text.trim().to_string() } else { text };
            let chars = text.chars().count();
            chars.div_ceil(width).max(1)
        })
        .sum()
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::mpsc;

    use ratatui::layout::Rect;

    use ratatui::text::{Line, Span};

    use super::{clamped_scroll, rendered_line_count, App, BrowserResponse, FocusPane, VersionTab};

    fn app() -> App {
        let (tx, rx) = mpsc::channel::<BrowserResponse>();
        App::new(None, tx, rx, HashSet::new(), None, false)
    }

    #[test]
    fn horizontal_keys_change_focus_and_tab_switches_version_tabs() {
        let mut app = app();
        app.versions_area = Rect::new(80, 10, 20, 10);

        assert_eq!(app.focus, FocusPane::Packages);
        app.focus_right();
        assert_eq!(app.focus, FocusPane::Details);
        app.focus_right();
        assert_eq!(app.focus, FocusPane::Versions);

        assert_eq!(app.ver_tab, VersionTab::Versions);
        app.next_focused_tab();
        assert_eq!(app.ver_tab, VersionTab::Dependencies);
        app.previous_focused_tab();
        assert_eq!(app.ver_tab, VersionTab::Versions);

        app.focus_left();
        assert_eq!(app.focus, FocusPane::Details);
        app.focus_left();
        assert_eq!(app.focus, FocusPane::Packages);

        app.next_focused_tab();
        assert_eq!(app.ver_tab, VersionTab::Versions);
    }

    #[test]
    fn right_focus_stays_on_details_when_versions_pane_is_not_visible() {
        let mut app = app();

        app.focus_right();
        assert_eq!(app.focus, FocusPane::Details);
        app.focus_right();
        assert_eq!(app.focus, FocusPane::Details);
    }

    #[test]
    fn info_scroll_is_clamped_to_rendered_content() {
        let lines = vec![
            Line::from(Span::raw("short")),
            Line::from(Span::raw("this line wraps across several cells")),
        ];
        let content_len = rendered_line_count(&lines, 10, true);

        assert!(content_len > lines.len());
        assert_eq!(clamped_scroll(99, content_len, 3), 2);
        assert_eq!(clamped_scroll(99, 2, 3), 0);
    }
}
