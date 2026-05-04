use std::path::Path;

use freight_core::toolchain::{
    detect_all_cached, detect_debuggers, load_all_templates, load_debugger_templates,
    toolchain_add, user_templates_dir,
};

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
    } else {
        println!("{:<12} {:<12} {}", "Compiler", "Version", "Path");
        println!("{}", "-".repeat(60));
        for d in &detected {
            println!("{:<12} {:<12} {}", d.template.name, d.version, d.path.display());
        }
    }

    // Show debuggers in a separate section.
    let dbg_templates = load_debugger_templates();
    let debuggers = detect_debuggers(&dbg_templates);
    if !debuggers.is_empty() {
        println!();
        println!("{:<12} {:<12} {}", "Debugger", "Version", "Path");
        println!("{}", "-".repeat(60));
        for d in &debuggers {
            let dap = d.dap_path.as_ref()
                .map(|p| format!("  (dap: {})", p.display()))
                .unwrap_or_default();
            println!("{:<12} {:<12} {}{}", d.template.name, d.version, d.path.display(), dap);
        }
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
