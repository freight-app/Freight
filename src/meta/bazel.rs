//! Bazel foreign build system integration.
use std::path::{Path, PathBuf};

use crate::error::FreightError;
use super::run;

pub fn build_bazel(dep_dir: &Path, tool_paths: &[PathBuf]) -> Result<(), FreightError> {
    run("bazel", &["build", "//..."], dep_dir, "bazel build", tool_paths)
}
