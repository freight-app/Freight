///! Interactive TUI registration form for `freight register`.
///!
///! Five fields: Registry URL / Username / Password / Confirm Password / Email (optional).
///! On submit calls POST /api/v1/users/register and saves the returned token.
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

struct RegisterForm {
    url:        String,
    username:   String,
    password:   String,
    confirm:    String,
    email:      String,
    field:      usize,   // 0=url 1=username 2=password 3=confirm 4=email
    status:     FormStatus,
    token_name: String,
}

enum Msg { Success(String /* token */), Err(String) }

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the register TUI.  Returns `(username, token)` on success.
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
    let first_empty = if username.is_none() { 1 } else { 2 };
    let mut form = RegisterForm {
        url,
        username:   username.unwrap_or_default(),
        password:   String::new(),
        confirm:    String::new(),
        email:      email.unwrap_or_default(),
        field:      first_empty,
        status:     FormStatus::Idle,
        token_name: token_name.unwrap_or_else(|| "init".to_string()),
    };

    let mut term = enter_tui()?;
    let result   = event_loop(&mut term, &mut form).await;
    leave_tui(&mut term)?;
    result
}

async fn event_loop(
    term: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    form: &mut RegisterForm,
) -> Result<(String, String)> {
    let (tx, mut rx) = mpsc::channel::<Msg>(4);

    loop {
        term.draw(|f| draw(f, form))?;

        if let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Success(token) => {
                    form.status = FormStatus::Done;
                    term.draw(|f| draw(f, form))?;
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    return Ok((form.username.clone(), token));
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
                KeyCode::Tab | KeyCode::Down   => { form.field = (form.field + 1) % 5; }
                KeyCode::BackTab | KeyCode::Up  => { form.field = (form.field + 4) % 5; }
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
                        // Validate
                        if form.username.is_empty() {
                            form.status = FormStatus::Err("username cannot be empty".into());
                            form.field = 1;
                            continue;
                        }
                        if form.password.len() < 8 {
                            form.status = FormStatus::Err("password must be at least 8 characters".into());
                            form.field = 2;
                            continue;
                        }
                        if form.password != form.confirm {
                            form.status = FormStatus::Err("passwords do not match".into());
                            form.field = 3;
                            continue;
                        }

                        let url        = form.url.clone();
                        let username   = form.username.clone();
                        let password   = form.password.clone();
                        let email      = if form.email.is_empty() { None } else { Some(form.email.clone()) };
                        let token_name = form.token_name.clone();
                        form.status    = FormStatus::Loading;
                        let tx2 = tx.clone();
                        tokio::spawn(async move {
                            match common::post_register(
                                &url, &username, &password,
                                email.as_deref(), Some(&token_name),
                            ).await {
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

    let title = match &form.status {
        FormStatus::Loading => " freight register — creating account… ",
        FormStatus::Done    => " freight register — ✓ registered! ",
        FormStatus::Err(_)  => " freight register — error ",
        FormStatus::Idle    => " freight register ",
    };

    let inner = render_popup(frame, title, &form.status, popup);

    let [url_a, usr_a, pw_a, cf_a, em_a, _, err_a, hint_a] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ]).areas(inner);

    render_field(frame, url_a, " Registry URL ",                        &form.url,      false, form.field == 0, &form.status);
    render_field(frame, usr_a, " Username ",                            &form.username, false, form.field == 1, &form.status);
    render_field(frame, pw_a,  " Password (min 8 chars) ",              &form.password, true,  form.field == 2, &form.status);
    render_field(frame, cf_a,  " Confirm Password ",                    &form.confirm,  true,  form.field == 3, &form.status);
    render_field(frame, em_a,  " Email (optional, Enter to submit) ",   &form.email,    false, form.field == 4, &form.status);

    let done_text = format!("Registered as '{}'  — token saved.", form.username);
    render_status(frame, err_a,  &form.status, &done_text);
    render_hint  (frame, hint_a, &form.status);
}
