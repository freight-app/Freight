//! CLI command shells. Every function here reads cwd / parses CLI args /
//! prints results, then delegates to a pure function in `crane-core`.

pub mod build;
pub mod check;
pub mod compile_commands;
pub mod deps;
pub mod migrate;
pub mod new;
pub mod toolchain;
