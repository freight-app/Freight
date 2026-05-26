//! Terminal UI components for interactive freight commands.
//!
//! Current:
//!   - `freight add` (no args)  → package browser (search, select, add to freight.toml)
//!   - `freight tui`            → registry admin panel (packages, users, tokens, orgs, audit)
//!
//! TODO: add TUI for:
//!   - `freight outdated`       → interactive update picker
//!   - `freight tree`           → navigable dependency tree
//!   - `freight build`          → live build progress / log viewer
//!   - `freight test`           → test runner with live output panel

pub mod browser;
pub mod common;
pub mod login;
pub mod register;
#[cfg(feature = "admin")]
pub mod registry;

pub use browser::run_package_browser;
