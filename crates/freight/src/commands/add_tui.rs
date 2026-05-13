use std::collections::HashSet;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};

use freight_core::fetch::vcpkg::default_triplet;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VcpkgPackage {
    pub name: String,
    pub version: String,
    pub description: String,
    pub compatibility: VcpkgCompatibility,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum VcpkgCompatibility {
    Compatible,
    Incompatible(String),
    Unknown(String),
}

impl VcpkgCompatibility {
    fn label(&self) -> String {
        match self {
            Self::Compatible => "compatible".to_string(),
            Self::Incompatible(reason) => format!("incompatible: {reason}"),
            Self::Unknown(reason) => format!("unknown: {reason}"),
        }
    }

    fn color(&self) -> Color {
        match self {
            Self::Compatible => Color::Green,
            Self::Incompatible(_) => Color::Red,
            Self::Unknown(_) => Color::Yellow,
        }
    }
}

#[derive(Debug)]
struct App {
    packages: Vec<VcpkgPackage>,
    filter: String,
    selected: usize,
    list_offset: usize,
    visible_list_rows: usize,
    list_area: Rect,
    status: String,
    checked: HashSet<usize>,
}

impl App {
    fn filtered_indices(&self) -> Vec<usize> {
        let needle = self.filter.trim().to_ascii_lowercase();
        self.packages
            .iter()
            .enumerate()
            .filter_map(|(idx, package)| {
                if needle.is_empty()
                    || package.name.to_ascii_lowercase().contains(&needle)
                    || package.description.to_ascii_lowercase().contains(&needle)
                {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered_indices().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    fn selected_package(&self, filtered: &[usize]) -> Option<&VcpkgPackage> {
        filtered
            .get(self.selected)
            .and_then(|idx| self.packages.get(*idx))
    }
}

/// Open an interactive vcpkg package picker and return the selected package names.
pub fn select_vcpkg_packages() -> Result<Vec<String>> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(anyhow!(
            "freight add without a package requires an interactive terminal"
        ));
    }

    let packages = load_vcpkg_packages()?;
    if packages.is_empty() {
        return Err(anyhow!("vcpkg search returned no packages"));
    }

    let mut stdout = io::stdout();
    enable_raw_mode().context("failed to enable raw terminal mode")?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to initialize terminal")?;
    let result = run_app(&mut terminal, packages);

    disable_raw_mode().context("failed to disable raw terminal mode")?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to restore cursor")?;

    result
}

fn load_vcpkg_packages() -> Result<Vec<VcpkgPackage>> {
    let output = Command::new(vcpkg_bin())
        .arg("search")
        .output()
        .context("failed to run `vcpkg search`; install vcpkg or set VCPKG=/path/to/vcpkg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "`vcpkg search` failed with status {}{}",
            output.status.code().unwrap_or(-1),
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = parse_vcpkg_search(&stdout);
    annotate_package_compatibility(&mut packages);
    Ok(packages)
}

fn vcpkg_bin() -> String {
    std::env::var("VCPKG").unwrap_or_else(|_| "vcpkg".into())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    packages: Vec<VcpkgPackage>,
) -> Result<Vec<String>> {
    let mut app = App {
        packages,
        filter: String::new(),
        selected: 0,
        list_offset: 0,
        visible_list_rows: 0,
        list_area: Rect::default(),
        status: "Space or click toggles a package. Enter adds checked packages (or the highlighted package). Press d to open docs.".to_string(),
        checked: HashSet::new(),
    };

    loop {
        terminal.draw(|frame| {
            let filtered = app.filtered_indices();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Min(8),
                ])
                .split(frame.area());
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                .split(chunks[2]);
            app.list_area = panes[0];
            app.visible_list_rows = usize::from(panes[0].height.saturating_sub(2));

            let help = Paragraph::new(
                "Browse vcpkg packages. Type to filter, ↑/↓ to move, Space/click to check, Enter to add checked, d opens docs, Esc cancels.",
            )
            .block(Block::default().title("freight add").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
            frame.render_widget(help, chunks[0]);

            let input = Paragraph::new(app.filter.as_str())
                .block(Block::default().title("Filter packages").borders(Borders::ALL))
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(input, chunks[1]);

            let items: Vec<ListItem> = filtered
                .iter()
                .map(|idx| {
                    let package = &app.packages[*idx];
                    let checkbox = if app.checked.contains(idx) { "[x]" } else { "[ ]" };
                    let version = if package.version.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", package.version)
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{checkbox} ")),
                        Span::styled(
                            compatibility_icon(&package.compatibility),
                            Style::default().fg(package.compatibility.color()),
                        ),
                        Span::raw(format!(" {}{}  ", package.name, version)),
                        Span::styled(
                            package.compatibility.label(),
                            Style::default().fg(package.compatibility.color()),
                        ),
                        Span::raw(format!("  {}", package.description)),
                    ]))
                })
                .collect();

            let mut state = ListState::default().with_offset(app.list_offset);
            if !items.is_empty() {
                state.select(Some(app.selected));
            }
            let list = List::new(items)
                .block(
                    Block::default()
                        .title(format!("Packages ({}/{})", filtered.len(), app.packages.len()))
                        .borders(Borders::ALL),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("› ");
            frame.render_stateful_widget(list, panes[0], &mut state);
            app.list_offset = state.offset();

            let details = Paragraph::new(package_detail_lines(&app, &filtered))
                .block(Block::default().title("Information").borders(Borders::ALL))
                .wrap(Wrap { trim: true });
            frame.render_widget(details, panes[1]);
        })?;

        match event::read()? {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                match key {
                    KeyEvent {
                        code: KeyCode::Esc, ..
                    }
                    | KeyEvent {
                        code: KeyCode::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                        ..
                    } => return Ok(Vec::new()),
                    KeyEvent {
                        code: KeyCode::Enter,
                        ..
                    } => {
                        let filtered = app.filtered_indices();
                        let mut selected: Vec<_> = app.checked.iter().copied().collect();
                        selected.sort_unstable();
                        if selected.is_empty() {
                            return Ok(app
                                .selected_package(&filtered)
                                .map(|package| vec![package.name.clone()])
                                .unwrap_or_default());
                        }
                        return Ok(selected
                            .into_iter()
                            .filter_map(|idx| {
                                app.packages.get(idx).map(|package| package.name.clone())
                            })
                            .collect());
                    }
                    KeyEvent {
                        code: KeyCode::Char(' '),
                        ..
                    } => {
                        let filtered = app.filtered_indices();
                        if let Some(idx) = filtered.get(app.selected) {
                            toggle_checked(&mut app.checked, *idx);
                        }
                    }
                    KeyEvent {
                        code: KeyCode::Char('d'),
                        modifiers,
                        ..
                    } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                        let filtered = app.filtered_indices();
                        open_selected_package_docs(&mut app, &filtered);
                    }
                    KeyEvent {
                        code: KeyCode::Up, ..
                    } => {
                        app.selected = app.selected.saturating_sub(1);
                    }
                    KeyEvent {
                        code: KeyCode::Down,
                        ..
                    } => {
                        let count = app.filtered_indices().len();
                        if app.selected + 1 < count {
                            app.selected += 1;
                        }
                    }
                    KeyEvent {
                        code: KeyCode::Home,
                        ..
                    } => app.selected = 0,
                    KeyEvent {
                        code: KeyCode::End, ..
                    } => {
                        app.selected = app.filtered_indices().len().saturating_sub(1);
                    }
                    KeyEvent {
                        code: KeyCode::Backspace,
                        ..
                    } => {
                        app.filter.pop();
                        app.clamp_selection();
                    }
                    KeyEvent {
                        code: KeyCode::Char(ch),
                        modifiers,
                        ..
                    } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                        app.filter.push(ch);
                        app.selected = 0;
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let filtered = app.filtered_indices();
                if let Some(row) =
                    package_row_for_click(mouse.row, mouse.column, app.list_area, app.list_offset)
                {
                    if row < filtered.len() {
                        app.selected = row;
                        if let Some(idx) = filtered.get(row) {
                            toggle_checked(&mut app.checked, *idx);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn package_detail_lines(app: &App, filtered: &[usize]) -> Vec<Line<'static>> {
    let checked_count = app.checked.len();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Checked: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(checked_count.to_string()),
        ]),
        Line::raw(""),
    ];

    let Some(package) = app.selected_package(filtered) else {
        lines.push(Line::raw("No packages match the current filter."));
        return lines;
    };

    let version = if package.version.is_empty() {
        "unknown".to_string()
    } else {
        package.version.clone()
    };
    lines.extend([
        Line::from(vec![
            Span::styled("Package", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(": {}", package.name)),
        ]),
        Line::from(vec![
            Span::styled("Version", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(": {version}")),
        ]),
        Line::from(vec![
            Span::styled(
                "Compatibility",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(": "),
            Span::styled(
                format!(
                    "{} {}",
                    compatibility_icon(&package.compatibility),
                    package.compatibility.label()
                ),
                Style::default().fg(package.compatibility.color()),
            ),
        ]),
        Line::raw(""),
        Line::styled("Description", Style::default().add_modifier(Modifier::BOLD)),
        Line::raw(package.description.clone()),
        Line::raw(""),
        Line::styled("Actions", Style::default().add_modifier(Modifier::BOLD)),
        Line::raw("Space/click: toggle checkbox"),
        Line::raw("Enter: add checked packages"),
        Line::raw("d: open vcpkg docs"),
        Line::raw("Esc/Ctrl-C: cancel"),
        Line::raw(""),
        Line::styled("Status", Style::default().add_modifier(Modifier::BOLD)),
        Line::raw(app.status.clone()),
    ]);
    lines
}

fn toggle_checked(checked: &mut HashSet<usize>, idx: usize) {
    if !checked.insert(idx) {
        checked.remove(&idx);
    }
}

fn open_selected_package_docs(app: &mut App, filtered: &[usize]) {
    if let Some(package) = app.selected_package(filtered) {
        let name = package.name.clone();
        match open_package_docs(&name) {
            Ok(()) => {
                app.status = format!("Opened docs: {}", package_docs_url(&name));
            }
            Err(e) => {
                app.status = format!(
                    "Could not open docs: {e}. Visit {}",
                    package_docs_url(&name)
                );
            }
        }
    }
}

fn compatibility_icon(compatibility: &VcpkgCompatibility) -> &'static str {
    match compatibility {
        VcpkgCompatibility::Compatible => "✓",
        VcpkgCompatibility::Incompatible(_) => "✗",
        VcpkgCompatibility::Unknown(_) => "?",
    }
}

fn annotate_package_compatibility(packages: &mut [VcpkgPackage]) {
    let Some(root) = vcpkg_root() else {
        for package in packages {
            package.compatibility = VcpkgCompatibility::Unknown(
                "set VCPKG_ROOT to read vcpkg port supports metadata".to_string(),
            );
        }
        return;
    };

    let triplet = default_triplet();
    let env = TripletSupportEnv::from_triplet(&triplet);
    for package in packages {
        package.compatibility = compatibility_for_package(&root, &package.name, &triplet, &env);
    }
}

fn vcpkg_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("VCPKG_ROOT") {
        let path = PathBuf::from(root);
        if path.is_dir() {
            return Some(path);
        }
    }

    let bin = PathBuf::from(vcpkg_bin());
    if bin.is_file() {
        return bin.parent().map(Path::to_path_buf);
    }

    None
}

fn compatibility_for_package(
    root: &Path,
    package_name: &str,
    triplet: &str,
    env: &TripletSupportEnv,
) -> VcpkgCompatibility {
    let manifest_path = root.join("ports").join(package_name).join("vcpkg.json");
    let Ok(contents) = std::fs::read_to_string(&manifest_path) else {
        return VcpkgCompatibility::Unknown(format!(
            "no vcpkg metadata for {package_name} under {}",
            root.display()
        ));
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return VcpkgCompatibility::Unknown(format!("could not parse {}", manifest_path.display()));
    };
    let Some(supports) = json.get("supports").and_then(|value| value.as_str()) else {
        return VcpkgCompatibility::Compatible;
    };
    match SupportsExpr::parse(supports).and_then(|expr| expr.eval(env)) {
        Ok(TriState::True) => VcpkgCompatibility::Compatible,
        Ok(TriState::False) => VcpkgCompatibility::Incompatible(format!(
            "supports {supports:?} does not match {triplet}"
        )),
        Ok(TriState::Unknown) => VcpkgCompatibility::Unknown(format!(
            "supports {supports:?} uses triplet features Freight cannot evaluate"
        )),
        Err(err) => {
            VcpkgCompatibility::Unknown(format!("invalid supports expression {supports:?}: {err}"))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriState {
    True,
    False,
    Unknown,
}

impl TriState {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }

    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SupportsExpr {
    Ident(String),
    Not(Box<SupportsExpr>),
    And(Box<SupportsExpr>, Box<SupportsExpr>),
    Or(Box<SupportsExpr>, Box<SupportsExpr>),
}

impl SupportsExpr {
    fn parse(src: &str) -> Result<Self> {
        let mut parser = SupportsParser::new(src);
        let expr = parser.parse_or()?;
        parser.skip_ws();
        if !parser.is_eof() {
            return Err(anyhow!(
                "unexpected token {:?} at byte {}",
                parser.peek_char().unwrap(),
                parser.pos
            ));
        }
        Ok(expr)
    }

    fn eval(&self, env: &TripletSupportEnv) -> Result<TriState> {
        Ok(match self {
            SupportsExpr::Ident(name) => env.matches(name),
            SupportsExpr::Not(inner) => inner.eval(env)?.not(),
            SupportsExpr::And(left, right) => left.eval(env)?.and(right.eval(env)?),
            SupportsExpr::Or(left, right) => left.eval(env)?.or(right.eval(env)?),
        })
    }
}

struct SupportsParser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> SupportsParser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn parse_or(&mut self) -> Result<SupportsExpr> {
        let mut expr = self.parse_and()?;
        loop {
            self.skip_ws();
            if !self.eat('|') {
                break;
            }
            expr = SupportsExpr::Or(Box::new(expr), Box::new(self.parse_and()?));
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<SupportsExpr> {
        let mut expr = self.parse_unary()?;
        loop {
            self.skip_ws();
            if !self.eat('&') {
                break;
            }
            expr = SupportsExpr::And(Box::new(expr), Box::new(self.parse_unary()?));
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<SupportsExpr> {
        self.skip_ws();
        if self.eat('!') {
            return Ok(SupportsExpr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<SupportsExpr> {
        self.skip_ws();
        if self.eat('(') {
            let expr = self.parse_or()?;
            self.skip_ws();
            if !self.eat(')') {
                return Err(anyhow!("expected ')' at byte {}", self.pos));
            }
            return Ok(expr);
        }
        self.parse_ident()
    }

    fn parse_ident(&mut self) -> Result<SupportsExpr> {
        self.skip_ws();
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(anyhow!("expected identifier at byte {}", self.pos));
        }
        Ok(SupportsExpr::Ident(
            self.src[start..self.pos].to_ascii_lowercase(),
        ))
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek_char() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }
}

#[derive(Debug)]
struct TripletSupportEnv {
    tags: HashSet<String>,
}

impl TripletSupportEnv {
    fn from_triplet(triplet: &str) -> Self {
        let mut tags = triplet
            .split('-')
            .map(|part| part.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        if tags.contains("linux") || tags.contains("osx") || tags.contains("freebsd") {
            tags.insert("unix".to_string());
        }
        if tags.contains("osx") {
            tags.insert("macos".to_string());
        }
        if tags.contains("windows") {
            tags.insert("win32".to_string());
        }
        Self { tags }
    }

    fn matches(&self, ident: &str) -> TriState {
        let ident = ident.to_ascii_lowercase();
        if self.tags.contains(&ident) {
            return TriState::True;
        }
        if matches!(
            ident.as_str(),
            "x86"
                | "x64"
                | "arm"
                | "arm64"
                | "wasm32"
                | "windows"
                | "win32"
                | "linux"
                | "osx"
                | "macos"
                | "ios"
                | "android"
                | "freebsd"
                | "unix"
                | "static"
                | "dynamic"
                | "uwp"
                | "mingw"
                | "msvc"
        ) {
            TriState::False
        } else {
            TriState::Unknown
        }
    }
}

fn package_row_for_click(
    row: u16,
    column: u16,
    list_area: Rect,
    list_offset: usize,
) -> Option<usize> {
    if column < list_area.x || column >= list_area.x.saturating_add(list_area.width) {
        return None;
    }

    let first_item_row = list_area.y.checked_add(1)?;
    let last_item_row = list_area
        .y
        .saturating_add(list_area.height.saturating_sub(1));
    if row < first_item_row || row >= last_item_row {
        return None;
    }

    Some(list_offset + usize::from(row - first_item_row))
}

fn open_package_docs(package_name: &str) -> Result<()> {
    let url = package_docs_url(package_name);
    open_url(&url)
}

fn package_docs_url(package_name: &str) -> String {
    format!("https://vcpkg.io/en/package/{package_name}")
}

fn open_url(url: &str) -> Result<()> {
    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to open {url}"))?;
    Ok(())
}

pub fn parse_vcpkg_search(output: &str) -> Vec<VcpkgPackage> {
    let mut packages = Vec::new();

    for line in output.lines() {
        let line = line.trim_end();
        if line.trim().is_empty()
            || line.starts_with("If your library")
            || line.starts_with("The result may be outdated")
        {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        if name.contains('[') || name.starts_with('-') {
            continue;
        }

        let version = parts.next().unwrap_or_default();
        let description = parts.collect::<Vec<_>>().join(" ");
        packages.push(VcpkgPackage {
            name: name.to_string(),
            version: version.to_string(),
            description,
            compatibility: VcpkgCompatibility::Unknown(
                "compatibility has not been loaded yet".to_string(),
            ),
        });
    }

    packages.sort_by(|left, right| left.name.cmp(&right.name));
    packages.dedup_by(|left, right| left.name == right.name);
    packages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_docs_url_points_at_vcpkg_package_page() {
        assert_eq!(
            package_docs_url("openssl"),
            "https://vcpkg.io/en/package/openssl"
        );
    }

    #[test]
    fn click_rows_map_to_package_indices() {
        let area = Rect::new(0, 6, 40, 12);
        assert_eq!(package_row_for_click(6, 2, area, 0), None);
        assert_eq!(package_row_for_click(7, 2, area, 0), Some(0));
        assert_eq!(package_row_for_click(9, 2, area, 0), Some(2));
        assert_eq!(package_row_for_click(9, 2, area, 20), Some(22));
        assert_eq!(package_row_for_click(17, 2, area, 0), None);
        assert_eq!(package_row_for_click(9, 40, area, 0), None);
    }

    #[test]
    fn evaluates_vcpkg_supports_against_triplet_tags() {
        let env = TripletSupportEnv::from_triplet("x64-linux");
        assert_eq!(
            SupportsExpr::parse("linux").unwrap().eval(&env).unwrap(),
            TriState::True
        );
        assert_eq!(
            SupportsExpr::parse("windows").unwrap().eval(&env).unwrap(),
            TriState::False
        );
        assert_eq!(
            SupportsExpr::parse("linux & !windows")
                .unwrap()
                .eval(&env)
                .unwrap(),
            TriState::True
        );
        assert_eq!(
            SupportsExpr::parse("linux & unknown-feature")
                .unwrap()
                .eval(&env)
                .unwrap(),
            TriState::Unknown
        );
    }

    #[test]
    fn triplet_tags_include_platform_aliases() {
        let linux = TripletSupportEnv::from_triplet("x64-linux");
        assert_eq!(linux.matches("unix"), TriState::True);

        let osx = TripletSupportEnv::from_triplet("arm64-osx");
        assert_eq!(osx.matches("macos"), TriState::True);
    }

    #[test]
    fn parses_vcpkg_search_rows() {
        let packages = parse_vcpkg_search(
            "zlib                 1.3.1#2          A compression library\n\
             openssl              3.5.0            TLS and SSL library\n\
             openssl[tools]                         Build command line tools\n",
        );

        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "openssl");
        assert_eq!(packages[0].version, "3.5.0");
        assert_eq!(packages[1].name, "zlib");
    }
}
