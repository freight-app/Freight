use std::path::Path;

use freight_core::toolchain::{
    backend_matches, detect_all_cached, detect_debuggers, group_into_toolchains, load_all_templates,
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

        let toolchain_rows: Vec<Vec<String>> = groups
            .toolchains
            .iter()
            .map(|tc| {
                let compilers: Vec<String> = tc
                    .compilers
                    .iter()
                    .map(|c| format!("{} {}", c.template.name, c.version))
                    .collect();
                let cpu_extensions = tc
                    .compilers
                    .iter()
                    .flat_map(|c| c.cpu_extensions.iter().map(String::as_str))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", ");

                vec![
                    tc.name.clone(),
                    tc.languages.join(", "),
                    compilers.join(", "),
                    display_or_dash(cpu_extensions),
                ]
            })
            .collect();
        print_table(
            &["Toolchain", "Languages", "Compilers", "CPU Extensions"],
            &toolchain_rows,
        );

        if !groups.guests.is_empty() {
            println!();
            println!("Guest extensions (extend the active toolchain):");
            let guest_rows: Vec<Vec<String>> = groups
                .guests
                .iter()
                .map(|g| {
                    let mut langs: Vec<&str> =
                        g.template.linking.keys().map(String::as_str).collect();
                    langs.sort_unstable();

                    vec![
                        g.template.name.clone(),
                        langs.join(", "),
                        g.version.clone(),
                        format!("host: {}", g.template.requires_toolchain.join(", ")),
                    ]
                })
                .collect();
            print_table(
                &["Compiler", "Languages", "Version", "Requires"],
                &guest_rows,
            );
        }
    }

    // Show debuggers in a separate section.
    let dbg_templates = load_debugger_templates();
    let debuggers = detect_debuggers(&dbg_templates);
    if !debuggers.is_empty() {
        println!();
        let debugger_rows: Vec<Vec<String>> = debuggers
            .iter()
            .map(|d| {
                let dap = d
                    .dap_path
                    .as_ref()
                    .map(|p| format!(" (dap: {})", p.display()))
                    .unwrap_or_default();
                vec![
                    d.template.name.clone(),
                    d.version.clone(),
                    format!("{}{}", d.path.display(), dap),
                ]
            })
            .collect();
        print_table(&["Debugger", "Version", "Path"], &debugger_rows);
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
            let on_path = detected.iter().any(|d| backend_matches(d, name));
            if on_path {
                print_success(&format!("{name} set as default toolchain"));
            } else {
                print_warning(&format!(
                    "{name} is not currently detected on PATH; \
                     preference saved and will apply once it is installed"
                ));
            }
        }
        Err(e) => print_error(&format!("failed to set default toolchain: {e}")),
    }
}

fn display_or_dash(value: String) -> String {
    if value.is_empty() {
        "—".to_string()
    } else {
        value
    }
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, header)| {
            rows.iter()
                .filter_map(|row| row.get(i))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
                .max(header.chars().count())
        })
        .collect();

    print_table_border(&widths);
    print_table_row(
        &headers.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
        &widths,
    );
    print_table_border(&widths);
    for row in rows {
        print_table_row(row, &widths);
    }
    print_table_border(&widths);
}

fn print_table_border(widths: &[usize]) {
    print!("+");
    for width in widths {
        print!("-{}-+", "-".repeat(*width));
    }
    println!();
}

fn print_table_row(row: &[String], widths: &[usize]) {
    print!("|");
    for (i, width) in widths.iter().enumerate() {
        let cell = row.get(i).map(String::as_str).unwrap_or("");
        print!(
            " {}{} |",
            cell,
            " ".repeat(width.saturating_sub(cell.chars().count()))
        );
    }
    println!();
}
