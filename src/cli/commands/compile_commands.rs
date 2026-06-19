use freight::build::{compile_commands, generate_compile_commands_at};
use freight::manifest::{find_manifest_dir, load_workspace_manifest};

use crate::output::{print_error, print_success};

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    pub release: bool,
}

impl Args {
    pub fn run(self) {
        cmd_compile_commands(self.release);
    }
}

pub fn cmd_compile_commands(release: bool) {
    let profile = if release { "release" } else { "debug" };
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("{e}"));
            return;
        }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => {
            print_error("no freight.toml found in current directory or any parent");
            return;
        }
    };

    // Workspace root: regenerate each member then write a merged root-level DB.
    if let Some(ws) = load_workspace_manifest(&project_dir) {
        let mut total = 0usize;
        let mut member_dirs = Vec::new();

        for member in &ws.members {
            let member_dir = project_dir.join(member.trim_end_matches('/'));
            match generate_compile_commands_at(&member_dir, profile) {
                Ok(n) => {
                    total += n;
                    member_dirs.push(member_dir);
                }
                Err(e) => {
                    print_error(&format!("{}: {e}", member));
                    return;
                }
            }
        }

        // Merge into workspace root.
        let mut all: Vec<compile_commands::CompileCommand> = Vec::new();
        for dir in &member_dirs {
            all.extend(compile_commands::load(dir));
        }
        all.sort_by(|a, b| a.file.cmp(&b.file));
        if let Err(e) = compile_commands::write(&project_dir, &all) {
            print_error(&format!(
                "could not write workspace compile_commands.json: {e}"
            ));
            return;
        }

        print_success(&format!(
            "Generated compile_commands.json ({total} entr{} across {} member{})",
            if total == 1 { "y" } else { "ies" },
            member_dirs.len(),
            if member_dirs.len() == 1 { "" } else { "s" },
        ));
        return;
    }

    // Single project.
    match generate_compile_commands_at(&project_dir, profile) {
        Ok(n) => print_success(&format!(
            "Generated compile_commands.json with {n} entr{}",
            if n == 1 { "y" } else { "ies" },
        )),
        Err(e) => print_error(&format!("{e}")),
    }
}
