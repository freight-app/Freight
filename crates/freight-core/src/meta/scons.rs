//! SCons foreign build system integration.
use std::path::Path;

use crate::error::FreightError;
use super::run;

pub fn build_scons(dep_dir: &Path) -> Result<(), FreightError> {
    run("scons", &[], dep_dir, "scons")
}
