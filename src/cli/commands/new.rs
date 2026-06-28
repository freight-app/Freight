use freight::new::{init_project, scaffold_project, ScaffoldOutcome};

use crate::output::{print_error, print_success};

#[derive(clap::Args)]
pub struct Args {
    pub name: String,
    #[arg(long, default_value = "c++")]
    pub lang: String,
}

impl Args {
    pub fn run(self) {
        cmd_new(&self.name, &self.lang);
    }
}

#[derive(clap::Args)]
pub struct InitArgs {
    #[arg(long)]
    pub lang: Option<String>,
}

impl InitArgs {
    pub fn run(self) {
        cmd_init(self.lang.as_deref());
    }
}

pub fn cmd_new(name: &str, lang: &str) {
    match scaffold_project(name, lang) {
        Ok(out) => print_created(&out, true),
        Err(e) => {
            print_error(&e.to_string());
            std::process::exit(1);
        }
    }
}

pub fn cmd_init(lang: Option<&str>) {
    match init_project(lang) {
        Ok(out) => print_created(&out, false),
        Err(e) => {
            print_error(&e.to_string());
            std::process::exit(1);
        }
    }
}

fn print_created(out: &ScaffoldOutcome, scaffolded: bool) {
    let verb = if scaffolded { "created" } else { "initialized" };
    print_success(&format!("{verb} `{}` ({} project)", out.name, out.language));
    if scaffolded {
        println!();
        println!("  cd {}", out.name);
        println!("  freight build");
        println!();
    }
}
