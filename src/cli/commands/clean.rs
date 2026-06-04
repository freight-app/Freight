use freight::build::{clean_project, clean_workspace};

use crate::output::{print_error, print_success};

#[derive(clap::Args)]
pub struct Args {}

impl Args {
    pub fn run(self) {
        cmd_clean();
    }
}

fn cmd_clean() {
    if super::build::at_workspace_root() {
        match clean_workspace() {
            Ok(()) => print_success("cleaned all workspace member target/ directories"),
            Err(e) => {
                println!();
                print_error(&e.to_string());
            }
        }
        return;
    }

    match clean_project() {
        Ok(()) => print_success("cleaned target/"),
        Err(e) => {
            println!();
            print_error(&e.to_string());
        }
    }
}
