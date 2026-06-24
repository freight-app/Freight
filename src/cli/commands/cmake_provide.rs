//! `freight cmake-provide <name>` — internal helper invoked by the cmake plugin's
//! CMake dependency provider (see `plugins/cmake/cmake.freight`). Given a
//! `find_package` name, it makes freight's copy available (building + exporting it
//! if freight has it) and prints the install prefix to stdout for the provider to
//! add to `CMAKE_PREFIX_PATH`. Prints nothing when freight provides nothing (the
//! dep is already on the host, or freight has no copy) — the provider then lets
//! CMake's normal search run.
//!
//! This replaces the former standalone `freight-cmake-resolve` executable: the
//! provider calls back into `freight` directly, on demand, during configure.

use freight::build::pipeline::provide_cmake_package;
use freight::event::silent;
use freight::manifest::find_manifest_dir;

#[derive(clap::Args)]
pub struct Args {
    /// The `find_package` package name CMake is requesting.
    pub name: String,
    /// Build profile (defaults to `release` for dependency builds).
    #[arg(long, default_value = "release")]
    pub profile: String,
}

impl Args {
    pub fn run(self) {
        let cwd = match std::env::current_dir() {
            Ok(d) => d,
            Err(_) => return,
        };
        // The provider runs inside the dep's build tree; resolve the freight
        // project that owns the `.pkgs/` pool from the cwd upward.
        let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd);
        // Silent: this prints only the prefix on stdout (consumed by CMake);
        // build events must not pollute that.
        if let Some(prefix) =
            provide_cmake_package(&self.name, &project_dir, &self.profile, &silent())
        {
            println!("{}", prefix.display());
        }
    }
}
