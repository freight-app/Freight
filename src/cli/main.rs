mod commands;
mod completion;
mod output;
mod tui;

use freight::{dap, lsp};

use clap::{CommandFactory, Parser, Subcommand};

use crate::completion::{print_completion_candidates, CompletionContext};

/// Returns the top-level [`clap::Command`] for this binary.
pub(crate) fn cli_command() -> clap::Command {
    Cli::command()
}

#[derive(Parser)]
#[command(
    name = "freight",
    version,
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
    Dap(dap::Args),
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
    /// Workspace-level inspection (e.g. `freight workspace graph`)
    Workspace(commands::workspace::Args),
    /// Emit machine-readable JSON metadata for the package and dependency graph
    Metadata(commands::metadata::Args),
    /// Show outdated registry dependencies
    Outdated(commands::outdated::Args),
    /// Show package metadata (from registry when a name is given, or the current project)
    Info(commands::info::Args),
    /// Search the package registry
    Search(commands::search::Args),
    /// Validate freight.toml
    Check(commands::check::Args),
    /// Internal: provide a CMake dependency for the cmake plugin's provider
    #[command(hide = true)]
    CmakeProvide(commands::cmake_provide::Args),
    /// Wipe target/
    Clean(commands::clean::Args),
    /// Generate compile_commands.json for clangd, native Fortran LSP, serve-d and other language servers
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
    /// Authenticate with a registry and store the token in the system keychain
    Login(commands::login::Args),
    /// Remove stored credentials for a registry from the system keychain
    Logout(commands::logout::Args),
    /// Upload this package to a registry
    Publish(commands::publish::Args),
    /// Register a new account on a registry
    Register(commands::register::Args),
    /// Yank a published version (prevents new installs)
    Yank(commands::yank::Args),
    /// Manage a package's members and their per-package roles
    Owner(commands::owner::Args),
    /// Format source files
    Fmt(commands::fmt::Args),
    /// Lint source files
    Lint(commands::lint::Args),
    /// Start the freight.toml language server, native Fortran/asm indexers, and clangd passthrough
    Lsp(lsp::Args),
    /// Manage compiler toolchains
    Toolchain(commands::toolchain::Args),
    /// Registry moderation & administration (requires a moderator/admin token)
    Admin(commands::admin::Args),
    /// Internal helper used by generated shell completion scripts
    #[command(name = "__complete", hide = true)]
    Complete { context: CompletionContext },
}

/// Names of every built-in subcommand (including visible aliases), used so a
/// user `[alias]` can never shadow a real command.
fn builtin_subcommands() -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for sub in cli_command().get_subcommands() {
        names.insert(sub.get_name().to_string());
        for a in sub.get_all_aliases() {
            names.insert(a.to_string());
        }
    }
    names.insert("help".to_string());
    names
}

/// Expand a leading `[alias]` from `.freight/config.toml` into its underlying
/// command + args. Built-in subcommands are never expanded. Single-pass (an
/// alias expanding to another alias is not re-expanded).
fn expand_aliases(mut args: Vec<String>) -> Vec<String> {
    // Find the first non-flag token after the binary name — the subcommand slot.
    let Some(offset) = args.iter().skip(1).position(|a| !a.starts_with('-')) else {
        return args;
    };
    let idx = offset + 1;
    let sub = args[idx].clone();

    if builtin_subcommands().contains(&sub) {
        return args;
    }

    let mut cfg = freight::toolchain::cache::GlobalConfig::load();
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(dir) = freight::manifest::find_manifest_dir(&cwd) {
            if let Some(local) = freight::toolchain::cache::GlobalConfig::load_local(&dir) {
                cfg.apply_local(local);
            }
        }
    }

    if let Some(value) = cfg.alias.remove(&sub) {
        let expansion = value.into_args();
        if !expansion.is_empty() {
            args.splice(idx..=idx, expansion);
        }
    }
    args
}

fn main() {
    let cli = Cli::parse_from(expand_aliases(std::env::args().collect()));

    // For non-LSP commands, initialise a stderr logger when FREIGHT_LOG is set.
    // (The lsp command sets up its own subscriber that forwards to VS Code.)
    if !matches!(cli.command, Commands::Lsp(_)) {
        lsp::log::init_stderr_logging();
    }

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
        Commands::Workspace(args) => args.run(),
        Commands::Metadata(args) => args.run(),
        Commands::Outdated(args) => args.run(),
        Commands::Info(args) => args.run(),
        Commands::Search(args) => args.run(),
        Commands::Check(args) => args.run(),
        Commands::CmakeProvide(args) => args.run(),
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
        Commands::Logout(args) => args.run(),
        Commands::Publish(args) => args.run(),
        Commands::Register(args) => args.run(),
        Commands::Yank(args) => args.run(),
        Commands::Owner(args) => args.run(),
        Commands::Fmt(args) => args.run(),
        Commands::Lint(args) => args.run(),
        Commands::Lsp(args) => args.run(),
        Commands::Toolchain(args) => args.run(),
        Commands::Admin(args) => args.run(),
        Commands::Complete { context } => print_completion_candidates(context),
    }

    if crate::output::had_error() {
        std::process::exit(1);
    }
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

    #[test]
    fn builtins_include_real_commands_not_aliases() {
        let b = builtin_subcommands();
        assert!(b.contains("build"));
        assert!(b.contains("run"));
        assert!(b.contains("help"));
        // A short user alias like `b` must not collide with a real command.
        assert!(!b.contains("b"));
    }

    #[test]
    fn expand_aliases_never_touches_builtins() {
        // `build` is a real command, so the arg vector is returned unchanged even
        // if a (hypothetical) alias existed — proven by the slot staying put.
        let args = vec![
            "freight".to_string(),
            "build".to_string(),
            "--release".to_string(),
        ];
        assert_eq!(expand_aliases(args.clone()), args);
    }

    #[test]
    fn expand_aliases_ignores_flag_only_invocation() {
        let args = vec!["freight".to_string(), "--help".to_string()];
        assert_eq!(expand_aliases(args.clone()), args);
    }
}
