//! PackageRepo implementation backed by vcpkg.
//!
//! Uses `vcpkg search` to look up and search packages. If vcpkg is not on
//! PATH, all lookups return `Ok(None)` / `Ok(vec![])`.

use std::process::Command;

use crate::error::FreightError;
use super::{PackageInfo, PackageVersion, PackageRepo};

pub struct VcpkgRepo;

impl PackageRepo for VcpkgRepo {
    fn repo_key(&self) -> &str {
        "vcpkg"
    }

    fn lookup(&self, name: &str) -> Result<Option<PackageInfo>, FreightError> {
        let output = match run_vcpkg_search(name) {
            Some(o) => o,
            None => return Ok(None),
        };
        for line in output.lines() {
            if let Some(info) = parse_vcpkg_line(line) {
                if info.name.eq_ignore_ascii_case(name) {
                    return Ok(Some(info));
                }
            }
        }
        Ok(None)
    }

    fn search(&self, query: &str) -> Result<Vec<PackageInfo>, FreightError> {
        let output = match run_vcpkg_search(query) {
            Some(o) => o,
            None => return Ok(vec![]),
        };
        let results = output.lines()
            .filter_map(parse_vcpkg_line)
            .collect();
        Ok(results)
    }
}

/// Run `vcpkg search <query>` and return the combined stdout+stderr, or
/// `None` if vcpkg is not found on PATH.
fn run_vcpkg_search(query: &str) -> Option<String> {
    let vcpkg = std::env::var("VCPKG").unwrap_or_else(|_| "vcpkg".into());
    let output = Command::new(&vcpkg)
        .arg("search")
        .arg(query)
        .output()
        .ok()?;

    // vcpkg sometimes writes results to stderr, sometimes stdout.
    let mut combined = String::new();
    if let Ok(s) = std::str::from_utf8(&output.stdout) {
        combined.push_str(s);
    }
    if let Ok(s) = std::str::from_utf8(&output.stderr) {
        combined.push_str(s);
    }
    Some(combined)
}

/// Parse a single vcpkg search output line.
///
/// Expected format: `<name>  <version>  <description…>`
/// Lines that don't match (headers, blank lines) return `None`.
fn parse_vcpkg_line(line: &str) -> Option<PackageInfo> {
    let mut tokens = line.split_whitespace();
    let name = tokens.next()?;
    // Skip header-like lines
    if name.starts_with('-') || name.eq_ignore_ascii_case("name") {
        return None;
    }
    let version = tokens.next()?.to_string();
    let description: String = tokens.collect::<Vec<_>>().join(" ");
    let description = if description.is_empty() { None } else { Some(description) };

    Some(PackageInfo {
        name: name.to_string(),
        description,
        latest: version.clone(),
        versions: vec![PackageVersion {
            version,
            checksum: None,
            download_url: None,
        }],
    })
}
