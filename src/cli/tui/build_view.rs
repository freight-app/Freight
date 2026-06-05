//! Ratatui inline viewport for `freight build`.
//!
//! The build runs in a background thread; the main thread drives a fixed-height
//! inline area that updates ~20 fps.  When the build finishes the viewport
//! stays in the scroll buffer as a compact build summary.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ratatui::{
    backend::CrosstermBackend,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame, Terminal, TerminalOptions, Viewport,
};

use freight::build::{build_project_with, build_workspace_with, BuildOutput};
use freight::error::FreightError;
use freight::event::{BuildEvent, Progress};

// ── Spinner ───────────────────────────────────────────────────────────────────

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn spinner_char(tick: u64) -> char {
    SPINNER[(tick as usize) % SPINNER.len()]
}

// ── State ─────────────────────────────────────────────────────────────────────

struct DepBuild {
    name: String,
    dots: usize,
}

enum Status {
    Done(String),
    Failed(String),
}

pub struct BuildViewState {
    project: String,
    profile: String,
    dep_builds: Vec<DepBuild>,
    recent: VecDeque<String>,      // last N compiling/linking/etc lines
    warnings: usize,
    status: Option<Status>,
    compiled: usize,
    skipped: usize,
    tick: u64,
    done: bool,
}

const MAX_RECENT: usize = 3;
const VIEWPORT_HEIGHT: u16 = 8;

impl BuildViewState {
    fn new() -> Self {
        Self {
            project: String::new(),
            profile: String::new(),
            dep_builds: Vec::new(),
            recent: VecDeque::new(),
            warnings: 0,
            status: None,
            compiled: 0,
            skipped: 0,
            tick: 0,
            done: false,
        }
    }

    fn push_recent(&mut self, line: String) {
        if self.recent.len() >= MAX_RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(line);
    }

    fn handle(&mut self, ev: BuildEvent) {
        match ev {
            BuildEvent::BuildStarted { name, profile } => {
                self.project = name;
                self.profile = profile;
            }
            BuildEvent::Compiling { path } => {
                self.compiled += 1;
                let file = path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.push_recent(format!("Compiling  {file}"));
            }
            BuildEvent::Fresh { .. } => {
                self.skipped += 1;
            }
            BuildEvent::Linking { name } => {
                self.push_recent(format!("Linking    {name}"));
            }
            BuildEvent::Archiving { name } => {
                self.push_recent(format!("Archiving  {name}"));
            }
            BuildEvent::FetchingDep { name, source } => {
                self.push_recent(format!("Fetching   {name} ({source})"));
            }
            BuildEvent::BuildingForeignDep { name, backend } => {
                self.push_recent(format!("Building   {name} ({backend})"));
            }
            BuildEvent::RunningScript { cached } => {
                if !cached {
                    self.push_recent("Running    build script".into());
                }
            }
            BuildEvent::DepBuildStarted { name } => {
                self.dep_builds.push(DepBuild { name, dots: 0 });
            }
            BuildEvent::DepCompiling => {
                if let Some(d) = self.dep_builds.last_mut() {
                    d.dots += 1;
                }
            }
            BuildEvent::DepBuildDone => {
                // Leave the dep in the list so it stays visible in the viewport.
            }
            BuildEvent::Warning(_) => {
                self.warnings += 1;
            }
            _ => {}
        }
    }

    fn finish_ok(&mut self, outputs: &[BuildOutput]) {
        let total_compiled: usize = outputs.iter().map(|o| o.compiled).sum();
        let total_skipped: usize = outputs.iter().map(|o| o.skipped).sum();
        let names: Vec<&str> = outputs.iter().map(|o| o.package_name.as_str()).collect();
        let label = if names.len() == 1 {
            names[0].to_string()
        } else {
            format!("{} packages", names.len())
        };
        let warn = if self.warnings > 0 {
            format!(", {} warning{}", self.warnings, if self.warnings == 1 { "" } else { "s" })
        } else {
            String::new()
        };
        self.status = Some(Status::Done(format!(
            "{label} ({total_compiled} compiled, {total_skipped} up to date{warn})"
        )));
        self.done = true;
    }

    fn finish_err(&mut self, e: &FreightError) {
        self.status = Some(Status::Failed(e.to_string()));
        self.done = true;
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(f: &mut Frame, state: &BuildViewState) {
    let area = f.area();

    let cyan_bold = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let dimmed = Style::default().add_modifier(Modifier::DIM);
    let green_bold = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let red_bold = Style::default()
        .fg(Color::Red)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line> = Vec::new();

    // ── Project header ────────────────────────────────────────────────────────
    if !state.project.is_empty() {
        let (prefix_char, prefix_style) = if state.done {
            match &state.status {
                Some(Status::Failed(_)) => ('✗', red_bold),
                _ => ('✓', green_bold),
            }
        } else {
            (spinner_char(state.tick), cyan_bold)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{prefix_char:>13} "), prefix_style),
            Span::styled(
                format!("{} [{}]", state.project, state.profile),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    // ── Active dep builds ─────────────────────────────────────────────────────
    for dep in &state.dep_builds {
        let dots: String = "·".repeat(dep.dots);
        lines.push(Line::from(vec![
            Span::styled(format!("{:>13} ", "Building"), cyan_bold),
            Span::raw(format!("{} ", dep.name)),
            Span::styled(dots, dimmed),
        ]));
    }

    // ── Recent activity ───────────────────────────────────────────────────────
    for line in &state.recent {
        lines.push(Line::from(Span::styled(
            format!("{:>13}  {}", "", line),
            dimmed,
        )));
    }

    // ── Final status ──────────────────────────────────────────────────────────
    if let Some(status) = &state.status {
        match status {
            Status::Done(msg) => {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:>13} ", "✓"), green_bold),
                    Span::raw(msg.clone()),
                ]));
            }
            Status::Failed(msg) => {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:>13} ", "error"), red_bold),
                    Span::raw(msg.clone()),
                ]));
            }
        }
    }

    // Pad to fill viewport height so previous content is overwritten.
    while lines.len() < area.height as usize {
        lines.push(Line::raw(""));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, area);
}

