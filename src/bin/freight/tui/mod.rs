//! Terminal UI components for interactive freight commands.
//!
//! Current:
//!   - `freight add` (no args)  → package browser (search, select, add to freight.toml)
//!
//! TODO: add TUI for:
//!   - `freight outdated`       → interactive update picker
//!   - `freight tree`           → navigable dependency tree
//!   - `freight build`          → live build progress / log viewer
//!   - `freight test`           → test runner with live output panel
//!   - registry admin           → user / audit log browser

pub mod browser;

pub use browser::run_package_browser;
