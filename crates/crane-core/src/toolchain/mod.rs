pub mod cache;
pub mod detect;
pub mod template;

pub use cache::{ToolchainCache, crane_home};
pub use detect::{DetectedCompiler, detect_all, detect_all_cached, load_templates, templates_dir};
pub use template::{BuildSettings, CompilerTemplate, ModuleStyle};

use crate::output::print_warning;

/// Run `crane toolchain list` — detect and print all available compilers.
pub fn cmd_toolchain_list() {
    let Some(dir) = templates_dir() else {
        print_warning("compiler-templates directory not found; set CRANE_TEMPLATES_DIR");
        return;
    };

    let templates = load_templates(&dir);
    if templates.is_empty() {
        print_warning("no compiler templates loaded");
        return;
    }

    let detected = detect_all_cached(&templates);
    if detected.is_empty() {
        println!("No supported compilers found on PATH.");
        println!("Templates loaded from: {}", dir.display());
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
