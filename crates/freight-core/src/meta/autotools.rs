//! Autotools (configure/make) foreign build system integration.
use std::path::Path;

use crate::error::FreightError;
use super::run;

pub fn build_autotools(dep_dir: &Path, build_dir: &Path) -> Result<(), FreightError> {
    if !dep_dir.join("configure").exists() {
        if dep_dir.join("autogen.sh").exists() {
            run("sh", &["autogen.sh"], dep_dir, "autogen.sh")?;
        } else {
            run("autoreconf", &["-fi"], dep_dir, "autoreconf")?;
        }
    }

    // Install into .freight-build/install/ so libs and headers land in known locations.
    let install_dir = build_dir.join("install");
    std::fs::create_dir_all(&install_dir)?;
    let configure = dep_dir.join("configure").to_string_lossy().into_owned();
    let prefix    = format!("--prefix={}", install_dir.display());

    run(&configure, &[&prefix], dep_dir, "configure")?;
    run("make", &[], dep_dir, "make")?;
    run("make", &["install"], dep_dir, "make install")
}
