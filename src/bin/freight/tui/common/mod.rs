//! Shared infrastructure for all freight TUI screens.
//!
//! ```text
//! common/
//!   term.rs    — terminal enter/leave helpers
//!   theme.rs   — FormStatus and colour palette
//!   widgets.rs — center_rect(), render_popup(), render_hint(),
//!                render_status(), render_field()
//!   http.rs    — sha256_hex(), post_login(), post_register()
//! ```

pub mod http;
pub mod term;
pub mod theme;
pub mod widgets;

// Flat re-exports for convenient use in sibling TUI modules.
pub use http::{post_login, post_register};
pub use term::{enter_tui, leave_tui};
pub use theme::FormStatus;
pub use widgets::{render_field, render_hint, render_popup, render_status};
