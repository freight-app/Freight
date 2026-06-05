//! Terminal UI components for interactive freight commands.
//!
//! TUI-enhanced commands:
//!   - `freight add`      → package browser (search, select, add to freight.toml)
//!   - `freight login`    → interactive login form
//!   - `freight register` → interactive registration form

pub mod browser;
pub mod build_view;
pub mod common;
pub mod login;
pub mod register;

pub use browser::run_package_browser;
pub use build_view::{run_build_viewport, BuildTarget};
