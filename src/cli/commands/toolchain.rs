use freight::toolchain::detect::DetectedCompiler;
use freight::toolchain::{
    backend_matches, detect_all_cached, detect_debuggers, group_into_toolchains,
    load_all_templates, load_debugger_templates, toolchain_use,
};

use crate::output::{print_error, print_success, print_warning};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: ToolchainCmd,
}

#[derive(clap::Subcommand)]
pub enum ToolchainCmd {
    /// Show detected compilers
    List,
    /// Set the default compiler backend
    Use { name: String },
}

impl Args {
    pub fn run(self) {
        match self.command {
            ToolchainCmd::List => cmd_toolchain_list(),
            ToolchainCmd::Use { name } => cmd_toolchain_use(&name),
        }
    }
}

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

        // One row per (family, version) — compilers sharing a version are merged.
        let mut toolchain_rows: Vec<Vec<String>> = Vec::new();
        for tc in &groups.toolchains {
            // Collect unique versions in the order they first appear.
            let mut versions: Vec<&str> = Vec::new();
            for c in &tc.compilers {
                if !versions.contains(&c.version.as_str()) {
                    versions.push(&c.version);
                }
            }
            for ver in versions {
                let same_ver: Vec<&DetectedCompiler> =
                    tc.compilers.iter().filter(|c| c.version == ver).collect();

                // Primary name: prefer the C compiler, then shortest non-++ name.
                let primary = same_ver
                    .iter()
                    .filter(|c| {
                        !c.template.name.ends_with("++") && c.template.linking.contains_key("c")
                    })
                    .min_by_key(|c| c.template.name.len())
                    .or_else(|| {
                        same_ver
                            .iter()
                            .filter(|c| !c.template.name.ends_with("++"))
                            .min_by_key(|c| c.template.name.len())
                    })
                    .map(|c| c.template.name.as_str())
                    .unwrap_or(&tc.name);

                let major = ver.split('.').next().unwrap_or(ver);
                let label = format!("{primary}-{major}");

                let mut langs: Vec<&str> = same_ver
                    .iter()
                    .flat_map(|c| c.template.linking.keys().map(String::as_str))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                langs.sort_unstable();

                toolchain_rows.push(vec![label, ver.to_string(), langs.join(", ")]);
            }
        }
        print_table(&["Compiler", "Version", "Languages"], &toolchain_rows);

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
