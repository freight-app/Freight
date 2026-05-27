//! Meson + Ninja foreign build system integration.
use std::path::{Path, PathBuf};

use crate::error::FreightError;
use super::run;

pub fn build_meson(dep_dir: &Path, build_dir: &Path, tool_paths: &[PathBuf]) -> Result<(), FreightError> {
    if !build_dir.join("build.ninja").exists() {
        run("meson", &[
            "setup",
            &build_dir.to_string_lossy(),
            &dep_dir.to_string_lossy(),
        ], dep_dir, "meson setup", tool_paths)?;
    }
    let jobs = rayon::current_num_threads().to_string();
    run("ninja", &["-C", &build_dir.to_string_lossy(), "-j", &jobs], dep_dir, "ninja", tool_paths)
}
