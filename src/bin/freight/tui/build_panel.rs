//! Live build-progress panel for `freight build --panel`.
//!
//! Runs `build_project_with` (or `build_workspace_with`) on a background
//! thread and feeds every [`BuildEvent`] through a sync channel to the TUI
//! event loop on the main thread.
//!
//! Layout:
//! ```text
//! ╭─ freight build [dev] ───────────────────────────── 0:03  ⠋ ─╮
//! │  Resolving  zlib (pkg-config)                                │
//! │  Compiling  src/main.cpp                                     │
//! │  Fresh      src/utils.cpp                                    │
//! │  Linking    myapp                                            │
//! ╰─────────── 3 compiled · 2 fresh · 0 ⚠  ↑/↓ scroll  q quit ─╯
//! ```

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Frame,
    layout::Alignment,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem},
};

use freight_core::build::{build_project_with, build_workspace_with, BuildOutput};
use freight_core::event::{BuildEvent, Progress};

// ── Messages from build thread ────────────────────────────────────────────────

enum BuildMsg {
    Event(BuildEvent),
    WorkspaceSuccess(Vec<BuildOutput>),
    ProjectSuccess(BuildOutput),
    Failed(String),
}

// ── Panel state ───────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum Phase {
    Building,
    Done,
    Failed,
}

struct StampedLine {
    /// Elapsed milliseconds since the build started (used for future timestamps).
    #[allow(dead_code)]
    elapsed_ms: u64,
    event: BuildEvent,
}

