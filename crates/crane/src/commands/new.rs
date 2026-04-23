use crane_core::new::{init_project, scaffold_project, ScaffoldOutcome};

use crate::output::{print_error, print_success};

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
        println!("  crane build");
        println!();
    }
}
