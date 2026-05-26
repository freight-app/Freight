mod commands;
mod completion;
mod output;
mod tui;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use crate::completion::{
    print_completion, print_completion_candidates, CompletionContext, CompletionShell,
};

use crate::commands::build::{cmd_bench, cmd_build, cmd_build_graph, cmd_clean, cmd_run, cmd_test, cmd_watch};
use crate::commands::check::cmd_check;
use crate::commands::compile_commands::cmd_compile_commands;
use crate::commands::debug::cmd_debug;
use crate::commands::deps::{
    cmd_add, cmd_add_interactive, cmd_fetch, cmd_includes, cmd_info, cmd_login, cmd_outdated,
    cmd_publish, cmd_publish_prebuilt, cmd_register, cmd_remove, cmd_search, cmd_tree, cmd_update,
    cmd_yank,
};
use crate::commands::doc::cmd_doc;
use crate::commands::fmt::cmd_fmt;
use crate::commands::install::{cmd_install, cmd_package};
use crate::commands::lint::cmd_lint;
use crate::commands::new::{cmd_init, cmd_new};
use crate::commands::migrate::{cmd_migrate_autotools, cmd_migrate_cmake, cmd_migrate_make};
use crate::commands::toolchain::{cmd_toolchain_list, cmd_toolchain_use};

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
        /// Print the build graph (compilation stages and link step) instead of building.
        #[arg(long)]
        graph: bool,
        /// Output format for --graph: text (default), mermaid, dot
        #[arg(long, default_value = "text", value_name = "FORMAT", requires = "graph")]
        graph_format: String,
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
        /// Pass a git URL (https://…) or archive URL (https://….tar.gz) to add
        /// without `--git`/`--url` flags. Omit entirely for an interactive prompt.
        #[arg(value_name = "NAME[@VERSION]|URL")]
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
    Fetch {
        /// Always download source tarballs, even if a prebuilt is available for the current triple
        #[arg(long, short = 's')]
        source: bool,
    },
    /// Print the dependency tree, or the source/header include tree with --sources
    Tree {
        /// Show the include graph for source and header files instead of the dependency tree
        #[arg(long, short = 's')]
        sources: bool,
        /// Also show system headers (#include <...>) when using --sources
        #[arg(long, short = 'a', requires = "sources")]
        all: bool,
        /// Output format: text (default), mermaid, dot
        #[arg(long, short = 'f', default_value = "text", value_name = "FORMAT")]
        format: String,
    },
    /// Show outdated registry dependencies
    Outdated {
        /// Registry to query (default: all configured registries in order)
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,
    },
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
        /// Generate Markdown docs for this project (output format: md)
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
        /// Registry base URL (default: first configured registry or https://freight.dev)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
        /// API token — skip username/password and save this token directly
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
        /// Username (skips the TUI and calls the login API directly when combined with --password)
        #[arg(long, value_name = "NAME")]
        username: Option<String>,
        /// Password (skips the TUI when combined with --username)
        #[arg(long, value_name = "PASS")]
        password: Option<String>,
        /// Skip the interactive TUI and use plain CLI prompts instead
        #[arg(long)]
        notui: bool,
    },
    /// Upload this package to a registry
    Publish {
        /// Dry run: print what would be uploaded without sending
        #[arg(long)]
        dry_run: bool,
        /// Registry to publish to (default: first configured registry)
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,
        /// Upload a prebuilt binary tarball for the given triple instead of source.
        /// Omit the triple to use the detected host triple (e.g. x86_64-linux-gnu).
        #[arg(long, value_name = "TRIPLE")]
        prebuilt: Option<Option<String>>,
    },
    /// Register a new account on a registry
    Register {
        /// Registry base URL (default: first configured registry or https://freight.dev)
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
        /// Username for the new account
        #[arg(long, value_name = "NAME")]
        username: Option<String>,
        /// Email address for the new account (optional)
        #[arg(long, value_name = "EMAIL")]
        email: Option<String>,
        /// Name for the initial API token (default: init)
        #[arg(long, value_name = "NAME")]
        token_name: Option<String>,
        /// Skip the interactive TUI and use plain CLI prompts instead
        #[arg(long)]
        notui: bool,
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
    /// Import a project from another build system into freight
    Migrate {
        #[command(subcommand)]
        command: MigrateCommands,
    },
    /// Manage compiler toolchains
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },
    /// Open the registry admin panel (packages, users, tokens, orgs, audit log)
    Tui {
        /// Registry base URL (default: http://localhost:7878 or configured registry)
        #[arg(long, env = "FREIGHT_REGISTRY_URL", default_value = "http://localhost:7878")]
        url: String,
        /// API token — omit to use saved credentials or the interactive login screen
        #[arg(long, env = "FREIGHT_REGISTRY_TOKEN")]
        token: Option<String>,
    },
    /// Internal helper used by generated shell completion scripts
    #[command(name = "__complete", hide = true)]
    Complete { context: CompletionContext },
}

