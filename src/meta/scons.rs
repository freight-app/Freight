//! SCons foreign build system integration.
use std::path::{Path, PathBuf};

use crate::error::FreightError;
use super::run;

pub fn build_scons(dep_dir: &Path, tool_paths: &[PathBuf]) -> Result<(), FreightError> {
    run("scons", &[], dep_dir, "scons", tool_paths)
}
