mod commands;
mod completion;
mod output;
mod tui;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use crate::completion::{print_completion_candidates, CompletionContext};

/// Returns the top-level [`clap::Command`] for this binary.
pub(crate) fn cli_command() -> clap::Command {
    Cli::command()
}

#[derive(Parser)]
#[command(
    name = "freight",
    about = "Build tool and package manager for C, C++, Fortran, and more"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scaffold a new project
    New(commands::new::Args),
    /// Initialize freight in the current directory
    Init(commands::new::InitArgs),
    /// Build the project
    Build(commands::build::Args),
    /// Build and run the default binary
    Run(commands::run::Args),
    /// Build and run benchmarks in benches/
    Bench(commands::bench::Args),
    /// Build and run tests
    Test(commands::test::Args),
    /// Build (debug) and launch an interactive debugger session
    Debug(commands::debug::Args),
    /// Start Freight's Debug Adapter Protocol server over stdio
    Dap(commands::dap::Args),
    /// Watch source files and rebuild on changes
    Watch(commands::watch::Args),
    /// Add a dependency
    Add(commands::add::Args),
    /// Remove a dependency
    Remove(commands::remove::Args),
    /// Update dependencies within semver ranges
    Update(commands::update::Args),
    /// Download dependencies without building
    Fetch(commands::fetch::Args),
    /// Print the dependency tree, or the source/header include tree with --sources
    Tree(commands::tree::Args),
    /// Show outdated registry dependencies
    Outdated(commands::outdated::Args),
    /// Show package metadata (from registry when a name is given, or the current project)
    Info(commands::info::Args),
    /// Search the package registry
    Search(commands::search::Args),
    /// Validate freight.toml
    Check(commands::check::Args),
    /// Wipe target/
    Clean(commands::clean::Args),
    /// Generate compile_commands.json for clangd, fortls, serve-d and other language servers
    #[command(visible_alias = "compile-commands")]
    Compile(commands::compile_commands::Args),
    /// Open the dependency documentation browser, or generate API docs / man pages
    Doc(commands::doc::Args),
    /// Install build outputs to a system prefix (binaries, libs, headers)
    Install(commands::install::Args),
    /// Build and pack outputs into a redistributable archive (.tar.gz, or .zip for Windows targets)
    Package(commands::install::PackageArgs),
    /// Generate shell completion scripts
    #[command(visible_alias = "completion")]
    Completions(commands::completions::Args),
    /// Authenticate with a registry and save the token
    Login(commands::login::Args),
    /// Upload this package to a registry
    Publish(commands::publish::Args),
    /// Register a new account on a registry
    Register(commands::register::Args),
    /// Yank a published version (prevents new installs)
    Yank(commands::yank::Args),
    /// Format source files
    Fmt(commands::fmt::Args),
    /// Lint source files
    Lint(commands::lint::Args),
    /// Import a project from another build system into freight
    Migrate(commands::migrate::Args),
    /// Start the freight.toml language server and clangd passthrough
    Lsp(commands::lsp::Args),
    /// Manage compiler toolchains
    Toolchain(commands::toolchain::Args),
    /// Internal helper used by generated shell completion scripts
    #[command(name = "__complete", hide = true)]
    Complete { context: CompletionContext },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::New(args) => args.run(),
        Commands::Init(args) => args.run(),
        Commands::Build(args) => args.run(),
        Commands::Run(args) => args.run(),
        Commands::Bench(args) => args.run(),
        Commands::Test(args) => args.run(),
        Commands::Debug(args) => args.run(),
        Commands::Dap(args) => args.run(),
        Commands::Watch(args) => args.run(),
        Commands::Add(args) => args.run(),
        Commands::Remove(args) => args.run(),
        Commands::Update(args) => args.run(),
        Commands::Fetch(args) => args.run(),
        Commands::Tree(args) => args.run(),
        Commands::Outdated(args) => args.run(),
        Commands::Info(args) => args.run(),
        Commands::Search(args) => args.run(),
        Commands::Check(args) => args.run(),
        Commands::Clean(args) => args.run(),
        Commands::Compile(args) => args.run(),
        Commands::Doc(args) => args.run(),
        Commands::Install(args) => args.run(),
        Commands::Package(args) => args.run(),
        Commands::Completions(args) => {
            let mut cmd = Cli::command();
            args.run(&mut cmd);
        }
        Commands::Login(args) => args.run(),
        Commands::Publish(args) => args.run(),
        Commands::Register(args) => args.run(),
        Commands::Yank(args) => args.run(),
        Commands::Fmt(args) => args.run(),
        Commands::Lint(args) => args.run(),
        Commands::Migrate(args) => args.run(),
        Commands::Lsp(args) => args.run(),
        Commands::Toolchain(args) => args.run(),
        Commands::Complete { context } => print_completion_candidates(context),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_accepts_missing_package_for_current_project() {
        let cli = Cli::try_parse_from(["freight", "info"]).unwrap();
        match cli.command {
            Commands::Info(args) => assert_eq!(args.package, None),
            _ => panic!("expected info command"),
        }
    }

    #[test]
    fn info_accepts_registry_package_name() {
        let cli = Cli::try_parse_from(["freight", "info", "zlib"]).unwrap();
        match cli.command {
            Commands::Info(args) => assert_eq!(args.package.as_deref(), Some("zlib")),
            _ => panic!("expected info command"),
        }
    }
}
