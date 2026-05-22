use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

use crate::manifest::types::Backend;
use crate::toolchain::DetectedCompiler;
use super::compile::{is_up_to_date, select_compiler};

/// A header that has been precompiled as a C++20 header unit BMI.
pub struct HeaderUnit {
    /// Path relative to its include directory — matches what users write in `import "..."`.
    pub rel_path: String,
    /// Absolute path to the compiled `.pcm` file.
    pub pcm_path: PathBuf,
}

/// Returns true when `std` is C++20 or later and header units are meaningful.
pub fn is_module_std(std: &str) -> bool {
    matches!(std, "c++20" | "c++23" | "c++26")
}

/// Precompile every `.h` / `.hpp` file found under `include_dirs` as a C++20 header unit.
///
/// Only runs when the detected C++ compiler supports header unit precompilation
/// (currently clang with `header_unit_flag` set in its template). Errors from
/// individual headers are printed as warnings and skipped rather than aborting.
///
/// PCMs are stored under `target/{profile}/header-units/` and are only recompiled
/// when the header's mtime is newer than the cached PCM.
pub fn precompile_dep_headers(
    project_dir: &Path,
    include_dirs: &[PathBuf],
    cpp_std: &str,
    backend: &Backend,
    detected: &[DetectedCompiler],
    profile: &str,
) -> Vec<HeaderUnit> {
    if include_dirs.is_empty() { return vec![]; }

    let Some(compiler) = select_compiler("cpp", backend, detected, None) else { return vec![]; };
    if !compiler.template.supports_header_units() { return vec![]; }

    let std_flag = match compiler.template.standards.get(cpp_std) {
        Some(f) => f.clone(),
        None => return vec![],
    };

    let hu_dir = project_dir.join("target").join(profile).join("header-units");

    let mut units: Vec<HeaderUnit> = Vec::new();

    for inc_dir in include_dirs {
        if !inc_dir.is_dir() { continue; }

        // Build the -I flag for this include dir so headers that include siblings compile.
        let i_flag = compiler.template.structure.include_dir
            .replace("{path}", &inc_dir.to_string_lossy());
        let include_flags: Vec<String> = i_flag.split_whitespace().map(str::to_owned).collect();

        for entry in WalkDir::new(inc_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let header_abs = entry.path();
            let ext = header_abs.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "h" | "hpp" | "hh" | "hxx") { continue; }

            let rel_path = match header_abs.strip_prefix(inc_dir) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };

            let pcm_path = hu_dir.join(&rel_path).with_extension("h.pcm");

            // Dirty check: PCM newer than header → skip.
            let dummy_dep = PathBuf::new(); // no .d file for header units
            if is_up_to_date(header_abs, &pcm_path, &dummy_dep) {
                units.push(HeaderUnit { rel_path, pcm_path });
                continue;
            }

            if let Some(parent) = pcm_path.parent() {
                if std::fs::create_dir_all(parent).is_err() { continue; }
            }

            let Some((binary, args)) = compiler.template.precompile_header_unit_cmd(
                header_abs, &pcm_path, &std_flag, &include_flags,
            ) else { continue; };

            let status = Command::new(&binary).args(&args).output();
            match status {
                Ok(out) if out.status.success() => {
                    units.push(HeaderUnit { rel_path, pcm_path });
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!("warning: header unit precompile skipped for {rel_path}: {stderr}");
                }
                Err(e) => {
                    eprintln!("warning: header unit precompile failed for {rel_path}: {e}");
                }
            }
        }
    }

    units
}

/// Build the `-fmodule-file=rel_path=pcm_path` flags for each header unit.
pub fn import_flags(units: &[HeaderUnit], compiler: &DetectedCompiler) -> Vec<String> {
    units.iter()
        .filter_map(|u| compiler.template.header_unit_import_flag(&u.rel_path, &u.pcm_path))
        .collect()
}
