mod commands;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::commands::build::{cmd_build, cmd_clean, cmd_run, cmd_test};
use crate::commands::compile_commands::cmd_compile_commands;
use crate::commands::check::cmd_check;
use crate::commands::debug::cmd_debug;
use crate::commands::deps::{
    cmd_add, cmd_fetch, cmd_info, cmd_login, cmd_publish, cmd_remove, cmd_search, cmd_tree,
    cmd_update, cmd_yank,
};
use crate::commands::migrate::cmd_migrate;
use crate::commands::new::{cmd_init, cmd_new};
use crate::commands::toolchain::{cmd_toolchain_add, cmd_toolchain_list};
use crate::output::print_unimplemented;

#[derive(Parser)]
#[command(name = "crane", about = "Build tool and package manager for C, C++, Fortran, and more")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scaffold a new project
    New {
        name: String,
        #[arg(long, default_value = "c++")]
        lang: String,
    },
    /// Initialize crane in the current directory
    Init {
        #[arg(long)]
        lang: Option<String>,
    },
    /// Build the project
    Build {
        #[arg(long)]
        release: bool,
    },
    /// Build and run the default binary
    Run {
        #[arg(long)]
        release: bool,
        /// Arguments to pass to the binary
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Build and run tests
    Test {
        name: Option<String>,
    },
    /// Build (debug) and launch an interactive debugger session
    Debug {
        /// Binary to debug (required when the project has multiple [[bin]] targets)
        binary: Option<String>,
        /// Debugger to use (e.g. lldb, gdb); auto-selected when omitted
        #[arg(long, value_name = "NAME")]
        debugger: Option<String>,
        /// Generate .vscode/launch.json instead of launching a debugger
        #[arg(long)]
        launch_json: bool,
        /// Arguments passed to the debugged program
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Add a dependency
    Add {
        /// Package name, optionally with version: `name` or `name@1.0`
        #[arg(value_name = "NAME[@VERSION]")]
        package: String,
        /// Add as a path dependency pointing to a local crane project
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        /// Add as a git dependency (URL)
        #[arg(long, value_name = "URL")]
        git: Option<String>,
        /// Git branch to track (requires --git)
        #[arg(long)]
        branch: Option<String>,
        /// Git tag to check out (requires --git)
        #[arg(long)]
        tag: Option<String>,
        /// Exact commit SHA to pin (requires --git)
        #[arg(long)]
        rev: Option<String>,
        /// Add as a system (linker) dependency
        #[arg(long)]
        system: bool,
        /// Add to [dev-dependencies] instead of [dependencies]
        #[arg(long)]
        dev: bool,
    },
    /// Remove a dependency
    Remove { package: String },
    /// Update dependencies within semver ranges
    Update { package: Option<String> },
    /// Download dependencies without building
    Fetch,
    /// Print the dependency tree
    Tree,
    /// Show package metadata
    Info { package: String },
    /// Search crane.dev
    Search { query: String },
    /// Validate crane.toml
    Check,
    /// Wipe target/
    Clean,
    /// Generate compile_commands.json for clangd, fortls, serve-d and other language servers
    CompileCommands {
        #[arg(long)]
        release: bool,
    },
    /// Import an existing build system (CMake, Makefile, or Meson)
    Migrate {
        /// Source build system; auto-detected when omitted
        #[arg(long, value_name = "FORMAT")]
        from: Option<String>,
        /// Print generated crane.toml to stdout without writing
        #[arg(long)]
        dry_run: bool,
        /// Overwrite an existing crane.toml
        #[arg(long)]
        force: bool,
    },
    /// Authenticate with crane.dev
    Login,
    /// Upload this package to crane.dev
    Publish,
    /// Yank a published version
    Yank { version: String },
    /// Manage compiler toolchains
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },
    /// Run the crane language server (for editor integration, stdio)
    Lsp,
}

#[derive(Subcommand)]
enum ToolchainCommands {
    /// Show detected compilers
    List,
    /// Install a compiler template
    Add { name: String },
    /// Set the default compiler backend
    Use { name: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::New { name, lang } => cmd_new(&name, &lang),
        Commands::Init { lang } => cmd_init(lang.as_deref()),
        Commands::Build { release } => cmd_build(release),
        Commands::Run { release, args } => cmd_run(release, &args),
        Commands::Test { name } => cmd_test(name.as_deref()),
        Commands::Debug { binary, debugger, launch_json, args } => {
            cmd_debug(binary.as_deref(), debugger.as_deref(), &args, launch_json);
        }
        Commands::Add { package, path, git, branch, tag, rev, system, dev } => {
            cmd_add(&package, path.as_deref(), git.as_deref(), branch.as_deref(), tag.as_deref(), rev.as_deref(), system, dev);
        }
        Commands::Remove { package } => cmd_remove(&package),
        Commands::Update { package } => cmd_update(package.as_deref()),
        Commands::Fetch => cmd_fetch(),
        Commands::Tree => cmd_tree(),
        Commands::Info { package } => cmd_info(&package),
        Commands::Search { query } => cmd_search(&query),
        Commands::Check => cmd_check(),
        Commands::Clean => cmd_clean(),
        Commands::CompileCommands { release } => cmd_compile_commands(release),
        Commands::Migrate { from, dry_run, force } => {
            cmd_migrate(from.as_deref(), dry_run, force);
        }
        Commands::Login => cmd_login(),
        Commands::Publish => cmd_publish(),
        Commands::Yank { version } => cmd_yank(&version),
        Commands::Toolchain { command } => match command {
            ToolchainCommands::List => cmd_toolchain_list(),
            ToolchainCommands::Add { name } => cmd_toolchain_add(&name),
            ToolchainCommands::Use { .. } => print_unimplemented("toolchain use"),
        },
        Commands::Lsp => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(crane_lsp::run());
        }
    }

    Ok(())
}
