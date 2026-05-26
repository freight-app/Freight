///! Interactive TUI login form for `freight login`.
///!
///! Presents a three-field form (Registry URL / Username / Password), calls
///! POST /api/v1/users/login, and saves the returned token to
///! ~/.freight/credentials.toml on success.
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout},
    Frame, Terminal,
};
use tokio::sync::mpsc;

use super::common::{
    self, enter_tui, leave_tui, render_field, render_hint, render_popup, render_status,
    FormStatus,
};
use super::common::widgets::center_rect;

// ── State ─────────────────────────────────────────────────────────────────────

struct LoginForm {
    url:      String,
    username: String,
    password: String,
    field:    usize,   // 0 = url, 1 = username, 2 = password
    status:   FormStatus,
}

enum Msg { Success { username: String, token: String }, Err(String) }

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the login TUI.  Returns `(username, token)` on success.
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
        status:   FormStatus::Idle,
    };

    let mut term = enter_tui()?;
    let result   = event_loop(&mut term, &mut form).await;
    leave_tui(&mut term)?;
    result
}

async fn event_loop(
    term: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    form: &mut LoginForm,
) -> Result<(String, String)> {
    let (tx, mut rx) = mpsc::channel::<Msg>(4);

    loop {
        term.draw(|f| draw(f, form))?;

        if let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Success { username, token } => {
                    form.status = FormStatus::Done;
                    term.draw(|f| draw(f, form))?;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    return Ok((username, token));
                }
                Msg::Err(e) => { form.status = FormStatus::Err(e); }
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

            if !form.status.is_interactive() { continue; }

            match key.code {
                KeyCode::Tab | KeyCode::Down  => { form.field = (form.field + 1) % 3; }
                KeyCode::BackTab | KeyCode::Up => { form.field = (form.field + 2) % 3; }
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
                        let url      = form.url.clone();
                        let username = form.username.clone();
                        let password = form.password.clone();
                        form.status  = FormStatus::Loading;
                        let tx2 = tx.clone();
                        tokio::spawn(async move {
                            match common::post_login(&url, &username, &password).await {
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
    let area  = frame.area();
    let popup = center_rect(56, 18, area);

    let title = match &form.status {
        FormStatus::Loading => " freight login — authenticating… ",
        FormStatus::Done    => " freight login — ✓ success ",
        FormStatus::Err(_)  => " freight login — error ",
        FormStatus::Idle    => " freight login ",
    };

    let inner = render_popup(frame, title, &form.status, popup);

    let [url_a, usr_a, pw_a, _, err_a, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ]).areas(inner);

    render_field(frame, url_a, " Registry URL ", &form.url,      false, form.field == 0, &form.status);
    render_field(frame, usr_a, " Username ",     &form.username, false, form.field == 1, &form.status);
    render_field(frame, pw_a,  " Password ",     &form.password, true,  form.field == 2, &form.status);

    render_status(frame, err_a,  &form.status, "Logged in — token saved.");
    render_hint  (frame, hint_a, &form.status);
}
