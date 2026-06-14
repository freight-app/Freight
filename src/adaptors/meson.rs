//! Meson + Ninja foreign build system integration.
use std::path::{Path, PathBuf};

use super::run;
use crate::error::FreightError;

pub fn build_meson(
    dep_dir: &Path,
    build_dir: &Path,
    defines: &[String],
    tool_paths: &[PathBuf],
) -> Result<(), FreightError> {
    if !build_dir.join("build.ninja").exists() {
        let build_s = build_dir.to_string_lossy();
        let dep_s = dep_dir.to_string_lossy();
        // meson project options: `-DKEY=VALUE`.
        let mut args: Vec<&str> = vec!["setup", &build_s, &dep_s];
        args.extend(defines.iter().map(String::as_str));
        run("meson", &args, dep_dir, "meson setup", tool_paths)?;
    }
    let jobs = rayon::current_num_threads().to_string();
    run(
        "ninja",
        &["-C", &build_dir.to_string_lossy(), "-j", &jobs],
        dep_dir,
        "ninja",
        tool_paths,
    )
}
