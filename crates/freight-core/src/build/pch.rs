use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FreightError;
use crate::toolchain::DetectedCompiler;

/// Result of compiling a PCH.
pub struct CompiledPch {
    /// Flag(s) to inject into real compiler invocations (e.g. `-include-pch /path/to/pch`).
    pub use_flag: String,
    /// Flag for `compile_commands.json` — always `-include {original_header}` so clangd
    /// reads the source header directly rather than an opaque binary PCH it can't use.
    pub clangd_flag: String,
}

/// Compile `header_rel` to a PCH and return the flag string to inject.
///
/// Returns `None` if the compiler has no PCH config (empty compile/use fields).
/// Skips recompilation if the PCH output is already newer than the header.
pub fn compile_pch(
    project_dir: &Path,
    header_rel: &str,
    profile: &str,
    compiler: &DetectedCompiler,
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    extra_flags: &[String],
) -> Result<Option<CompiledPch>, FreightError> {
    let pch_cfg = &compiler.template.pch;
    if pch_cfg.compile.is_empty() || pch_cfg.use_flag.is_empty() {
        return Ok(None);
    }

    let header_path = project_dir.join(header_rel);
    if !header_path.exists() {
        eprintln!("warning: pch header '{}' not found, skipping PCH", header_rel);
        return Ok(None);
    }

    let stem = header_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pch");
    let pch_dir = project_dir.join("target").join(profile).join("pch");
    std::fs::create_dir_all(&pch_dir)?;
    let pch_out = pch_dir.join(format!("{}{}", stem, pch_cfg.extension));

    // Dirty check: skip if PCH is newer than the header.
    if pch_out.exists() {
        let header_mtime = std::fs::metadata(&header_path)
            .and_then(|m| m.modified())
            .ok();
        let pch_mtime = std::fs::metadata(&pch_out)
            .and_then(|m| m.modified())
            .ok();
        if let (Some(hm), Some(pm)) = (header_mtime, pch_mtime) {
            if pm >= hm {
                let use_flag = expand_pch_use_flag(&pch_cfg.use_flag, &header_path, &pch_out);
                let clangd_flag = format!("-include {}", header_path.display());
                return Ok(Some(CompiledPch { use_flag, clangd_flag }));
            }
        }
    }

    // Compile.
    let mut cmd = Command::new(&compiler.template.binary);
    for dir in include_dirs {
        cmd.arg(format!("-I{}", dir.display()));
    }
    for d in feature_defines {
        cmd.arg(d);
    }
    for f in extra_flags {
        cmd.arg(f);
    }
    for flag in pch_cfg.compile.split_whitespace() {
        cmd.arg(flag);
    }
    cmd.arg(&header_path);
    cmd.arg("-o").arg(&pch_out);

    let status = cmd
        .status()
        .map_err(|e| FreightError::CompileFailed(format!("PCH compile failed: {e}"), String::new()))?;
    if !status.success() {
        return Err(FreightError::CompileFailed(
            format!("PCH compilation of '{}' failed", header_rel),
            String::new(),
        ));
    }

    let use_flag = expand_pch_use_flag(&pch_cfg.use_flag, &header_path, &pch_out);
    let clangd_flag = format!("-include {}", header_path.display());
    Ok(Some(CompiledPch { use_flag, clangd_flag }))
}

fn expand_pch_use_flag(template: &str, header_path: &Path, pch_path: &Path) -> String {
    template
        .replace("{header_path}", &header_path.display().to_string())
        .replace("{pch_path}", &pch_path.display().to_string())
}
