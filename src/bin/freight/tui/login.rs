///! Interactive TUI login form for `freight login`.
///!
///! Presents a three-field form (Registry URL / Username / Password), calls
///! POST /api/v1/users/login, and saves the returned token to
///! ~/.freight/credentials.toml on success.
use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame, Terminal,
};
use tokio::sync::mpsc;

use sha2::{Digest, Sha256};

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum Status { Idle, Loading, Done, Err(String) }

struct LoginForm {
    url:      String,
    username: String,
    password: String,
    field:    usize,   // 0 = url, 1 = username, 2 = password
    status:   Status,
}

enum Msg { Success { username: String, token: String }, Err(String) }

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the login TUI.  Returns the saved token on success so the caller can
/// update credentials without a second file-read.
pub fn run(url: String, prefill_username: Option<String>) -> Result<(String, String)> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(url, prefill_username))
}

async fn run_async(url: String, prefill_username: Option<String>) -> Result<(String, String)> {
    let field_start = if prefill_username.is_none() { 1 } else { 2 };
    let mut form = LoginForm {
        url,
        username: prefill_username.unwrap_or_default(),
        password: String::new(),
        field:    field_start,
        status:   Status::Idle,
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend  = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = event_loop(&mut term, &mut form).await;

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;

    result
}

async fn event_loop(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    form: &mut LoginForm,
) -> Result<(String, String)> {
    let (tx, mut rx) = mpsc::channel::<Msg>(4);

    loop {
        term.draw(|f| draw(f, form))?;

        // Non-blocking msg poll first
        if let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Success { username, token } => {
                    form.status = Status::Done;
                    term.draw(|f| draw(f, form))?;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    return Ok((username, token));
                }
                Msg::Err(e) => {
                    form.status = Status::Err(e);
                }
            }
        }

        if !event::poll(Duration::from_millis(50))? { continue; }

        if let Event::Key(key) = event::read()? {
            // Ctrl-C / Esc → cancel
            if key.code == KeyCode::Esc
                || (key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c'))
            {
                anyhow::bail!("cancelled");
            }

            if matches!(form.status, Status::Loading) { continue; }

            match key.code {
                KeyCode::Tab | KeyCode::Down => {
                    form.field = (form.field + 1) % 3;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    form.field = (form.field + 2) % 3;
                }
                KeyCode::Char(c) => match form.field {
                    0 => form.url.push(c),
                    1 => form.username.push(c),
                    _ => form.password.push(c),
                },
                KeyCode::Backspace => match form.field {
                    0 => { form.url.pop(); }
                    1 => { form.username.pop(); }
                    _ => { form.password.pop(); }
                },
                KeyCode::Enter => {
                    if form.field < 2 {
                        form.field += 1;
                    } else {
                        // Submit
                        let url      = form.url.clone();
                        let username = form.username.clone();
                        let password = form.password.clone();
                        form.status  = Status::Loading;
                        let tx2 = tx.clone();
                        tokio::spawn(async move {
                            match http_login(&url, &username, &password).await {
                                Ok(token) => {
                                    tx2.send(Msg::Success { username, token }).await.ok();
                                }
                                Err(e) => {
                                    tx2.send(Msg::Err(e.to_string())).await.ok();
                                }
                            }
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn draw(frame: &mut Frame, form: &LoginForm) {
    let area   = frame.area();
    let popup  = center_rect(56, 18, area);
    frame.render_widget(Clear, popup);

    let title = match &form.status {
        Status::Loading   => " freight login — authenticating… ",
        Status::Done      => " freight login — ✓ success ",
        Status::Err(_)    => " freight login — error ",
        Status::Idle      => " freight login ",
    };
    let border_colour = match &form.status {
        Status::Done   => Color::Green,
        Status::Err(_) => Color::Red,
        Status::Loading => Color::Yellow,
        Status::Idle   => Color::Cyan,
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(border_colour));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [url_a, usr_a, pw_a, sp1, err_a, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ]).areas(inner);

    let fs = |idx: usize| {
        if form.field == idx && !matches!(form.status, Status::Loading | Status::Done) {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        }
    };

    frame.render_widget(
        Paragraph::new(form.url.as_str())
            .block(Block::default().title(" Registry URL ").borders(Borders::ALL)
                .border_style(fs(0))),
        url_a,
    );
    frame.render_widget(
        Paragraph::new(form.username.as_str())
            .block(Block::default().title(" Username ").borders(Borders::ALL)
                .border_style(fs(1))),
        usr_a,
    );
    let pw_mask: String = "•".repeat(form.password.len());
    frame.render_widget(
        Paragraph::new(pw_mask.as_str())
            .block(Block::default().title(" Password ").borders(Borders::ALL)
                .border_style(fs(2))),
        pw_a,
    );

    let _ = sp1; // spacer

    match &form.status {
        Status::Err(e) => {
            frame.render_widget(
                Paragraph::new(e.as_str())
                    .style(Style::default().fg(Color::Red))
                    .wrap(ratatui::widgets::Wrap { trim: true }),
                err_a,
            );
        }
        Status::Done => {
            frame.render_widget(
                Paragraph::new("Logged in — token saved.")
                    .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
                    .alignment(Alignment::Center),
                err_a,
            );
        }
        _ => {}
    }

    let hint = if matches!(form.status, Status::Loading) {
        " Please wait…"
    } else {
        " Tab/↑↓ move  Enter next/submit  Esc cancel"
    };
    frame.render_widget(
        Paragraph::new(hint)
            .style(Style::default().fg(Color::DarkGray)),
        hint_a,
    );
}

fn center_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect { x, y, width: width.min(area.width), height: height.min(area.height) }
}

// ── HTTP helper (no dep on tui::registry) ────────────────────────────────────

async fn http_login(url: &str, username: &str, password: &str) -> anyhow::Result<String> {
    let pw_hash: String = Sha256::digest(password.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let resp = reqwest::Client::new()
        .post(format!("{url}/api/v1/users/login"))
        .json(&serde_json::json!({ "username": username, "password": pw_hash }))
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if let Some(t) = body["token"].as_str() {
        Ok(t.to_string())
    } else {
        let detail = body["errors"][0]["detail"].as_str().unwrap_or("login failed");
        anyhow::bail!("{detail}")
    }
}
