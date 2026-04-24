use std::path::Path;

use crane_core::toolchain::{detect_all_cached, load_all_templates, toolchain_add, user_templates_dir};

use crate::output::{print_error, print_success, print_warning};

pub fn cmd_toolchain_list() {
    let templates = load_all_templates();
    if templates.is_empty() {
        print_warning("no compiler templates loaded");
        return;
    }

    let detected = detect_all_cached(&templates);
    if detected.is_empty() {
        println!("No supported compilers found on PATH.");
        return;
    }

    println!("{:<12} {:<12} {}", "Compiler", "Version", "Path");
    println!("{}", "-".repeat(60));
    for d in &detected {
        println!(
            "{:<12} {:<12} {}",
            d.template.name,
            d.version,
            d.path.display()
        );
    }
}

pub fn cmd_toolchain_add(path: &str) {
    match toolchain_add(Path::new(path)) {
        Ok(dest) => {
            print_success(&format!("template installed to {}", dest.display()));
            if let Some(user_dir) = user_templates_dir() {
                println!("  User templates directory: {}", user_dir.display());
            }
        }
        Err(e) => print_error(&format!("failed to install template: {e}")),
    }
}