// ── Public entry point ────────────────────────────────────────────────────────

pub enum BuildTarget {
    Project {
        profile: String,
        features: Vec<String>,
        use_defaults: bool,
        sanitize: Vec<String>,
    },
    Workspace {
        profile: String,
        package: Option<String>,
        features: Vec<String>,
        use_defaults: bool,
    },
}

/// Run a build with a ratatui inline viewport.  Returns `true` on success.
/// Falls back gracefully if the terminal can't be initialised.
pub fn run_build_viewport(target: BuildTarget) -> bool {
    let (tx, rx) = mpsc::channel::<BuildEvent>();
    let tx2 = tx.clone();
    let progress: Progress = Arc::new(move |ev| {
        let _ = tx2.send(ev);
    });

    // Sentinel: build thread drops tx when done, closing the channel.
    drop(tx);

    let handle: std::thread::JoinHandle<Result<Vec<BuildOutput>, FreightError>> =
        match &target {
            BuildTarget::Project { .. } => {
                let (profile, features, use_defaults, sanitize) = match target {
                    BuildTarget::Project {
                        profile,
                        features,
                        use_defaults,
                        sanitize,
                    } => (profile, features, use_defaults, sanitize),
                    _ => unreachable!(),
                };
                std::thread::spawn(move || {
                    build_project_with(&profile, &features, use_defaults, &sanitize, &progress)
                        .map(|o| vec![o])
                })
            }
            BuildTarget::Workspace { .. } => {
                let (profile, package, features, use_defaults) = match target {
                    BuildTarget::Workspace {
                        profile,
                        package,
                        features,
                        use_defaults,
                    } => (profile, package, features, use_defaults),
                    _ => unreachable!(),
                };
                std::thread::spawn(move || {
                    build_workspace_with(
                        &profile,
                        package.as_deref(),
                        &features,
                        use_defaults,
                        &progress,
                    )
                })
            }
        };

    // Set up inline terminal.  If anything fails fall back to non-TUI path.
    let mut terminal = match Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_HEIGHT),
        },
    ) {
        Ok(t) => t,
        Err(_) => return run_fallback(handle, rx),
    };

    let mut state = BuildViewState::new();

    loop {
        for ev in rx.try_iter() {
            state.handle(ev);
        }
        let _ = terminal.draw(|f| render(f, &state));
        if handle.is_finished() {
            break;
        }
        state.tick = state.tick.wrapping_add(1);
        std::thread::sleep(Duration::from_millis(50));
    }

    // Drain any events emitted just before the thread exited.
    for ev in rx.try_iter() {
        state.handle(ev);
    }
    let result = handle
        .join()
        .unwrap_or_else(|_| Err(FreightError::OptionError("build thread panicked".into())));
    match &result {
        Ok(outputs) => state.finish_ok(outputs),
        Err(e) => state.finish_err(e),
    }
    let _ = terminal.draw(|f| render(f, &state));
    let _ = io::stdout().write_all(b"\n");
    result.is_ok()
}

/// Plain-text fallback used when the terminal can't be set up (e.g. piped output).
fn run_fallback(
    handle: std::thread::JoinHandle<Result<Vec<BuildOutput>, FreightError>>,
    rx: mpsc::Receiver<BuildEvent>,
) -> bool {
    use crate::output::{print_error, print_status, print_success, print_warning};
    use owo_colors::OwoColorize;

    for ev in rx {
        match ev {
            BuildEvent::BuildStarted { name, profile } => {
                print_status("Building", &format!("{name} [{profile}]"));
            }
            BuildEvent::Compiling { path } => {
                print_status("Compiling", &path.display().to_string());
            }
            BuildEvent::Fresh { path } => {
                println!("{:>12} {}", "Fresh".dimmed(), path.display());
            }
            BuildEvent::Linking { name } => print_status("Linking", &name),
            BuildEvent::Archiving { name } => print_status("Archiving", &name),
            BuildEvent::DepBuildStarted { name } => {
                use std::io::Write;
                print!("{:>12} {name} ", "Building".bold().cyan());
                let _ = std::io::stdout().flush();
            }
            BuildEvent::DepCompiling => {
                use std::io::Write;
                print!("·");
                let _ = std::io::stdout().flush();
            }
            BuildEvent::DepBuildDone => println!(),
            BuildEvent::Warning(msg) => print_warning(&msg),
            _ => {}
        }
    }

    match handle.join().unwrap_or_else(|_| {
        Err(FreightError::OptionError("build thread panicked".into()))
    }) {
        Ok(outputs) => {
            println!();
            for o in &outputs {
                print_success(&format!(
                    "{} ({} compiled, {} up to date)",
                    o.package_name, o.compiled, o.skipped,
                ));
                for bin in &o.binaries {
                    println!("    {}", bin.display());
                }
            }
            true
        }
        Err(e) => {
            println!();
            print_error(&e.to_string());
            false
        }
    }
}
