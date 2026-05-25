use std::path::Path;

use freight_core::migration::autotools::{import_autotools, purge_autotools};
use freight_core::migration::cmake::{import_cmake, purge_cmake};
use freight_core::migration::make::{import_make, purge_make};

use crate::output::{print_error, print_status, print_warning};

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
