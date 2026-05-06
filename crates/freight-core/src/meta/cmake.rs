//! CMake foreign build system integration.
use std::path::Path;

use crate::error::FreightError;
use super::run;

pub fn build_cmake(
    dep_dir: &Path,
    build_dir: &Path,
    profile: &str,
    extra_args: &[String],
) -> Result<(), FreightError> {
    let build_type = if profile == "release" { "Release" } else { "Debug" };

    let src   = dep_dir.to_string_lossy().into_owned();
    let bdir  = build_dir.to_string_lossy().into_owned();
    let btype = format!("-DCMAKE_BUILD_TYPE={build_type}");

    let mut configure_args: Vec<&str> = vec!["-S", &src, "-B", &bdir, &btype];
    for a in extra_args { configure_args.push(a.as_str()); }

    run("cmake", &configure_args, dep_dir, "cmake configure")?;
    run("cmake", &["--build", &bdir], dep_dir, "cmake build")
}
