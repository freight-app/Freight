//! SCons foreign build system integration.
use std::path::{Path, PathBuf};

use super::run;
use crate::error::FreightError;

pub fn build_scons(dep_dir: &Path, tool_paths: &[PathBuf]) -> Result<(), FreightError> {
    run("scons", &[], dep_dir, "scons", tool_paths)
}
