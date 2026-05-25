//! GNU Make foreign build system integration.
use std::path::Path;

use crate::error::FreightError;
use super::{MAX_JOBS, run};

pub fn build_make(dep_dir: &Path) -> Result<(), FreightError> {
    let jobs = std::thread::available_parallelism()
        .map(|n| n.get().min(MAX_JOBS))
        .unwrap_or(1)
        .to_string();
    run("make", &["-j", &jobs], dep_dir, "make")
}
