//! Bazel foreign build system integration.
use std::path::Path;

use crate::error::FreightError;
use super::run;

pub fn build_bazel(dep_dir: &Path) -> Result<(), FreightError> {
    run("bazel", &["build", "//..."], dep_dir, "bazel build")
}