struct Panel {
    lines:       Vec<StampedLine>,
    /// First visible line index (0-based).
    scroll:      usize,
    /// When true the view tracks the bottom of the log automatically.
    auto_scroll: bool,
    phase:       Phase,
    started:     Instant,
    compiled:    usize,
    fresh:       usize,
    warnings:    usize,
    profile:     String,
    outputs:     Vec<BuildOutput>,
    error_msg:   Option<String>,
    spinner_idx: usize,
}

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Panel {
    fn new(profile: &str) -> Self {
        Self {
            lines:       Vec::new(),
            scroll:      0,
            auto_scroll: true,
            phase:       Phase::Building,
            started:     Instant::now(),
            compiled:    0,
            fresh:       0,
            warnings:    0,
            profile:     profile.to_string(),
            outputs:     Vec::new(),
            error_msg:   None,
            spinner_idx: 0,
        }
    }

    fn elapsed_str(&self) -> String {
        let secs = self.started.elapsed().as_secs();
        format!("{}:{:02}", secs / 60, secs % 60)
    }

    fn push_event(&mut self, ev: BuildEvent) {
        match &ev {
            BuildEvent::Compiling { .. } => self.compiled += 1,
            BuildEvent::Fresh { .. }     => self.fresh    += 1,
            BuildEvent::Warning(_)       => self.warnings += 1,
            _ => {}
        }
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        self.lines.push(StampedLine { elapsed_ms, event: ev });
        if self.auto_scroll {
            self.scroll = usize::MAX; // clamped in draw()
        }
    }

    fn scroll_up(&mut self, n: usize) {
        self.auto_scroll = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: usize, view_height: usize) {
        let max = self.lines.len().saturating_sub(view_height);
        let new = self.scroll.saturating_add(n).min(max);
        if new >= max {
            self.auto_scroll = true;
        }
        self.scroll = new;
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the build panel.  Returns an exit code: 0 = success, 1 = build failed,
/// 130 = quit while building (SIGINT-style).
pub fn run(
    profile:        &str,
    features:       Vec<String>,
    use_defaults:   bool,
    sanitize:       Vec<String>,
    workspace_mode: bool,
    package:        Option<String>,
) -> i32 {
    let (tx, rx) = mpsc::sync_channel::<BuildMsg>(512);

    let profile_owned = profile.to_string();

    // ── Spawn the build on a background thread ────────────────────────────────
    std::thread::spawn(move || {
        let tx_ev = tx.clone();
        let progress: Progress = Arc::new(move |event| {
            let _ = tx_ev.send(BuildMsg::Event(event));
        });

        if workspace_mode {
            match build_workspace_with(
                &profile_owned,
                package.as_deref(),
                &features,
                use_defaults,
                &progress,
            ) {
                Ok(outputs) => { let _ = tx.send(BuildMsg::WorkspaceSuccess(outputs)); }
                Err(e)      => { let _ = tx.send(BuildMsg::Failed(e.to_string())); }
            }
        } else {
            match build_project_with(
                &profile_owned,
                &features,
                use_defaults,
                &sanitize,
                &progress,
            ) {
                Ok(output) => { let _ = tx.send(BuildMsg::ProjectSuccess(output)); }
                Err(e)     => { let _ = tx.send(BuildMsg::Failed(e.to_string())); }
            }
        }
    });

    // ── Set up terminal ───────────────────────────────────────────────────────
    let mut term = match super::common::term::enter_tui() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("TUI init failed: {e}");
            return 1;
        }
    };

    let mut panel       = Panel::new(profile);
    let mut tick        = Instant::now();
    let mut view_height = 20usize;

    let exit_code = loop {
        // ── Drain all pending build messages (non-blocking) ───────────────────
        loop {
            match rx.try_recv() {
                Ok(BuildMsg::Event(ev)) => panel.push_event(ev),
                Ok(BuildMsg::ProjectSuccess(out)) => {
                    panel.outputs     = vec![out];
                    panel.phase       = Phase::Done;
                    panel.auto_scroll = true;
                    panel.scroll      = usize::MAX;
                }
                Ok(BuildMsg::WorkspaceSuccess(outs)) => {
                    panel.outputs     = outs;
                    panel.phase       = Phase::Done;
                    panel.auto_scroll = true;
                    panel.scroll      = usize::MAX;
                }
                Ok(BuildMsg::Failed(msg)) => {
                    panel.error_msg = Some(msg);
                    panel.phase     = Phase::Failed;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if panel.phase == Phase::Building {
                        panel.phase     = Phase::Failed;
                        panel.error_msg = Some("build thread panicked".to_string());
                    }
                    break;
                }
            }
        }

        // Advance spinner
        if tick.elapsed() >= Duration::from_millis(80) {
            panel.spinner_idx = (panel.spinner_idx + 1) % SPINNER.len();
            tick = Instant::now();
        }

        // ── Draw ──────────────────────────────────────────────────────────────
        let _ = term.draw(|f| { view_height = draw(f, &mut panel); });

        // ── Input (very short poll while building; relaxed when done) ─────────
        let timeout = if panel.phase == Phase::Building {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(200)
        };

        if event::poll(timeout).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            break if panel.phase == Phase::Done { 0 } else { 130 };
                        }
                        KeyCode::Up | KeyCode::Char('k')   => panel.scroll_up(1),
                        KeyCode::Down | KeyCode::Char('j') => panel.scroll_down(1, view_height),
                        KeyCode::PageUp                    => panel.scroll_up(view_height.saturating_sub(2)),
                        KeyCode::PageDown                  => panel.scroll_down(view_height.saturating_sub(2), view_height),
                        KeyCode::Home | KeyCode::Char('g') => {
                            panel.auto_scroll = false;
                            panel.scroll      = 0;
                        }
                        KeyCode::End | KeyCode::Char('G')  => {
                            panel.auto_scroll = true;
                            panel.scroll      = usize::MAX;
                        }
                        _ => {
                            // any key exits once the build is finished
                            if panel.phase != Phase::Building {
                                break if panel.phase == Phase::Done { 0 } else { 1 };
                            }
                        }
                    }
                }
            }
        }
    };

    let _ = super::common::term::leave_tui(&mut term);
    exit_code
}

// ── Draw ──────────────────────────────────────────────────────────────────────

