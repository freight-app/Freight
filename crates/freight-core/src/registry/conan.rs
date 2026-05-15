//! PackageRepo implementation backed by Conan Center.
//!
//! Uses `conan search <name> -r=conancenter --raw` to look up packages.
//! If conan is not available, all lookups return `Ok(None)` / `Ok(vec![])`.

use std::process::Command;

use crate::error::FreightError;
use crate::meta::conan::is_conan_available;
use super::{PackageInfo, PackageVersion, PackageRepo};

pub struct ConanRepo;

impl PackageRepo for ConanRepo {
    fn repo_key(&self) -> &str {
        "conan"
    }

    fn lookup(&self, name: &str) -> Result<Option<PackageInfo>, FreightError> {
        if !is_conan_available() {
            return Ok(None);
        }
        let output = run_conan_search(name)?;
        let pattern = format!("{}/", name.to_ascii_lowercase());
        let mut latest: Option<String> = None;
        let mut versions: Vec<PackageVersion> = Vec::new();

        for line in output.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with(&pattern) {
                if let Some(ver) = line.split('/').nth(1) {
                    let ver = ver.split_whitespace().next().unwrap_or(ver).to_string();
                    if latest.is_none() {
                        latest = Some(ver.clone());
                    }
                    versions.push(PackageVersion {
                        version: ver,
                        checksum: None,
                        download_url: None,
                    });
                }
            }
        }

        match latest {
            Some(ver) => Ok(Some(PackageInfo {
                name: name.to_string(),
                description: None,
                latest: ver,
                versions,
            })),
            None => Ok(None),
        }
    }

    fn search(&self, query: &str) -> Result<Vec<PackageInfo>, FreightError> {
        if !is_conan_available() {
            return Ok(vec![]);
        }
        let output = run_conan_search(query)?;

        // Group lines by package name (everything before the first '/').
        let mut map: std::collections::BTreeMap<String, Vec<PackageVersion>> =
            std::collections::BTreeMap::new();

        for line in output.lines() {
            if let Some(slash) = line.find('/') {
                let pkg_name = line[..slash].to_string();
                let ver_part = line[slash + 1..].split_whitespace().next().unwrap_or("").to_string();
                if !ver_part.is_empty() {
                    map.entry(pkg_name).or_default().push(PackageVersion {
                        version: ver_part,
                        checksum: None,
                        download_url: None,
                    });
                }
            }
        }

        let results = map
            .into_iter()
            .filter_map(|(name, versions)| {
                let latest = versions.first()?.version.clone();
                Some(PackageInfo {
                    name,
                    description: None,
                    latest,
                    versions,
                })
            })
            .collect();
        Ok(results)
    }
}

/// Run `conan search <query> -r=conancenter --raw` and return stdout+stderr.
fn run_conan_search(query: &str) -> Result<String, FreightError> {
    let output = Command::new("conan")
        .args(["search", query, "-r=conancenter", "--raw"])
        .output()
        .map_err(|e| FreightError::RegistryError(format!("conan search failed: {e}")))?;

    let mut combined = String::new();
    if let Ok(s) = std::str::from_utf8(&output.stdout) {
        combined.push_str(s);
    }
    if let Ok(s) = std::str::from_utf8(&output.stderr) {
        combined.push_str(s);
    }
    Ok(combined)
}
