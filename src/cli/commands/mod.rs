//! CLI command shells. Every module here owns its `pub struct Args` (clap
//! definition) and `impl Args { pub fn run(self) }` (implementation).
//! `main.rs` dispatches to `args.run()` and nothing else.

pub mod add;
pub mod admin;
pub mod bench;
pub mod build;
pub mod check;
pub mod clean;
pub mod common;
pub mod compile_commands;
pub mod completions;
pub mod debug;
pub mod doc;
pub mod fetch;
pub mod fmt;
pub mod info;
pub mod install;
pub mod lint;
pub mod login;
pub mod logout;
pub mod metadata;
pub mod migrate;
pub mod new;
pub mod outdated;
pub mod publish;
pub mod register;
pub mod remove;
pub mod run;
pub mod search;
pub mod test;
pub mod toolchain;
pub mod tree;
pub mod update;
pub mod watch;
pub mod workspace;
pub mod yank;

// Keep deps.rs around until it is empty — it is referenced nowhere now.
// Delete it after `cargo check` passes cleanly.
