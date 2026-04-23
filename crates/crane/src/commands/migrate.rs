use crane_core::importer::{parse_format, run_migrate};

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

    // Status line is emitted up front so the user sees what we're doing even
    // if parsing later fails.
    if let Some(f) = fmt {
        print_status("Importing", &format!("{f} project at {}", cwd.display()));
    } else {
        print_status("Importing", &format!("auto-detected project at {}", cwd.display()));
    }

    match run_migrate(&cwd, fmt, dry_run, force) {
        Ok(outcome) => {
            if outcome.written_to.is_none() {
                // Dry-run: print the would-be manifest to stdout.
                print!("{}", outcome.toml);
                return;
            }
            if outcome.note_count > 0 {
                print_warning(&format!(
                    "{} construct(s) could not be imported — see `# CRANE:` comments in crane.toml",
                    outcome.note_count
                ));
            }
            if let Some(path) = outcome.written_to {
                print_success(&format!("wrote {}", path.display()));
            }
        }
        Err(e) => print_error(&e.to_string()),
    }
}
