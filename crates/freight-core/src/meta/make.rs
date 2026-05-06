//! GNU Make foreign build system integration.
use std::path::Path;

use crate::error::FreightError;
use super::run;

pub fn build_make(dep_dir: &Path) -> Result<(), FreightError> {
    run("make", &[], dep_dir, "make")
}
