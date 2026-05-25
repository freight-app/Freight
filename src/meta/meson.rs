//! Meson + Ninja foreign build system integration.
use std::path::Path;

use crate::error::FreightError;
use super::run;

pub fn build_meson(dep_dir: &Path, build_dir: &Path) -> Result<(), FreightError> {
    if !build_dir.join("build.ninja").exists() {
        run("meson", &[
            "setup",
            &build_dir.to_string_lossy(),
            &dep_dir.to_string_lossy(),
        ], dep_dir, "meson setup")?;
    }
    let jobs = rayon::current_num_threads().to_string();
    run("ninja", &["-C", &build_dir.to_string_lossy(), "-j", &jobs], dep_dir, "ninja")
}
