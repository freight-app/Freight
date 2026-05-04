//! HTTP archive download, SHA-256 verification, and pkg-config integration.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::error::FreightError;

// ── Public types ──────────────────────────────────────────────────────────────

pub struct PkgConfigResult {
    pub include_dirs: Vec<PathBuf>,
    pub link_flags: Vec<String>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Download a source archive from any URL that `curl` supports (`https://`,
/// `http://`, `ftp://`, etc.) to `.deps/{name}/`, verify SHA-256, and extract.
///
/// If `.deps/{name}/.freight-fetched` already exists the download is skipped
/// (use `freight update <name>` to re-fetch). Returns the extracted source dir.
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

    // Download with curl: -L follows redirects, --fail treats HTTP errors as
    // failures, --silent suppresses the progress meter.
    let status = Command::new("curl")
        .args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(&archive_path)
        .arg(url)
        .status()
        .map_err(|e| FreightError::CompilerNotFound(format!("curl not found: {e}")))?;

    if !status.success() {
        return Err(FreightError::ManifestParse(format!(
            "failed to download '{url}' for dep '{name}'"
        )));
    }

    // Verify SHA-256 when provided.
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

    // Mark as successfully fetched so subsequent builds skip the download.
    std::fs::write(&sentinel, url)?;

    Ok(deps_dir)
}

/// Run `pkg-config` for the given query and return the compiler and linker flags.
pub fn pkg_config_query(query: &str) -> Result<PkgConfigResult, FreightError> {
    let cflags = run_pkg_config(query, "--cflags")?;
    let libs   = run_pkg_config(query, "--libs")?;

    let include_dirs = cflags.split_ascii_whitespace()
        .filter_map(|f| f.strip_prefix("-I"))
        .map(PathBuf::from)
        .collect();

    let link_flags = libs.split_ascii_whitespace()
        .map(str::to_owned)
        .collect();

    Ok(PkgConfigResult { include_dirs, link_flags })
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn run_pkg_config(query: &str, flag: &str) -> Result<String, FreightError> {
    let parts: Vec<&str> = query.split_whitespace().collect();
    let out = Command::new("pkg-config")
        .arg(flag)
        .args(&parts)
        .output()
        .map_err(|e| FreightError::CompilerNotFound(format!("pkg-config not found: {e}")))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(FreightError::ManifestParse(format!(
            "pkg-config failed for '{query}': {stderr}"
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn sha256_of_file(path: &Path) -> Result<String, FreightError> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = hasher.finalize();
    Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
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
        Command::new("unzip")
            .args(["-q", &archive_s, "-d", &dest_s])
            .status()
            .map_err(|e| FreightError::CompilerNotFound(format!("unzip not found: {e}")))?
            .success()
    } else {
        // tar -xf auto-detects compression; --strip-components=1 removes
        // the single top-level directory that release archives usually contain.
        Command::new("tar")
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
