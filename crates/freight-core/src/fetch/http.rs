//! HTTP archive download and SHA-256 verification via libcurl.
use std::io::Write;
use std::path::{Path, PathBuf};

use curl::easy::Easy;
use sha2::{Digest, Sha256};

use crate::error::FreightError;

/// Download a source archive to `.deps/{name}/`, verify SHA-256, and extract.
///
/// If `.deps/{name}/.freight-fetched` already exists the download is skipped.
/// Returns the extracted source dir.
pub fn fetch_url_dep(
    name: &str,
    url: &str,
    expected_sha256: Option<&str>,
    project_dir: &Path,
) -> Result<PathBuf, FreightError> {
    let deps_dir = project_dir.join(".deps").join(name);
    let sentinel = deps_dir.join(".freight-fetched");

    if sentinel.exists() {
        return Ok(deps_dir);
    }

    use owo_colors::OwoColorize;
    println!("  {} {} from {}", "Fetching".dimmed(), name, url);

    std::fs::create_dir_all(project_dir.join(".deps"))?;

    let ext = archive_ext(url);
    let archive_path = project_dir.join(".deps").join(format!("{name}.{ext}"));

    download(url, &archive_path)?;

    if let Some(expected) = expected_sha256 {
        let actual = sha256_of_file(&archive_path)?;
        if actual != expected.to_lowercase() {
            let _ = std::fs::remove_file(&archive_path);
            return Err(FreightError::ManifestParse(format!(
                "SHA-256 mismatch for dep '{name}': expected {expected}, got {actual}"
            )));
        }
    }

    std::fs::create_dir_all(&deps_dir)?;
    extract_archive(&archive_path, &deps_dir, ext)?;
    let _ = std::fs::remove_file(&archive_path);

    std::fs::write(&sentinel, url)?;
    Ok(deps_dir)
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn download(url: &str, dest: &Path) -> Result<(), FreightError> {
    let file = std::fs::File::create(dest)?;
    let mut file = std::io::BufWriter::new(file);

    let mut easy = Easy::new();
    easy.url(url)
        .map_err(|e| FreightError::ManifestParse(format!("curl url error: {e}")))?;
    easy.follow_location(true)
        .map_err(|e| FreightError::ManifestParse(format!("curl option error: {e}")))?;
    easy.fail_on_error(true)
        .map_err(|e| FreightError::ManifestParse(format!("curl option error: {e}")))?;

    {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                file.write_all(data).ok();
                Ok(data.len())
            })
            .map_err(|e| FreightError::ManifestParse(format!("curl write setup: {e}")))?;
        transfer
            .perform()
            .map_err(|e| FreightError::ManifestParse(format!("download failed: {e}")))?;
    }

    Ok(())
}

fn sha256_of_file(path: &Path) -> Result<String, FreightError> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn archive_ext(url: &str) -> &'static str {
    if url.ends_with(".tar.gz") || url.ends_with(".tgz") { return "tar.gz"; }
    if url.ends_with(".tar.bz2")                         { return "tar.bz2"; }
    if url.ends_with(".tar.xz")                          { return "tar.xz"; }
    if url.ends_with(".zip")                             { return "zip"; }
    "tar.gz"
}

fn extract_archive(archive: &Path, dest: &Path, ext: &str) -> Result<(), FreightError> {
    let archive_s = archive.to_string_lossy().into_owned();
    let dest_s    = dest.to_string_lossy().into_owned();

    let ok = if ext == "zip" {
        std::process::Command::new("unzip")
            .args(["-q", &archive_s, "-d", &dest_s])
            .status()
            .map_err(|e| FreightError::CompilerNotFound(format!("unzip not found: {e}")))?
            .success()
    } else {
        std::process::Command::new("tar")
            .args(["-xf", &archive_s, "-C", &dest_s, "--strip-components=1"])
            .status()
            .map_err(|e| FreightError::CompilerNotFound(format!("tar not found: {e}")))?
            .success()
    };

    if !ok {
        return Err(FreightError::ManifestParse(format!(
            "extraction failed for '{}'", archive.display()
        )));
    }
    Ok(())
}
