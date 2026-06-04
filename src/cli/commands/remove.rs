use freight::dep_cmds::manifest_remove_dep;

use crate::output::{print_error, print_success};

#[derive(clap::Args)]
pub struct Args {
    pub package: String,
}

impl Args {
    pub fn run(self) {
        cmd_remove(&self.package);
    }
}

fn cmd_remove(package: &str) {
    let project_dir = match super::common::locate_project_dir() {
        Some(d) => d,
        None => return,
    };

    match manifest_remove_dep(&project_dir.join("freight.toml"), package) {
        Ok(true) => {
            print_success(&format!("removed `{package}`"));
            super::common::refresh_lock(&project_dir);
        }
        Ok(false) => {
            print_error(&format!(
                "`{package}` not found in [dependencies] or [dev-dependencies]"
            ));
        }
        Err(e) => print_error(&e.to_string()),
    }
}
