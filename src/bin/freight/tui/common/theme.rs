//! Visual theme shared across all freight form TUIs.
//!
//! Defines the standard status lifecycle and consistent colour palette used by
//! login, register, and any future forms.
use ratatui::style::{Color, Modifier, Style};

// ── Colour palette ────────────────────────────────────────────────────────────

/// Border/title colour when the form is idle and ready for input.
pub const COLOR_IDLE:    Color = Color::Cyan;
/// Border/title colour while an async operation is in progress.
pub const COLOR_LOADING: Color = Color::Yellow;
/// Border/title colour on success.
pub const COLOR_DONE:    Color = Color::Green;
/// Border/title colour when an error has occurred.
pub const COLOR_ERR:     Color = Color::Red;
/// Colour used for the active input field border.
pub const COLOR_ACTIVE:  Color = Color::Yellow;
/// Colour used for hint/help text.
pub const COLOR_HINT:    Color = Color::DarkGray;

// ── Form status ───────────────────────────────────────────────────────────────

/// Lifecycle state shared by all form TUIs.
pub enum FormStatus {
    Idle,
    Loading,
    Done,
    Err(String),
}

impl FormStatus {
    /// Border colour for the outer popup block.
    pub fn border_color(&self) -> Color {
        match self {
            FormStatus::Idle    => COLOR_IDLE,
            FormStatus::Loading => COLOR_LOADING,
            FormStatus::Done    => COLOR_DONE,
            FormStatus::Err(_)  => COLOR_ERR,
        }
    }

    /// Returns `true` when the form accepts keyboard input
    /// (i.e. not loading and not finished).
    pub fn is_interactive(&self) -> bool {
        matches!(self, FormStatus::Idle | FormStatus::Err(_))
    }
}

/// Style for text inside an active input field.
pub fn input_style() -> Style {
    Style::default().fg(Color::White)
}

/// Style for success messages.
pub fn success_style() -> Style {
    Style::default().fg(COLOR_DONE).add_modifier(Modifier::BOLD)
}

/// Style for error messages.
pub fn error_style() -> Style {
    Style::default().fg(COLOR_ERR)
}
