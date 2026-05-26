mod app;
mod client;
mod config;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use app::{App, DataEvent};
use client::Client;

/// Launch the registry admin TUI (blocks the calling thread).
///
/// `url`   — registry base URL (e.g. `http://localhost:7878`)
/// `token` — optional pre-loaded API token; shows the login screen if absent
pub fn run(url: String, token: Option<String>) -> Result<()> {
    // Prefer CLI/env token; fall back to persisted config file.
    let (url, token) = match (token, config::TuiConfig::load()) {
        (Some(tok), _)        => (url, Some(tok)),
        (None, Some(cfg))     => (cfg.url, Some(cfg.token)),
        (None, None)          => (url, None),
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_async(url, token))
}

async fn run_async(url: String, token: Option<String>) -> Result<()> {
    let client  = Client::new(url.clone(), token);
    let mut app = App::new(client, url);

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend  = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let result = event_loop(&mut term, &mut app).await;

    // Restore terminal unconditionally
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;

    result
}

async fn event_loop(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app:  &mut App,
) -> Result<()> {
    let (data_tx, mut data_rx) = mpsc::channel::<DataEvent>(64);
    let (key_tx,  mut key_rx)  = mpsc::channel::<crossterm::event::KeyEvent>(32);

    // Blocking key-reader in a dedicated OS thread so we don't stall the tokio executor.
    let key_tx2 = key_tx.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(Event::Key(k)) = event::read() {
                    if key_tx2.blocking_send(k).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Initial data load.
    app.load_me(data_tx.clone());
    app.load_current_tab(data_tx.clone());

    loop {
        term.draw(|f| ui::draw(f, app))?;

        tokio::select! {
            key = key_rx.recv() => {
                if let Some(k) = key {
                    if app.handle_key(k, &data_tx) { break; }
                }
            }
            data = data_rx.recv() => {
                if let Some(d) = data { app.handle_data(d, &data_tx); }
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                // periodic redraw tick — updates spinner and relative timestamps
            }
        }
    }

    Ok(())
}
