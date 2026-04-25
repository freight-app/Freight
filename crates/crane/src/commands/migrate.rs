use crane_migrator::{parse_format, run_migrate};

use crate::output::{print_error, print_status, print_success, print_warning};

pub fn cmd_migrate(from: Option<&str>, dry_run: bool, force: bool) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };

    let fmt = match from {
        Some(s) => match parse_format(s) {
            Ok(f) => Some(f),
            Err(e) => { print_error(&e.to_string()); return; }
        },
        None => None,
    };

    if let Some(f) = fmt {
        print_status("Importing", &format!("{f} project at {}", cwd.display()));
    } else {
        print_status("Importing", &format!("auto-detected project at {}", cwd.display()));
    }

    match run_migrate(&cwd, fmt, dry_run, force) {
        Ok(outcome) => {
            if outcome.is_workspace {
                if outcome.written_to.is_none() {
                    // Dry-run: print workspace root + member manifests.
                    println!("# === workspace root crane.toml ===");
                    print!("{}", outcome.toml);
                    for (dir, content) in &outcome.workspace_members {
                        println!("\n# === {dir}/crane.toml ===");
                        print!("{content}");
                    }
                    return;
                }
                if outcome.note_count > 0 {
                    print_warning(&format!(
                        "{} construct(s) could not be imported — see `# CRANE:` comments",
                        outcome.note_count,
                    ));
                }
                print_success(&format!(
                    "workspace with {} member(s) written",
                    outcome.workspace_members.len(),
                ));
                if let Some(root) = &outcome.written_to {
                    println!("    {}", root.display());
                }
                for (dir, _) in &outcome.workspace_members {
                    let member_path = cwd.join(dir).join("crane.toml");
                    println!("    {}", member_path.display());
                }
                return;
            }

            // Single-project path.
            if outcome.written_to.is_none() {
                print!("{}", outcome.toml);
                return;
            }
            if outcome.note_count > 0 {
                print_warning(&format!(
                    "{} construct(s) could not be imported — see `# CRANE:` comments in crane.toml",
                    outcome.note_count,
                ));
            }
            if let Some(path) = outcome.written_to {
                print_success(&format!("wrote {}", path.display()));
            }
        }
        Err(e) => print_error(&e.to_string()),
    }
}
