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
    /// Adopt an existing foreign build (CMake): harvest `find_package` deps and
    /// convert vendored submodules / FetchContent / add_subdirectory into freight
    /// deps, instead of writing a plain native manifest.
    #[arg(long)]
    pub migrate: bool,
    /// With `--migrate`: extract real build data (sources, defines, include dirs,
    /// language standard) from CMake's File API and write a freight-native manifest,
    /// rather than a `build = "cmake"` self-build. Falls back to the self-build when
    /// the project's shape can't be represented natively. Implies `--migrate`.
    #[arg(long)]
    pub native: bool,
}

impl InitArgs {
    pub fn run(self) {
        cmd_init(self.lang.as_deref(), self.migrate || self.native, self.native);
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

pub fn cmd_init(lang: Option<&str>, migrate: bool, native: bool) {
    match init_project(lang, migrate, native) {
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
    match out.migrate_mode {
        Some("native") => {
            println!("  migrated from CMake — native manifest (sources/defines from File API)");
        }
        Some("cmake") => {
            println!("  migrated from CMake — foreign self-build (build = \"cmake\")");
        }
        _ => {}
    }
    if scaffolded {
        println!();
        println!("  cd {}", out.name);
        println!("  freight build");
        println!();
    }
    if !out.pruneable_paths.is_empty() {
        println!();
        println!(
            "  Converted {} vendored dependenc{} to freight deps.",
            out.pruneable_paths.len(),
            if out.pruneable_paths.len() == 1 { "y" } else { "ies" },
        );
        println!("  After a clean `freight build`, these trees can be removed:");
        for p in &out.pruneable_paths {
            println!("    git rm {p}");
        }
        println!();
    }
    if out.cmake_detected {
        println!();
        println!("  Detected a CMakeLists.txt in this directory.");
        println!("  Run `freight init --migrate` to adopt it (harvest deps, convert vendored imports).");
        println!();
    }
}