/// Renders the panel and returns the inner view height for scroll calculations.
fn draw(f: &mut Frame, panel: &mut Panel) -> usize {
    let area = f.area();

    let border_color = match panel.phase {
        Phase::Building => Color::Cyan,
        Phase::Done     => Color::Green,
        Phase::Failed   => Color::Red,
    };

    // Right-aligned status (spinner / elapsed / result)
    let right_title = match panel.phase {
        Phase::Building => format!(
            " {} {} ",
            panel.elapsed_str(),
            SPINNER[panel.spinner_idx % SPINNER.len()],
        ),
        Phase::Done    => format!(" ✓  {} ", panel.elapsed_str()),
        Phase::Failed  => " ✗ failed ".to_string(),
    };

    let bottom_left = format!(
        " {} compiled · {} fresh · {} ⚠  ",
        panel.compiled, panel.fresh, panel.warnings,
    );
    let bottom_right = if panel.phase == Phase::Building {
        " ↑/↓ scroll  q quit "
    } else {
        " any key to close "
    };

    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(" freight build ", Style::new().bold()),
            Span::styled(
                format!("[{}] ", panel.profile),
                Style::new().fg(Color::DarkGray),
            ),
        ]))
        .title(
            Line::from(Span::styled(right_title, Style::new().fg(border_color).bold()))
                .alignment(Alignment::Right),
        )
        .title_bottom(Line::from(vec![
            Span::styled(bottom_left, Style::new().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(bottom_right, Style::new().fg(Color::DarkGray)),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let view_height = inner.height as usize;

    // ── Assemble all log lines ────────────────────────────────────────────────
    let mut items: Vec<Line<'static>> = Vec::with_capacity(panel.lines.len() + 8);
    for sl in &panel.lines {
        items.push(event_to_line(&sl.event));
    }

    // Append success summary
    if panel.phase == Phase::Done {
        items.push(Line::from(""));
        for out in &panel.outputs {
            items.push(Line::from(vec![
                Span::styled("   ✓  ", Style::new().fg(Color::Green).bold()),
                Span::styled(out.package_name.clone(), Style::new().bold()),
                Span::styled(
                    format!("  ({} compiled, {} up to date)", out.compiled, out.skipped),
                    Style::new().fg(Color::DarkGray),
                ),
            ]));
            for bin in &out.binaries {
                items.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(bin.display().to_string(), Style::new().fg(Color::Blue)),
                ]));
            }
        }
    }

    // Append failure detail
    if panel.phase == Phase::Failed {
        items.push(Line::from(""));
        if let Some(ref msg) = panel.error_msg {
            for l in msg.lines() {
                items.push(Line::from(vec![Span::styled(
                    l.to_string(),
                    Style::new().fg(Color::Red),
                )]));
            }
        }
        items.push(Line::from(""));
    }

    // ── Clamp and apply scroll ────────────────────────────────────────────────
    let total    = items.len();
    let max_scroll = total.saturating_sub(view_height);

    if panel.auto_scroll {
        panel.scroll = max_scroll;
    } else {
        panel.scroll = panel.scroll.min(max_scroll);
    }

    let start   = panel.scroll.min(total);
    let end     = (start + view_height).min(total);
    let visible: Vec<ListItem<'static>> = items[start..end]
        .iter()
        .map(|l| ListItem::new(l.clone()))
        .collect();

    f.render_widget(List::new(visible), inner);

    // Scroll indicator: dim chevrons when not at extremes
    if total > view_height && inner.width > 4 {
        if panel.scroll > 0 {
            let ind = Line::from(Span::styled("  ↑ ", Style::new().fg(Color::DarkGray)));
            f.render_widget(
                ratatui::widgets::Paragraph::new(ind),
                ratatui::layout::Rect {
                    x: inner.x + inner.width.saturating_sub(5),
                    y: inner.y,
                    width: 4,
                    height: 1,
                },
            );
        }
        if panel.scroll < max_scroll {
            let ind = Line::from(Span::styled("  ↓ ", Style::new().fg(Color::DarkGray)));
            f.render_widget(
                ratatui::widgets::Paragraph::new(ind),
                ratatui::layout::Rect {
                    x: inner.x + inner.width.saturating_sub(5),
                    y: inner.y + inner.height.saturating_sub(1),
                    width: 4,
                    height: 1,
                },
            );
        }
    }

    view_height
}

// ── Event → styled line ───────────────────────────────────────────────────────

