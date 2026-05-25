//! CLI command shells. Every function here reads cwd / parses CLI args /
//! prints results, then delegates to a pure function in `freight-core`.

pub mod build;
pub mod check;
pub mod compile_commands;
pub mod debug;
pub mod deps;
pub mod doc;
pub mod fmt;
pub mod install;
pub mod lint;
pub mod new;
pub mod migrate;
pub mod toolchain;
