//! Reusable ratatui widgets and layout helpers shared by all freight TUIs.
use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use super::theme::{self, FormStatus};

// ── Layout helpers ────────────────────────────────────────────────────────────

/// Return a centred `Rect` of the requested size inside `area`.
/// Clamps to the available terminal space so it never overflows.
pub fn center_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect { x, y, width: width.min(area.width), height: height.min(area.height) }
}

// ── Popup helpers ─────────────────────────────────────────────────────────────

/// Clear the popup area and render the outer border block.
/// Returns the inner area ready for content.
pub fn render_popup(frame: &mut Frame, title: &str, status: &FormStatus, popup: Rect) -> Rect {
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(status.border_color()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    inner
}

// ── Standard form widgets ─────────────────────────────────────────────────────

/// Render the keyboard hint line at the bottom of a form.
///
/// Shows "Please wait…" while loading, otherwise the standard navigation hint.
pub fn render_hint(frame: &mut Frame, area: Rect, status: &FormStatus) {
    let text = if matches!(status, FormStatus::Loading) {
        " Please wait…"
    } else {
        " Tab/↑↓ move  Enter next/submit  Esc cancel"
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(theme::COLOR_HINT)),
        area,
    );
}

/// Render the status message area below the input fields.
///
/// - On `Err`: shows the error string in red with word-wrap.
/// - On `Done`: shows `done_text` centred in bold green.
/// - Otherwise: renders nothing.
pub fn render_status(frame: &mut Frame, area: Rect, status: &FormStatus, done_text: &str) {
    match status {
        FormStatus::Err(e) => {
            frame.render_widget(
                Paragraph::new(e.as_str())
                    .style(theme::error_style())
                    .wrap(Wrap { trim: true }),
                area,
            );
        }
        FormStatus::Done => {
            frame.render_widget(
                Paragraph::new(done_text)
                    .style(theme::success_style())
                    .alignment(Alignment::Center),
                area,
            );
        }
        _ => {}
    }
}

/// Render a single labelled text input field.
///
/// `value` is the displayed text. `mask` controls whether to show bullet
/// characters instead of the actual content (for password fields).
pub fn render_field(
    frame:   &mut Frame,
    area:    Rect,
    label:   &str,
    value:   &str,
    mask:    bool,
    active:  bool,
    status:  &FormStatus,
) {
    let display: String = if mask {
        "•".repeat(value.chars().count())
    } else {
        value.to_string()
    };
    let border_style = if active && status.is_interactive() {
        Style::default().fg(theme::COLOR_ACTIVE)
    } else {
        Style::default()
    };
    frame.render_widget(
        Paragraph::new(display.as_str())
            .style(theme::input_style())
            .block(
                Block::default()
                    .title(label)
                    .borders(Borders::ALL)
                    .border_style(border_style),
            ),
        area,
    );
}
