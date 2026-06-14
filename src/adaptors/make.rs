//! GNU Make foreign build system integration.
use std::path::{Path, PathBuf};

use super::run;
use crate::error::FreightError;

pub fn build_make(
    dep_dir: &Path,
    defines: &[String],
    tool_paths: &[PathBuf],
) -> Result<(), FreightError> {
    let jobs = rayon::current_num_threads().to_string();
    // make variable assignments (`KEY=VALUE`) are positional arguments.
    let mut args: Vec<&str> = vec!["-j", &jobs];
    args.extend(defines.iter().map(String::as_str));
    run("make", &args, dep_dir, "make", tool_paths)
}
