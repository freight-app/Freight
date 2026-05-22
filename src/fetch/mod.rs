pub mod git;
pub mod http;

pub use git::*;
pub use http::fetch_url_dep;

use std::path::Path;
use crate::error::FreightError;

/// Apply a list of patch files to `dep_dir` using `patch -p1`.
/// `patch_paths` are resolved relative to `project_dir`.
/// Already-applied patches are not tracked — callers must only invoke this
/// once per fetch (i.e. before writing the `.freight-fetched` sentinel).
pub fn apply_patches(
    dep_dir: &Path,
    patch_paths: &[String],
    project_dir: &Path,
) -> Result<(), FreightError> {
    for rel in patch_paths {
        let patch_file = project_dir.join(rel);
        if !patch_file.exists() {
            return Err(FreightError::ManifestParse(format!(
                "patch file not found: {}",
                patch_file.display()
            )));
        }

        let patch_data = std::fs::read(&patch_file)?;

        let mut child = std::process::Command::new("patch")
            .args(["-p1", "--batch", "--forward"])
            .current_dir(dep_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| FreightError::CompilerNotFound(format!("`patch` not found: {e}")))?;

        if let Some(stdin) = child.stdin.take() {
            use std::io::Write;
            let mut stdin = stdin;
            stdin.write_all(&patch_data)?;
        }

        let out = child.wait_with_output()?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(FreightError::CompileFailed(
                rel.clone(),
                format!("patch failed: {stderr}"),
            ));
        }
    }
    Ok(())
}
