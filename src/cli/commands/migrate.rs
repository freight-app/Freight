use std::path::Path;

use freight::migration::autotools::{import_autotools, purge_autotools};
use freight::migration::cmake::{import_cmake, purge_cmake};
use freight::migration::make::{import_make, purge_make};

use crate::output::{print_error, print_status, print_warning};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: MigrateCmd,
}

#[derive(clap::Subcommand)]
pub enum MigrateCmd {
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

impl Args {
    pub fn run(self) {
        match self.command {
            MigrateCmd::Make {
                input,
                out_dir,
                purge,
            } => cmd_migrate_make(&input, out_dir.as_deref(), purge),
            MigrateCmd::Cmake {
                input,
                out_dir,
                purge,
            } => cmd_migrate_cmake(&input, out_dir.as_deref(), purge),
            MigrateCmd::Autotools {
                input,
                out_dir,
                purge,
            } => cmd_migrate_autotools(&input, out_dir.as_deref(), purge),
        }
    }
}

pub fn cmd_migrate_make(input: &str, out_dir: Option<&str>, purge: bool) {
    let input_path = Path::new(input);
    let out = out_dir.map(Path::new);

    match import_make(input_path, out) {
        Ok(result) => {
            for w in &result.warnings {
                print_warning(w);
            }
            for path in &result.written {
                print_status("Generated", &path.display().to_string());
            }
            if purge {
                let dir = if input_path.is_dir() {
                    input_path.to_path_buf()
                } else {
                    input_path.parent().unwrap_or(Path::new(".")).to_path_buf()
                };
                for msg in purge_make(&dir) {
                    print_status("Removed", &msg);
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            std::process::exit(1);
        }
    }
}

pub fn cmd_migrate_cmake(input: &str, out_dir: Option<&str>, purge: bool) {
    let input_path = Path::new(input);
    let out = out_dir.map(Path::new);

    match import_cmake(input_path, out) {
        Ok(result) => {
            for w in &result.warnings {
                print_warning(w);
            }
            for path in &result.written {
                print_status("Generated", &path.display().to_string());
            }
            if purge {
                let dir = if input_path.is_dir() {
                    input_path.to_path_buf()
                } else {
                    input_path.parent().unwrap_or(Path::new(".")).to_path_buf()
                };
                for msg in purge_cmake(&dir) {
                    print_status("Removed", &msg);
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            std::process::exit(1);
        }
    }
}

pub fn cmd_migrate_autotools(input: &str, out_dir: Option<&str>, purge: bool) {
    let input_path = Path::new(input);
    let out = out_dir.map(Path::new);

    match import_autotools(input_path, out) {
        Ok(result) => {
            for w in &result.warnings {
                print_warning(w);
            }
            for path in &result.written {
                print_status("Generated", &path.display().to_string());
            }
            if purge {
                let dir = if input_path.is_dir() {
                    input_path.to_path_buf()
                } else {
                    input_path.parent().unwrap_or(Path::new(".")).to_path_buf()
                };
                for msg in purge_autotools(&dir) {
                    print_status("Removed", &msg);
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            std::process::exit(1);
        }
    }
}
