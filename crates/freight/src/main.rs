mod commands;
mod completion;
mod output;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use crate::completion::{
    print_completion, print_completion_candidates, CompletionContext, CompletionShell,
};

use crate::commands::build::{cmd_build, cmd_clean, cmd_run, cmd_test, cmd_watch};
use crate::commands::check::cmd_check;
use crate::commands::compile_commands::cmd_compile_commands;
use crate::commands::debug::cmd_debug;
use crate::commands::deps::{
    cmd_add, cmd_add_interactive, cmd_fetch, cmd_info, cmd_login, cmd_publish, cmd_remove,
    cmd_search, cmd_tree, cmd_update, cmd_yank,
};
use crate::commands::doc::{cmd_doc, cmd_man};
use crate::commands::fmt::cmd_fmt;
use crate::commands::install::{cmd_install, cmd_package};
use crate::commands::lint::cmd_lint;
use crate::commands::new::{cmd_init, cmd_new};
use crate::commands::toolchain::{cmd_toolchain_add, cmd_toolchain_list, cmd_toolchain_use};

/// Returns the top-level [`clap::Command`] for this binary.
/// Used by `freight man` to generate man pages without re-parsing argv.
pub(crate) fn cli_command() -> clap::Command {
    Cli::command()
}

#[derive(Parser)]
#[command(
    name = "freight",
    about = "Build tool and package manager for C, C++, Fortran, and more"
)]
struct Cli {
    /// Print every compiler and linker invocation
    #[arg(long, short = 'v', global = true)]
    verbose: bool,
    /// Number of parallel compile jobs (default: logical CPUs)
    #[arg(long, short = 'j', global = true, value_name = "N")]
    jobs: Option<usize>,
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
    /// Initialize freight in the current directory
    Init {
        #[arg(long)]
        lang: Option<String>,
    },
    /// Build the project
    Build {
        #[arg(long)]
        release: bool,
        /// Activate specific features (comma-separated or repeated)
        #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
        features: Vec<String>,
        /// Do not activate default features
        #[arg(long)]
        no_default_features: bool,
        /// Enable sanitizers for this run (e.g. address,undefined). Overrides the profile setting.
        #[arg(long, value_name = "LIST", value_delimiter = ',')]
        sanitize: Vec<String>,
    },
    /// Build and run the default binary
    Run {
        #[arg(long)]
        release: bool,
        /// Binary to run when the project has multiple [[bin]] targets
        #[arg(long, value_name = "NAME")]
        bin: Option<String>,
        /// Activate specific features (comma-separated or repeated)
        #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
        features: Vec<String>,
        /// Do not activate default features
        #[arg(long)]
        no_default_features: bool,
        /// Arguments to pass to the binary
        #[arg(last = true)]
        args: Vec<String>,
        /// Enable sanitizers for this run (e.g. address,undefined). Overrides the profile setting.
        #[arg(long, value_name = "LIST", value_delimiter = ',')]
        sanitize: Vec<String>,
    },
    /// Build and run tests
    Test {
        name: Option<String>,
        #[arg(long)]
        release: bool,
        /// Activate specific features (comma-separated or repeated)
        #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
        features: Vec<String>,
        /// Do not activate default features
        #[arg(long)]
        no_default_features: bool,
        /// Enable sanitizers for this run (e.g. address,undefined). Overrides the profile setting.
        #[arg(long, value_name = "LIST", value_delimiter = ',')]
        sanitize: Vec<String>,
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
    /// Watch source files and rebuild on changes
    Watch {
        #[arg(long)]
        release: bool,
    },
    /// Add a dependency
    Add {
        /// Package name, optionally with version: `name` or `name@1.0`. Omit to browse vcpkg.
        #[arg(value_name = "NAME[@VERSION]")]
        package: Option<String>,
        /// Add as a path dependency pointing to a local freight project
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
    /// Search freight.dev
    Search { query: String },
    /// Validate freight.toml
    Check,
    /// Wipe target/
    Clean,
    /// Generate compile_commands.json for clangd, fortls, serve-d and other language servers
    CompileCommands {
        #[arg(long)]
        release: bool,
    },
    /// Open the dependency documentation browser, or generate API docs with --format
    Doc {
        /// Output format: md | json | msgpack | all
        #[arg(long, short, value_name = "FORMAT")]
        format: Option<String>,
    },
    /// Install build outputs to a system prefix (binaries, libs, headers)
    Install {
        /// Installation prefix (default: /usr/local)
        #[arg(long, value_name = "PATH", default_value = "/usr/local")]
        prefix: String,
        /// Staging root prepended before prefix (for package managers / fakeroot)
        #[arg(long, value_name = "PATH")]
        destdir: Option<String>,
        /// Install release build (default: true)
        #[arg(long, default_value_t = true)]
        release: bool,
        /// Skip the build step; install from existing target/ outputs
        #[arg(long)]
        no_build: bool,
        /// Cross-compilation target triple (e.g. aarch64-linux-gnu)
        #[arg(long, value_name = "TRIPLE")]
        target: Option<String>,
    },
    /// Build and pack outputs into a redistributable tar.gz archive
    Package {
        /// Package the release build (default: true)
        #[arg(long, default_value_t = true)]
        release: bool,
        /// Target triples to package, comma-separated (e.g. aarch64-linux-gnu,x86_64-linux-gnu).
        /// Omit for a native build. Unsupported combinations are skipped with a warning.
        #[arg(long, value_name = "TRIPLES", value_delimiter = ',')]
        target: Vec<String>,
    },
    /// Generate man pages for all freight subcommands
    Man {
        /// Output directory (default: target/man/)
        #[arg(long, value_name = "DIR")]
        out_dir: Option<String>,
    },
    /// Generate shell completion scripts
    #[command(visible_alias = "completion")]
    Completions {
        /// Shell to generate completions for (bash, elvish, fish, powershell, zsh)
        shell: CompletionShell,
    },
    /// Authenticate with freight.dev
    Login,
    /// Upload this package to freight.dev
    Publish,
    /// Yank a published version
    Yank { version: String },
    /// Format source files
    Fmt {
        /// Check formatting without modifying files
        #[arg(long)]
        check: bool,
    },
    /// Lint source files
    Lint {
        /// Apply auto-fixes where possible
        #[arg(long)]
        fix: bool,
    },
    /// Manage compiler toolchains
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },
    /// Internal helper used by generated shell completion scripts
    #[command(name = "__complete", hide = true)]
    Complete { context: CompletionContext },
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

    if cli.verbose {
        // Safety: single-threaded at this point; no rayon workers started yet.
        unsafe {
            std::env::set_var("FREIGHT_VERBOSE", "1");
        }
    }
    if let Some(n) = cli.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .ok();
    }