fn event_to_line(ev: &BuildEvent) -> Line<'static> {
    const W: usize = 12; // label column width (matches CLI output)

    match ev {
        BuildEvent::BuildStarted { name, profile } => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Building"),
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(name.clone(), Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(format!(" [{profile}]"), Style::new().fg(Color::DarkGray)),
        ]),
        BuildEvent::Compiling { path } => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Compiling"),
                Style::new().fg(Color::Cyan),
            ),
            Span::raw(path.display().to_string()),
        ]),
        BuildEvent::Fresh { path } => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Fresh"),
                Style::new().fg(Color::DarkGray),
            ),
            Span::styled(
                path.display().to_string(),
                Style::new().fg(Color::DarkGray),
            ),
        ]),
        BuildEvent::Linking { name } => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Linking"),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(name.clone(), Style::new().add_modifier(Modifier::BOLD)),
        ]),
        BuildEvent::Archiving { name } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "Archiving"), Style::new().fg(Color::Yellow)),
            Span::raw(name.clone()),
        ]),
        BuildEvent::FetchingDep { name, source } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "Fetching"), Style::new().fg(Color::Blue)),
            Span::raw(name.clone()),
            Span::styled(format!(" ({source})"), Style::new().fg(Color::DarkGray)),
        ]),
        BuildEvent::ResolvingDep { name, via } => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Resolving"),
                Style::new().fg(Color::DarkGray),
            ),
            Span::raw(name.clone()),
            Span::styled(format!(" ({via})"), Style::new().fg(Color::DarkGray)),
        ]),
        BuildEvent::BuildingForeignDep { name, backend } => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Building"),
                Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
            ),
            Span::raw(name.clone()),
            Span::styled(format!(" ({backend})"), Style::new().fg(Color::DarkGray)),
        ]),
        BuildEvent::Warning(msg) => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Warning"),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(msg.clone(), Style::new().fg(Color::Yellow)),
        ]),
        BuildEvent::RunningScript { cached } => {
            let suffix = if *cached { " (cached)" } else { "" };
            Line::from(vec![
                Span::styled(
                    format!("{:>W$}  ", "Running"),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("build script{suffix}"),
                    Style::new().fg(Color::DarkGray),
                ),
            ])
        }
        BuildEvent::TestLinking { name } | BuildEvent::BenchLinking { name } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "Linking"), Style::new().fg(Color::Yellow)),
            Span::raw(name.clone()),
        ]),
        BuildEvent::TestRunning { name } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "Running"), Style::new().fg(Color::Cyan)),
            Span::raw(name.clone()),
        ]),
        BuildEvent::TestResult { name, passed } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "test"), Style::new().fg(Color::DarkGray)),
            Span::raw(name.clone()),
            if *passed {
                Span::styled(" ... ok", Style::new().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else {
                Span::styled(
                    " ... FAILED",
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                )
            },
        ]),
        BuildEvent::BenchRunning { name } => Line::from(vec![
            Span::styled(
                format!("{:>W$}  ", "Benchmarking"),
                Style::new().fg(Color::Cyan),
            ),
            Span::raw(name.clone()),
        ]),
        BuildEvent::BenchResult { name, mean_ns } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "bench"), Style::new().fg(Color::Cyan)),
            Span::raw(name.clone()),
            Span::styled(format!("  {}", fmt_ns(*mean_ns)), Style::new().fg(Color::DarkGray)),
        ]),
        BuildEvent::EmittedAsm { path } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "Emitted"), Style::new().fg(Color::DarkGray)),
            Span::styled(
                path.display().to_string(),
                Style::new().fg(Color::DarkGray),
            ),
        ]),
        BuildEvent::Timing { path, ns } => Line::from(vec![
            Span::styled(format!("{:>W$}  ", "Timed"), Style::new().fg(Color::DarkGray)),
            Span::styled(
                path.display().to_string(),
                Style::new().fg(Color::DarkGray),
            ),
            Span::styled(format!("  {}", fmt_ns(*ns)), Style::new().fg(Color::DarkGray)),
        ]),
    }
}

fn fmt_ns(ns: u64) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.3} s", ns as f64 / 1e9)
    } else if ns >= 1_000_000 {
        format!("{:.3} ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.3} µs", ns as f64 / 1e3)
    } else {
        format!("{ns} ns")
    }
}
