///! Interactive TUI registration form for `freight register`.
///!
///! Five fields: Registry URL / Username / Password / Confirm Password / Email (optional).
///! On submit calls POST /api/v1/users/register and saves the returned token.
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
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use tokio::sync::mpsc;

use sha2::{Digest, Sha256};

// ── State ─────────────────────────────────────────────────────────────────────

enum Status { Idle, Loading, Done, Err(String) }

struct RegisterForm {
    url:      String,
    username: String,
    password: String,
    confirm:  String,
    email:    String,
    field:    usize,   // 0=url 1=username 2=password 3=confirm 4=email
    status:   Status,
    token_name: String,
}

enum Msg { Success(String /* token */), Err(String) }

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the register TUI.  Returns (username, token) on success.
pub fn run(
    url:        String,
    username:   Option<String>,
    email:      Option<String>,
    token_name: Option<String>,
) -> Result<(String, String)> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(url, username, email, token_name))
}

async fn run_async(
    url:        String,
    username:   Option<String>,
    email:      Option<String>,
    token_name: Option<String>,
) -> Result<(String, String)> {
    // Pre-fill provided args; cursor starts at first empty field.
    let first_empty = if username.is_none() { 1 } else { 2 };
    let mut form = RegisterForm {
        url,
        username:   username.unwrap_or_default(),
        password:   String::new(),
        confirm:    String::new(),
        email:      email.unwrap_or_default(),
        field:      first_empty,
        status:     Status::Idle,
        token_name: token_name.unwrap_or_else(|| "init".to_string()),
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
    form: &mut RegisterForm,
) -> Result<(String, String)> {
    let (tx, mut rx) = mpsc::channel::<Msg>(4);

    loop {
        term.draw(|f| draw(f, form))?;

        if let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Success(token) => {
                    form.status = Status::Done;
                    term.draw(|f| draw(f, form))?;
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    return Ok((form.username.clone(), token));
                }
                Msg::Err(e) => { form.status = Status::Err(e); }
            }
        }

        if !event::poll(Duration::from_millis(50))? { continue; }

        if let Event::Key(key) = event::read()? {
            if key.code == KeyCode::Esc
                || (key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c'))
            {
                anyhow::bail!("cancelled");
            }

            if matches!(form.status, Status::Loading) { continue; }

            match key.code {
                KeyCode::Tab | KeyCode::Down => {
                    form.field = (form.field + 1) % 5;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    form.field = (form.field + 4) % 5;
                }
                KeyCode::Char(c) => match form.field {
                    0 => form.url.push(c),
                    1 => form.username.push(c),
                    2 => form.password.push(c),
                    3 => form.confirm.push(c),
                    _ => form.email.push(c),
                },
                KeyCode::Backspace => match form.field {
                    0 => { form.url.pop(); }
                    1 => { form.username.pop(); }
                    2 => { form.password.pop(); }
                    3 => { form.confirm.pop(); }
                    _ => { form.email.pop(); }
                },
                KeyCode::Enter => {
                    if form.field < 4 {
                        form.field += 1;
                    } else {
                        // Validate then submit
                        if form.username.is_empty() {
                            form.status = Status::Err("username cannot be empty".into());
                            form.field = 1;
                            continue;
                        }
                        if form.password.len() < 8 {
                            form.status = Status::Err("password must be at least 8 characters".into());
                            form.field = 2;
                            continue;
                        }
                        if form.password != form.confirm {
                            form.status = Status::Err("passwords do not match".into());
                            form.field = 3;
                            continue;
                        }

                        let url        = form.url.clone();
                        let username   = form.username.clone();
                        let password   = form.password.clone();
                        let email      = if form.email.is_empty() { None } else { Some(form.email.clone()) };
                        let token_name = form.token_name.clone();
                        form.status    = Status::Loading;
                        let tx2 = tx.clone();
                        tokio::spawn(async move {
                            match http_register(&url, &username, &password, email.as_deref(), Some(&token_name)).await {
                                Ok(token) => { tx2.send(Msg::Success(token)).await.ok(); }
                                Err(e)    => { tx2.send(Msg::Err(e.to_string())).await.ok(); }
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

fn draw(frame: &mut Frame, form: &RegisterForm) {
    let area  = frame.area();
    let popup = center_rect(58, 24, area);
    frame.render_widget(Clear, popup);

    let (title, border_colour) = match &form.status {
        Status::Loading   => (" freight register — creating account… ", Color::Yellow),
        Status::Done      => (" freight register — ✓ registered! ",      Color::Green),
        Status::Err(_)    => (" freight register — error ",               Color::Red),
        Status::Idle      => (" freight register ",                       Color::Cyan),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(border_colour));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [url_a, usr_a, pw_a, cf_a, em_a, sp1, err_a, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
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
            .block(Block::default().title(" Password (min 8 chars) ").borders(Borders::ALL)
                .border_style(fs(2))),
        pw_a,
    );
    let cf_mask: String = "•".repeat(form.confirm.len());
    frame.render_widget(
        Paragraph::new(cf_mask.as_str())
            .block(Block::default().title(" Confirm Password ").borders(Borders::ALL)
                .border_style(fs(3))),
        cf_a,
    );
    frame.render_widget(
        Paragraph::new(form.email.as_str())
            .block(Block::default().title(" Email (optional, Enter to submit) ").borders(Borders::ALL)
                .border_style(fs(4))),
        em_a,
    );

    let _ = sp1;

    match &form.status {
        Status::Err(e) => {
            frame.render_widget(
                Paragraph::new(e.as_str())
                    .style(Style::default().fg(Color::Red))
                    .wrap(Wrap { trim: true }),
                err_a,
            );
        }
        Status::Done => {
            frame.render_widget(
                Paragraph::new(
                    Line::from(format!("Registered as '{}'  — token saved.", form.username))
                )
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

async fn http_register(
    url:        &str,
    username:   &str,
    password:   &str,
    email:      Option<&str>,
    token_name: Option<&str>,
) -> anyhow::Result<String> {
    let pw_hash: String = Sha256::digest(password.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let resp = reqwest::Client::new()
        .post(format!("{url}/api/v1/users/register"))
        .json(&serde_json::json!({
            "username":   username,
            "password":   pw_hash,
            "email":      email,
            "token_name": token_name,
        }))
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if let Some(t) = body["token"].as_str() {
        Ok(t.to_string())
    } else {
        let detail = body["errors"][0]["detail"].as_str().unwrap_or("registration failed");
        anyhow::bail!("{detail}")
    }
}