    match cli.command {
        Commands::New { name, lang } => cmd_new(&name, &lang),
        Commands::Init { lang } => cmd_init(lang.as_deref()),
        Commands::Build {
            release,
            features,
            no_default_features,
            sanitize,
        } => {
            cmd_build(release, &features, !no_default_features, &sanitize);
        }
        Commands::Run {
            release,
            bin,
            features,
            no_default_features,
            args,
            sanitize,
        } => {
            cmd_run(
                release,
                bin.as_deref(),
                &features,
                !no_default_features,
                &args,
                &sanitize,
            );
        }
        Commands::Test {
            name,
            release,
            features,
            no_default_features,
            sanitize,
        } => {
            cmd_test(
                name.as_deref(),
                release,
                &features,
                !no_default_features,
                &sanitize,
            );
        }
        Commands::Watch { release } => cmd_watch(release),
        Commands::Debug {
            binary,
            debugger,
            launch_json,
            args,
        } => {
            cmd_debug(binary.as_deref(), debugger.as_deref(), &args, launch_json);
        }
        Commands::Add {
            package,
            path,
            git,
            branch,
            tag,
            rev,
            system,
            dev,
        } => {
            if let Some(package) = package {
                cmd_add(
                    &package,
                    path.as_deref(),
                    git.as_deref(),
                    branch.as_deref(),
                    tag.as_deref(),
                    rev.as_deref(),
                    system,
                    dev,
                );
            } else {
                cmd_add_interactive(
                    path.as_deref(),
                    git.as_deref(),
                    branch.as_deref(),
                    tag.as_deref(),
                    rev.as_deref(),
                    system,
                    dev,
                );
            }
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
        Commands::Install {
            prefix,
            destdir,
            release,
            no_build,
            target,
        } => {
            cmd_install(
                Some(&prefix),
                destdir.as_deref(),
                release,
                no_build,
                target.as_deref(),
            );
        }
        Commands::Package { release, target } => cmd_package(release, &target),
        Commands::Doc { format } => cmd_doc(format.as_deref()),
        Commands::Man { out_dir } => cmd_man(out_dir.as_deref()),
        Commands::Completions { shell } => {
            let cmd = Cli::command();
            print_completion(shell, &cmd);
        }
        Commands::Login => cmd_login(),
        Commands::Publish => cmd_publish(),
        Commands::Yank { version } => cmd_yank(&version),
        Commands::Fmt { check } => cmd_fmt(check),
        Commands::Lint { fix } => cmd_lint(fix),
        Commands::Toolchain { command } => match command {
            ToolchainCommands::List => cmd_toolchain_list(),
            ToolchainCommands::Add { name } => cmd_toolchain_add(&name),
            ToolchainCommands::Use { name } => cmd_toolchain_use(&name),
        },
        Commands::Complete { context } => print_completion_candidates(context),
    }

    Ok(())
}