#[derive(Subcommand)]
enum ToolchainCommands {
    /// Show detected compilers
    List,
    /// Set the default compiler backend
    Use { name: String },
}

#[derive(Subcommand)]
enum MigrateCommands {
    /// Import a Make/Makefile project
    Make {
        /// Path to the project directory or Makefile
        input: String,
        /// Write generated freight.toml files here instead of next to the Makefile
        #[arg(long, value_name = "DIR")]
        out_dir: Option<String>,
        /// Remove the Makefile(s) after successful import
        #[arg(long)]
        purge: bool,
    },
    /// Import a CMake project (CMakeLists.txt)
    Cmake {
        /// Path to the project directory or CMakeLists.txt
        input: String,
        /// Write generated freight.toml files here instead of next to CMakeLists.txt
        #[arg(long, value_name = "DIR")]
        out_dir: Option<String>,
        /// Remove CMakeLists.txt and CMake artefacts after successful import
        #[arg(long)]
        purge: bool,
    },
    /// Import an Autotools project (configure.ac + Makefile.am)
    Autotools {
        /// Path to the project directory
        input: String,
        /// Write the generated freight.toml here instead of next to configure.ac
        #[arg(long, value_name = "DIR")]
        out_dir: Option<String>,
        /// Remove autotools files after successful import
        #[arg(long)]
        purge: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        // Safety: single-threaded at this point; no rayon workers started yet.
        unsafe {
            std::env::set_var("FREIGHT_VERBOSE", "1");
        }
    }
    // Always configure the rayon thread pool so foreign build systems (cmake,
    // make, ninja, …) can read rayon::current_num_threads() as the job count.
    // Default: min(logical CPUs, 6) — prevents saturating all cores.
    let jobs = cli.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(6)
    });
    rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build_global()
        .ok();

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
            graph,
            graph_format,
        } => {
            if graph {
                cmd_build_graph(release, package.as_deref(), &features, !no_default_features, &graph_format);
            } else {
                cmd_build(release, package.as_deref(), &features, !no_default_features, &sanitize, &emit, time_passes);
            }
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
                    repo.as_deref(),
                    dev,
                );
            } else {
                cmd_add_interactive(repo.as_deref(), dev);
            }
        }
        Commands::Remove { package } => cmd_remove(&package),
        Commands::Update { package } => cmd_update(package.as_deref()),
        Commands::Fetch { source } => cmd_fetch(source),
        Commands::Tree { sources, all, format } => {
            if sources { cmd_includes(all, &format) } else { cmd_tree() }
        }
        Commands::Outdated { repo } => cmd_outdated(repo.as_deref()),
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
        Commands::Login { registry, token, username, password, notui } => {
            // --token → store directly, no TUI
            // --username + --password → call API directly, no TUI
            // --notui → existing stdin-prompt CLI
            // default → TUI login form
            if token.is_some() || notui {
                cmd_login(registry.as_deref(), token.as_deref());
            } else if username.is_some() || password.is_some() {
                cmd_login_with_credentials(
                    registry.as_deref(),
                    username.as_deref(),
                    password.as_deref(),
                );
            } else {
                let url = resolve_registry_url(registry.as_deref());
                match tui::login::run(url.clone(), None) {
                    Ok((uname, token)) => {
                        let name = registry_name_for(&url);
                        match freight_core::toolchain::cache::GlobalConfig::save_credential(&url, &name, &token) {
                            Ok(()) => crate::output::print_success(
                                &format!("logged in as `{uname}` — token saved to ~/.freight/credentials.toml")
                            ),
                            Err(e) => {
                                crate::output::print_error(&e.to_string());
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) if e.to_string() == "cancelled" => {}
                    Err(e) => {
                        crate::output::print_error(&e.to_string());
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Register { registry, username, email, token_name, notui } => {
            // --notui or username already given → CLI path (stdin prompts for missing fields)
            if notui {
                cmd_register(registry.as_deref(), username.as_deref(), email.as_deref(), token_name.as_deref());
            } else {
                let url = resolve_registry_url(registry.as_deref());
                match tui::register::run(url.clone(), username, email, token_name) {
                    Ok((uname, token)) => {
                        let name = registry_name_for(&url);
                        match freight_core::toolchain::cache::GlobalConfig::save_credential(&url, &name, &token) {
                            Ok(()) => crate::output::print_success(
                                &format!("registered as `{uname}` — token saved to ~/.freight/credentials.toml")
                            ),
                            Err(e) => {
                                crate::output::print_error(&e.to_string());
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) if e.to_string() == "cancelled" => {}
                    Err(e) => {
                        crate::output::print_error(&e.to_string());
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Publish { dry_run, repo, prebuilt } => {
            if let Some(triple_opt) = prebuilt {
                cmd_publish_prebuilt(triple_opt.as_deref(), repo.as_deref());
            } else {
                cmd_publish(dry_run, repo.as_deref());
            }
        }
        Commands::Yank { version, undo, repo } => cmd_yank(&version, undo, repo.as_deref()),
        Commands::Fmt { check } => cmd_fmt(check),
        Commands::Lint { fix } => cmd_lint(fix),
        Commands::Migrate { command } => match command {
            MigrateCommands::Make { input, out_dir, purge } => {
                cmd_migrate_make(&input, out_dir.as_deref(), purge);
            }
            MigrateCommands::Cmake { input, out_dir, purge } => {
                cmd_migrate_cmake(&input, out_dir.as_deref(), purge);
            }
            MigrateCommands::Autotools { input, out_dir, purge } => {
                cmd_migrate_autotools(&input, out_dir.as_deref(), purge);
            }
        },
        Commands::Toolchain { command } => match command {
            ToolchainCommands::List => cmd_toolchain_list(),
            ToolchainCommands::Use { name } => cmd_toolchain_use(&name),
        },
        Commands::Tui { url, token } => {
            if let Err(e) = tui::registry::run(url, token) {
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
        Commands::Complete { context } => print_completion_candidates(context),
    }

    Ok(())
}

// ── TUI dispatch helpers ───────────────────────────────────────────────────────

/// Resolve the registry URL from the explicit flag, first configured registry,
/// or the default freight.dev URL — same logic as cmd_login.
fn resolve_registry_url(registry: Option<&str>) -> String {
    use freight_core::toolchain::cache::GlobalConfig;
    registry
        .map(str::to_string)
        .or_else(|| GlobalConfig::load().registries.into_iter().next().map(|r| r.url))
        .unwrap_or_else(|| "https://freight.dev".to_string())
}

/// Return the configured registry name for a URL, falling back to "freight".
fn registry_name_for(url: &str) -> String {
    use freight_core::toolchain::cache::GlobalConfig;
    GlobalConfig::load()
        .registries
        .iter()
        .find(|r| r.url == url)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "freight".to_string())
}

/// Call /api/v1/users/login with username + password and save the resulting token.
/// Used by `freight login --username NAME --password PASS` (non-TUI path).
fn cmd_login_with_credentials(
    registry_url: Option<&str>,
    username:     Option<&str>,
    password:     Option<&str>,
) {
    use freight_core::toolchain::cache::GlobalConfig;

    let url  = resolve_registry_url(registry_url);
    let name = registry_name_for(&url);

    let username = match username {
        Some(u) => u.to_string(),
        None => {
            use std::io::{self, Write};
            print!("Username: ");
            io::stdout().flush().ok();
            let mut u = String::new();
            io::stdin().read_line(&mut u).ok();
            u.trim().to_string()
        }
    };
    let password = match password {
        Some(p) => p.to_string(),
        None => {
            use std::io::{self, Write};
            print!("Password: ");
            io::stdout().flush().ok();
            let mut p = String::new();
            io::stdin().read_line(&mut p).ok();
            p.trim().to_string()
        }
    };

    if username.is_empty() {
        crate::output::print_error("username cannot be empty");
        std::process::exit(1);
    }

    // Use the TUI client (reqwest + tokio, already deps) to call the login API.
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => { crate::output::print_error(&e.to_string()); std::process::exit(1); }
    };
    let client = tui::registry::client::Client::new(url.clone(), None);
    let token = match rt.block_on(client.login(&username, &password)) {
        Ok(resp) => resp.token,
        Err(e) => {
            crate::output::print_error(&format!("login failed: {e}"));
            std::process::exit(1);
        }
    };

    match GlobalConfig::save_credential(&url, &name, &token) {
        Ok(()) => crate::output::print_success(
            &format!("logged in as `{username}` — token saved to ~/.freight/credentials.toml")
        ),
        Err(e) => {
            crate::output::print_error(&e.to_string());
            std::process::exit(1);
        }
    }
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
