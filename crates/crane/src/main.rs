use anyhow::Result;
use clap::{Parser, Subcommand};
use crane_core::build::{cmd_build, cmd_clean, cmd_run, cmd_test};
use crane_core::dep_cmds::{
    cmd_add, cmd_fetch, cmd_info, cmd_login, cmd_publish, cmd_remove, cmd_search, cmd_tree,
    cmd_update, cmd_yank,
};
use crane_core::manifest::cmd_check;
use crane_core::new::{init_project, scaffold_project};
use crane_core::output::{print_error, print_unimplemented};
use crane_core::toolchain::cmd_toolchain_list;

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
    /// Add a dependency
    Add {
        /// Package name, optionally with version: `name` or `name@1.0`
        #[arg(value_name = "NAME[@VERSION]")]
        package: String,
        /// Add as a path dependency pointing to a local crane project
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
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
    /// Import an existing build system
    Migrate {
        #[arg(long, value_name = "FORMAT")]
        from: Option<String>,
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
        Commands::New { name, lang } => scaffold_project(&name, &lang)?,
        Commands::Init { lang } => {
            if let Err(e) = init_project(lang.as_deref()) {
                print_error(&e.to_string());
                std::process::exit(1);
            }
        }
        Commands::Build { release } => cmd_build(release),
        Commands::Run { release, args } => cmd_run(release, &args),
        Commands::Test { name } => cmd_test(name.as_deref()),
        Commands::Add { package, path, system, dev } => {
            cmd_add(&package, path.as_deref(), system, dev);
        }
        Commands::Remove { package } => cmd_remove(&package),
        Commands::Update { package } => cmd_update(package.as_deref()),
        Commands::Fetch => cmd_fetch(),
        Commands::Tree => cmd_tree(),
        Commands::Info { package } => cmd_info(&package),
        Commands::Search { query } => cmd_search(&query),
        Commands::Check => cmd_check(),
        Commands::Clean => cmd_clean(),
        Commands::Migrate { .. } => print_unimplemented("migrate"),
        Commands::Login => cmd_login(),
        Commands::Publish => cmd_publish(),
        Commands::Yank { version } => cmd_yank(&version),
        Commands::Toolchain { command } => match command {
            ToolchainCommands::List => cmd_toolchain_list(),
            ToolchainCommands::Add { .. } => print_unimplemented("toolchain add"),
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
