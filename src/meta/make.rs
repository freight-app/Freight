//! GNU Make foreign build system integration.
use std::path::{Path, PathBuf};

use crate::error::FreightError;
use super::run;

pub fn build_make(dep_dir: &Path, tool_paths: &[PathBuf]) -> Result<(), FreightError> {
    let jobs = rayon::current_num_threads().to_string();
    run("make", &["-j", &jobs], dep_dir, "make", tool_paths)
}
