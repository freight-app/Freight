//! Tracing subscriber that forwards log events to the LSP client as
//! `window/logMessage` notifications, visible in VS Code's Output panel.
//!
//! Activate by setting `FREIGHT_LOG` (e.g. `FREIGHT_LOG=debug`).
//! Supported levels: `error`, `warn`, `info`, `debug`, `trace`.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use serde_json::json;
use tracing::Level;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

// ---------------------------------------------------------------------------
// Public init
// ---------------------------------------------------------------------------

/// Initialise the global tracing subscriber for `freight lsp`.
///
/// When `FREIGHT_LOG` is unset or empty the subscriber is a no-op (zero
/// runtime cost).  When set (e.g. `FREIGHT_LOG=debug`) every log event is
/// forwarded to VS Code as a `window/logMessage` notification.
pub fn init_lsp_logging(out: Arc<Mutex<io::Stdout>>) {
    let level = match std::env::var("FREIGHT_LOG")
        .ok()
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => LevelFilter::TRACE,
        "debug" => LevelFilter::DEBUG,
        "info"  => LevelFilter::INFO,
        "warn"  => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        _ => return, // not set → no logging
    };

    let layer = LspLogLayer { out }.with_filter(level);
    let _ = tracing_subscriber::registry().with(layer).try_init();
}

/// Initialise a plain stderr subscriber for non-LSP commands.
///
/// Active when `FREIGHT_LOG` is set; output goes to stderr so it appears in
/// the terminal (or in VS Code's integrated terminal).
pub fn init_stderr_logging() {
    if std::env::var("FREIGHT_LOG").map_or(true, |v| v.is_empty()) {
        return;
    }
    let filter = tracing_subscriber::EnvFilter::from_env("FREIGHT_LOG");
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_target(false)
        .try_init();
}

// ---------------------------------------------------------------------------
// LspLogLayer
// ---------------------------------------------------------------------------

struct LspLogLayer {
    out: Arc<Mutex<io::Stdout>>,
}

impl<S: tracing::Subscriber> Layer<S> for LspLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = *event.metadata().level();
        let msg_type = match level {
            Level::ERROR => 1u32,
            Level::WARN  => 2,
            Level::INFO  => 3,
            _            => 4, // DEBUG + TRACE → Log
        };

        let mut visitor = FieldCollector::default();
        event.record(&mut visitor);

        let target = event.metadata().target();
        let msg = if visitor.extra.is_empty() {
            format!("[freight/{target}] {}", visitor.message)
        } else {
            format!("[freight/{target}] {} — {}", visitor.message, visitor.extra)
        };

        let notification = json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": msg_type, "message": msg }
        });

        let body = match serde_json::to_vec(&notification) {
            Ok(b) => b,
            Err(_) => return,
        };

        // Use try_lock to avoid deadlocks if logging happens while `out` is held.
        if let Ok(mut out) = self.out.try_lock() {
            let _ = write!(out, "Content-Length: {}\r\n\r\n", body.len());
            let _ = out.write_all(&body);
            let _ = out.flush();
        }
    }
}

// ---------------------------------------------------------------------------
// Field visitor
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FieldCollector {
    message: String,
    extra: String,
}

impl tracing::field::Visit for FieldCollector {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            push_extra(&mut self.extra, field.name(), value);
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        if field.name() == "message" {
            // Strip surrounding quotes that Debug adds for &str
            self.message = s.trim_matches('"').to_string();
        } else {
            push_extra(&mut self.extra, field.name(), &s);
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        push_extra(&mut self.extra, field.name(), &value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        push_extra(&mut self.extra, field.name(), &value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        push_extra(&mut self.extra, field.name(), &value.to_string());
    }
}

fn push_extra(buf: &mut String, key: &str, val: &str) {
    if !buf.is_empty() {
        buf.push_str(", ");
    }
    buf.push_str(key);
    buf.push('=');
    buf.push_str(val);
}
