//! Terminal UI components for interactive freight commands.
//!
//! Current:
//!   - `freight add` (no args)     → package browser (search, select, add to freight.toml)
//!   - `freight tui`               → registry admin panel (packages, users, tokens, orgs, audit)
//!   - `freight build --panel`     → live build-progress log panel
//!
//! TODO: add TUI for:
//!   - `freight outdated`       → interactive update picker
//!   - `freight tree`           → navigable dependency tree
//!   - `freight test`           → test runner with live output panel

pub mod browser;
pub mod build_panel;
pub mod common;
pub mod login;
pub mod register;
#[cfg(feature = "admin")]
pub mod registry;

pub use browser::run_package_browser;
