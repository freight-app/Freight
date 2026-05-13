use std::io::{self, IsTerminal};
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
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VcpkgPackage {
    pub name: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug)]
struct App {
    packages: Vec<VcpkgPackage>,
    filter: String,
    selected: usize,
    list_offset: usize,
    visible_list_rows: usize,
    status: String,
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

/// Open an interactive vcpkg package picker and return the selected package name.
pub fn select_vcpkg_package() -> Result<Option<String>> {
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
    Ok(parse_vcpkg_search(&stdout))
}

fn vcpkg_bin() -> String {
    std::env::var("VCPKG").unwrap_or_else(|_| "vcpkg".into())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    packages: Vec<VcpkgPackage>,
) -> Result<Option<String>> {
    let mut app = App {
        packages,
        filter: String::new(),
        selected: 0,
        list_offset: 0,
        visible_list_rows: 0,
        status: "Click a package to open its vcpkg documentation.".to_string(),
    };

    loop {
        terminal.draw(|frame| {
            let filtered = app.filtered_indices();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Min(6),
                    Constraint::Length(5),
                ])
                .split(frame.area());
            app.visible_list_rows = usize::from(chunks[2].height.saturating_sub(2));

            let help = Paragraph::new(
                "Browse vcpkg packages. Type to filter, ↑/↓ to move, Enter to add, click to open docs, Esc or Ctrl-C to cancel.",
            )
            .block(Block::default().title("freight add").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
            frame.render_widget(help, chunks[0]);

            let input = Paragraph::new(app.filter.as_str())
                .block(Block::default().title("Filter").borders(Borders::ALL))
                .style(Style::default().fg(Color::Yellow));
            frame.render_widget(input, chunks[1]);

            let items: Vec<ListItem> = filtered
                .iter()
                .map(|idx| {
                    let package = &app.packages[*idx];
                    let line = if package.version.is_empty() {
                        format!("{}  {}", package.name, package.description)
                    } else {
                        format!("{}  {}  {}", package.name, package.version, package.description)
                    };
                    ListItem::new(line)
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
            frame.render_stateful_widget(list, chunks[2], &mut state);
            app.list_offset = state.offset();

            let detail = app.selected_package(&filtered).map_or_else(
                || "No packages match the current filter.".to_string(),
                |package| {
                    format!(
                        "Selected: {}{}\n{}\n{}",
                        package.name,
                        if package.version.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", package.version)
                        },
                        package.description,
                        app.status
                    )
                },
            );
            let details = Paragraph::new(detail)
                .block(Block::default().title("Details").borders(Borders::ALL))
                .wrap(Wrap { trim: true });
            frame.render_widget(details, chunks[3]);
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
                    } => return Ok(None),
                    KeyEvent {
                        code: KeyCode::Enter,
                        ..
                    } => {
                        let filtered = app.filtered_indices();
                        return Ok(app
                            .selected_package(&filtered)
                            .map(|package| package.name.clone()));
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
                    package_row_for_click(mouse.row, app.list_offset, app.visible_list_rows)
                {
                    if row < filtered.len() {
                        app.selected = row;
                        if let Some(package) = app.selected_package(&filtered) {
                            match open_package_docs(&package.name) {
                                Ok(()) => {
                                    app.status =
                                        format!("Opened docs: {}", package_docs_url(&package.name));
                                }
                                Err(e) => {
                                    app.status = format!(
                                        "Could not open docs: {e}. Visit {}",
                                        package_docs_url(&package.name)
                                    );
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn package_row_for_click(row: u16, list_offset: usize, visible_list_rows: usize) -> Option<usize> {
    // Layout rows: help (0..3), filter (3..6), package list starts at row 6.
    // The list block has a top border, so package entries start one row later.
    let visible_row = usize::from(row.checked_sub(7)?);
    if visible_row >= visible_list_rows {
        return None;
    }
    Some(list_offset + visible_row)
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
        assert_eq!(package_row_for_click(6, 0, 10), None);
        assert_eq!(package_row_for_click(7, 0, 10), Some(0));
        assert_eq!(package_row_for_click(9, 0, 10), Some(2));
        assert_eq!(package_row_for_click(9, 20, 10), Some(22));
        assert_eq!(package_row_for_click(17, 0, 10), None);
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
