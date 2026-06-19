//! HTTP archive download and SHA-256 verification via libcurl.
use std::io::Write;
use std::path::{Path, PathBuf};

use curl::easy::Easy;
use sha2::{Digest, Sha256};

use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};

/// Download a source archive to `.deps/{name}/`, verify SHA-256, and extract.
///
/// If `.deps/{name}/.freight-fetched` already exists the download is skipped.
/// Returns the extracted source dir.
pub fn fetch_url_dep(
    name: &str,
    url: &str,
    expected_sha256: Option<&str>,
    project_dir: &Path,
    progress: &Progress,
) -> Result<PathBuf, FreightError> {
    let deps_dir = project_dir.join(".pkgs").join(name);
    let sentinel = deps_dir.join(".freight-fetched");

    if sentinel.exists() {
        return Ok(deps_dir);
    }

    progress(BuildEvent::FetchingDep {
        name: name.to_string(),
        source: url.to_string(),
    });

    std::fs::create_dir_all(project_dir.join(".pkgs"))?;

    let ext = archive_ext(url);
    let archive_path = project_dir.join(".pkgs").join(format!("{name}.{ext}"));

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
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn archive_ext(url: &str) -> &'static str {
    if url.ends_with(".tar.gz") || url.ends_with(".tgz") {
        return "tar.gz";
    }
    if url.ends_with(".tar.bz2") {
        return "tar.bz2";
    }
    if url.ends_with(".tar.xz") {
        return "tar.xz";
    }
    if url.ends_with(".zip") {
        return "zip";
    }
    "tar.gz"
}

fn extract_archive(archive: &Path, dest: &Path, ext: &str) -> Result<(), FreightError> {
    let archive_s = archive.to_string_lossy().into_owned();
    let dest_s = dest.to_string_lossy().into_owned();

    if ext == "zip" {
        // Extracted in-process (no external `unzip`), with the same
        // strip-first-component behaviour as the `tar --strip-components=1`
        // path below — so zip and tarball deps share an identical layout.
        return extract_zip(archive, dest);
    }

    let ok = std::process::Command::new("tar")
        .args(["-xf", &archive_s, "-C", &dest_s, "--strip-components=1"])
        .status()
        .map_err(|e| FreightError::CompilerNotFound(format!("tar not found: {e}")))?
        .success();

    if !ok {
        return Err(FreightError::ManifestParse(format!(
            "extraction failed for '{}'",
            archive.display()
        )));
    }
    Ok(())
}

/// Extract a `.zip` archive into `dest`, stripping the first path component of
/// every entry (matching `tar --strip-components=1`). Done in-process via the
/// `zip` crate so no external `unzip` binary is required.
fn extract_zip(archive: &Path, dest: &Path) -> Result<(), FreightError> {
    let file = std::fs::File::open(archive).map_err(|e| {
        FreightError::ManifestParse(format!("opening '{}': {e}", archive.display()))
    })?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| FreightError::ManifestParse(format!("reading zip archive: {e}")))?;

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| FreightError::ManifestParse(format!("zip entry {i}: {e}")))?;
        // Use the sanitised path (rejects absolute paths and `..` traversal).
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        // Strip the leading component; skip the top-level dir entry itself.
        let mut comps = name.components();
        comps.next();
        let stripped: PathBuf = comps.as_path().to_path_buf();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(&stripped);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extract_zip_strips_top_component() {
        // Regression: zip deps used to need an external `unzip` AND skipped the
        // strip-components-1 that tarballs get, yielding a different layout.
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("a.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            w.add_directory("pkg-1.0/include/", opts).unwrap();
            w.start_file("pkg-1.0/include/foo.h", opts).unwrap();
            w.write_all(b"#define X 1\n").unwrap();
            w.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_zip(&zip_path, &dest).unwrap();
        assert!(
            dest.join("include/foo.h").exists(),
            "first path component should be stripped (got {:?})",
            std::fs::read_dir(&dest)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .collect::<Vec<_>>()
        );
        assert!(
            !dest.join("pkg-1.0").exists(),
            "top-level dir must be stripped"
        );
    }
}
