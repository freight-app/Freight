mod commands;
mod completion;
mod output;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use crate::completion::{
    print_completion, print_completion_candidates, CompletionContext, CompletionShell,
};

use crate::commands::build::{cmd_bench, cmd_build, cmd_clean, cmd_run, cmd_test, cmd_watch};
use crate::commands::check::cmd_check;
use crate::commands::compile_commands::cmd_compile_commands;
use crate::commands::debug::cmd_debug;
use crate::commands::deps::{
    cmd_add, cmd_add_interactive, cmd_fetch, cmd_info, cmd_login, cmd_publish, cmd_remove,
    cmd_search, cmd_tree, cmd_update, cmd_yank,
};
use crate::commands::doc::cmd_doc;
use crate::commands::fmt::cmd_fmt;
use crate::commands::install::{cmd_install, cmd_package};
use crate::commands::lint::cmd_lint;
use crate::commands::new::{cmd_init, cmd_new};
use crate::commands::toolchain::{cmd_toolchain_add, cmd_toolchain_list, cmd_toolchain_use};

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
        /// Select a specific workspace member to build
        #[arg(long, short = 'p', value_name = "PACKAGE")]
        package: Option<String>,
        /// Extra outputs to emit alongside object files. Accepted value: `asm`
        /// (writes `.s` files to `target/{profile}/asm/`).
        #[arg(long, value_name = "FORMAT", value_delimiter = ',')]
        emit: Vec<String>,
        /// Print a per-file compilation time table sorted by slowest first.
        #[arg(long)]
        time_passes: bool,
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
        /// Select a specific workspace member to run
        #[arg(long, short = 'p', value_name = "PACKAGE")]
        package: Option<String>,
    },
    /// Build and run benchmarks in benches/
    Bench {
        /// Run only the bench with this name (file stem)
        name: Option<String>,
        /// Activate specific features (comma-separated or repeated)
        #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
        features: Vec<String>,
        /// Do not activate default features
        #[arg(long)]
        no_default_features: bool,
        /// Select a specific workspace member to benchmark
        #[arg(long, short = 'p', value_name = "PACKAGE")]
        package: Option<String>,
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
        /// Select a specific workspace member to test
        #[arg(long, short = 'p', value_name = "PACKAGE")]
        package: Option<String>,
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
        /// Package name, optionally with version: `name` or `name@1.0`.
        /// Omit to get an interactive prompt.
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
        /// Package repository to use (default: freight registry).
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,
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
    /// Show package metadata (from registry when a name is given, or the current project)
    Info {
        package: Option<String>,
        /// Registry to query (default: all configured registries in order)
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,
    },
    /// Search the package registry
    Search {
        query: String,
        /// Registry to search (default: all configured registries in order)
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,
    },
    /// Validate freight.toml
    Check,
    /// Wipe target/
    Clean,
    /// Generate compile_commands.json for clangd, fortls, serve-d and other language servers
    CompileCommands {
        #[arg(long)]
        release: bool,
    },
    /// Open the dependency documentation browser, or generate API docs / man pages
    Doc {
        /// Output format: md | json | msgpack | all
        #[arg(long, short, value_name = "FORMAT")]
        format: Option<String>,
        /// Generate man pages for all freight subcommands
        #[arg(long)]
        man: bool,
        /// Output directory for man pages (default: target/man/)
        #[arg(long, value_name = "DIR", requires = "man")]
        out_dir: Option<String>,
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
    /// Build and pack outputs into a redistributable archive (.tar.gz, or .zip for Windows targets)
    Package {
        /// Package the release build (default: true)
        #[arg(long, default_value_t = true)]
        release: bool,
        /// Target triples to package, comma-separated (e.g. aarch64-linux-gnu,x86_64-linux-gnu).
        /// Omit for a native build. Unsupported combinations are skipped with a warning.
        #[arg(long, value_name = "TRIPLES", value_delimiter = ',')]
        target: Vec<String>,
    },
/// Generate shell completion scripts
    #[command(visible_alias = "completion")]
    Completions {
        /// Shell to generate completions for (bash, elvish, fish, powershell, zsh)
        shell: CompletionShell,
    },
    /// Authenticate with a registry and save the token
    Login {
        /// Registry base URL (default: https://freight.dev or first configured registry)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
        /// API token (prompted interactively if omitted)
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
    },
    /// Upload this package to a registry
    Publish {
        /// Dry run: print what would be uploaded without sending
        #[arg(long)]
        dry_run: bool,
        /// Registry to publish to (default: first configured registry)
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,
    },
    /// Yank a published version (prevents new installs)
    Yank {
        /// Package name and version to yank (e.g. mylib@1.0.0)
        /// Omit the package name to use the current project
        version: String,
        /// Undo a yank (re-allow installs)
        #[arg(long)]
        undo: bool,
        /// Registry to operate on (default: first configured registry)
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,
    },
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
            package,
            emit,
            time_passes,
        } => {
            cmd_build(release, package.as_deref(), &features, !no_default_features, &sanitize, &emit, time_passes);
        }
        Commands::Run {
            release,
            bin,
            features,
            no_default_features,
            args,
            sanitize,
            package,
        } => {
            cmd_run(
                release,
                package.as_deref(),
                bin.as_deref(),
                &features,
                !no_default_features,
                &args,
                &sanitize,
            );
        }
        Commands::Bench {
            name,
            features,
            no_default_features,
            package,
        } => {
            cmd_bench(name.as_deref(), package.as_deref(), &features, !no_default_features);
        }
        Commands::Test {
            name,
            release,
            features,
            no_default_features,
            sanitize,
            package,
        } => {
            cmd_test(
                name.as_deref(),
                release,
                package.as_deref(),
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
            repo,
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
                    repo.as_deref(),
                    dev,
                );
            } else {
                cmd_add_interactive(repo.as_deref(), dev);
            }
        }
        Commands::Remove { package } => cmd_remove(&package),
        Commands::Update { package } => cmd_update(package.as_deref()),
        Commands::Fetch => cmd_fetch(),
        Commands::Tree => cmd_tree(),
        Commands::Info { package, repo } => cmd_info(package.as_deref(), repo.as_deref()),
        Commands::Search { query, repo } => cmd_search(&query, repo.as_deref()),
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
        Commands::Doc { format, man, out_dir } => cmd_doc(format.as_deref(), man, out_dir.as_deref()),
        Commands::Completions { shell } => {
            let cmd = Cli::command();
            print_completion(shell, &cmd);
        }
        Commands::Login { registry, token } => cmd_login(registry.as_deref(), token.as_deref()),
        Commands::Publish { dry_run, repo } => cmd_publish(dry_run, repo.as_deref()),
        Commands::Yank { version, undo, repo } => cmd_yank(&version, undo, repo.as_deref()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_accepts_missing_package_for_current_project() {
        let cli = Cli::try_parse_from(["freight", "info"]).unwrap();
        match cli.command {
            Commands::Info { package, .. } => assert_eq!(package, None),
            _ => panic!("expected info command"),
        }
    }

    #[test]
    fn info_accepts_registry_package_name() {
        let cli = Cli::try_parse_from(["freight", "info", "zlib"]).unwrap();
        match cli.command {
            Commands::Info { package, .. } => assert_eq!(package.as_deref(), Some("zlib")),
            _ => panic!("expected info command"),
        }
    }
}
