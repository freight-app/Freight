use std::path::Path;

use freight_core::toolchain::{
    detect_all_cached, detect_debuggers, group_into_toolchains, load_all_templates,
    load_debugger_templates, toolchain_add, toolchain_use, user_templates_dir,
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
        let groups = group_into_toolchains(detected);

        println!("{:<12} {:<28} {}", "Toolchain", "Languages", "Compilers");
        println!("{}", "-".repeat(72));
        for tc in &groups.toolchains {
            let langs = tc.languages.join(", ");
            let compilers: Vec<String> = tc.compilers
                .iter()
                .map(|c| format!("{} {}", c.template.name, c.version))
                .collect();
            println!("{:<12} {:<28} {}", tc.name, langs, compilers.join(", "));
        }

        if !groups.guests.is_empty() {
            println!();
            println!("Guest extensions (extend the active toolchain):");
            println!("{:<12} {:<16} {:<12} {}", "Compiler", "Languages", "Version", "Requires");
            println!("{}", "-".repeat(60));
            for g in &groups.guests {
                let mut langs: Vec<&str> =
                    g.template.linking.keys().map(String::as_str).collect();
                langs.sort_unstable();
                println!(
                    "{:<12} {:<16} {:<12} host: {}",
                    g.template.name,
                    langs.join(", "),
                    g.version,
                    g.template.requires_toolchain.join(", "),
                );
            }
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

pub fn cmd_toolchain_use(name: &str) {
    let templates = load_all_templates();
    match toolchain_use(name, &templates) {
        Ok(()) => {
            let detected = detect_all_cached(&templates);
            let groups = group_into_toolchains(detected);
            if !groups.toolchains.iter().any(|tc| tc.name == name) {
                print_warning(&format!(
                    "{name} is not currently detected on PATH; \
                     preference saved and will apply once it is installed"
                ));
            } else {
                print_success(&format!("{name} set as default toolchain"));
            }
        }
        Err(e) => print_error(&format!("failed to set default toolchain: {e}")),
    }
}
