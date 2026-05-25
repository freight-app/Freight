//! Autotools (configure/make) foreign build system integration.
//!
//! Adapted from autotools-rs. Key features retained:
//! - `--host=<triple>` injected for cross-compilation
//! - Parallel `make -j{N}` using all available CPUs
//! - Fast-build: skips the configure step when `config.status` and `Makefile`
//!   already exist and the `configure` script hasn't changed since then
//! - Emscripten support: uses `emconfigure`/`emmake` for wasm/emscripten targets
//! - Always passes `--enable-static --disable-shared`
use std::path::Path;

use crate::error::FreightError;
use super::run;

pub fn build_autotools(
    dep_dir: &Path,
    build_dir: &Path,
    target: Option<&str>,
) -> Result<(), FreightError> {
    let use_emscripten = target
        .map(|t| t.contains("wasm") || t.contains("emscripten"))
        .unwrap_or(false);

    // Generate configure script if missing.
    if !dep_dir.join("configure").exists() {
        if dep_dir.join("autogen.sh").exists() {
            if use_emscripten {
                run("emconfigure", &["sh", "autogen.sh"], dep_dir, "autogen.sh")?;
            } else {
                run("sh", &["autogen.sh"], dep_dir, "autogen.sh")?;
            }
        } else {
            run("autoreconf", &["-fi"], dep_dir, "autoreconf")?;
        }
    }

    let install_dir = build_dir.join("install");
    std::fs::create_dir_all(&install_dir)?;

    // Configure step — skipped when already up-to-date.
    if !configure_up_to_date(dep_dir) {
        let configure = dep_dir.join("configure").to_string_lossy().into_owned();
        let prefix    = format!("--prefix={}", install_dir.display());

        let mut args: Vec<String> = vec![
            prefix,
            "--enable-static".into(),
            "--disable-shared".into(),
        ];
        if let Some(triple) = target {
            args.push(format!("--host={triple}"));
        }

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        if use_emscripten {
            let mut full: Vec<&str> = vec![&configure];
            full.extend_from_slice(&arg_refs);
            run("emconfigure", &full, dep_dir, "configure")?;
        } else {
            run(&configure, &arg_refs, dep_dir, "configure")?;
        }
    }

    // Build step — cap at MAX_JOBS to avoid saturating all cores.
    let jobs     = std::thread::available_parallelism().map(|n| n.get().min(super::MAX_JOBS)).unwrap_or(1);
    let jobs_str = jobs.to_string();

    if use_emscripten {
        run("emmake", &["make", "-j", &jobs_str], dep_dir, "make")?;
        run("emmake", &["make", "install"],        dep_dir, "make install")?;
    } else {
        run("make", &["-j", &jobs_str], dep_dir, "make")?;
        run("make", &["install"],       dep_dir, "make install")?;
    }

    Ok(())
}

/// Returns `true` when configure output is already present and up-to-date:
/// both `config.status` and `Makefile` exist and `configure` is not newer
/// than `config.status`.
fn configure_up_to_date(dep_dir: &Path) -> bool {
    let config_status = dep_dir.join("config.status");
    let makefile      = dep_dir.join("Makefile");
    let configure     = dep_dir.join("configure");

    if !config_status.exists() || !makefile.exists() || !configure.exists() {
        return false;
    }
    let (Ok(c_meta), Ok(cs_meta)) = (
        std::fs::metadata(&configure),
        std::fs::metadata(&config_status),
    ) else {
        return false;
    };
    let (Ok(c_mtime), Ok(cs_mtime)) = (c_meta.modified(), cs_meta.modified()) else {
        return false;
    };
    c_mtime <= cs_mtime
}
