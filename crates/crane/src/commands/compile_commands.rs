use crane_core::build::generate_compile_commands_at;
use crane_core::manifest::find_manifest_dir;

use crate::output::{print_error, print_success};

pub fn cmd_compile_commands(release: bool) {
    let profile = if release { "release" } else { "dev" };
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("{e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no crane.toml found in current directory or any parent"); return; }
    };

    match generate_compile_commands_at(&project_dir, profile) {
        Ok(n) => {
            print_success(&format!("Generated compile_commands.json with {n} entr{}", if n == 1 { "y" } else { "ies" }));
            if !release {
                println!("  tip: run `crane build` first for complete dependency include paths");
            }
        }
        Err(e) => print_error(&format!("{e}")),
    }
}
