//! Standard-library / libc provider packages.
//!
//! Maps a system header to the *package* that provides it (glibc, musl, bionic,
//! libSystem, libstdc++, libc++) rather than to the standard it belongs to — so a
//! cross build shows the target's library. Each provider declares the capabilities
//! it `provides` (`stdlib` / `posix` / `cxx`) and how it's detected (compiler
//! triple substrings for libc, resolved-path substrings for the C++ stdlib).
//!
//! Data-driven: bundled `std-providers.toml` plus user `.toml` files in
//! `$FREIGHT_HOME/toolchains/std-providers/`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::toolchain::cache::freight_home;

#[derive(Debug, Clone)]
pub struct StdProvider {
    pub name: String,
    pub provides: Vec<String>,
    /// Triple substrings (libc providers).
    pub triple: Vec<String>,
    /// Resolved-path substrings (C++ stdlib providers).
    pub path: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProvider {
    #[serde(default)]
    provides: Vec<String>,
    #[serde(default)]
    triple: Vec<String>,
    #[serde(default)]
    path: Vec<String>,
}

const BUNDLED: &str = include_str!("std-providers.toml");

fn parse_doc(src: &str) -> BTreeMap<String, RawProvider> {
    toml::from_str(src).unwrap_or_default()
}

/// Load all provider definitions (bundled + user overrides). A user entry with the
/// same name replaces the built-in.
pub fn load_std_providers() -> Vec<StdProvider> {
    let mut table = parse_doc(BUNDLED);
    if let Some(dir) = freight_home().map(|h| h.join("toolchains").join("std-providers")) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut files: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect();
            files.sort();
            for path in files {
                if let Ok(src) = std::fs::read_to_string(&path) {
                    for (name, prov) in parse_doc(&src) {
                        table.insert(name, prov);
                    }
                }
            }
        }
    }
    table
        .into_iter()
        .map(|(name, p)| StdProvider {
            name,
            provides: p.provides,
            triple: p.triple,
            path: p.path,
        })
        .collect()
}

/// Resolve the active provider package for `capability` given the effective
/// compiler triple (target, else host) and an optional resolved header path.
/// `cxx` is matched by path (most-specific marker wins); libc capabilities by
/// triple substring. `None` when it can't be determined confidently.
pub fn resolve_provider(
    capability: &str,
    providers: &[StdProvider],
    triple: Option<&str>,
    resolved_path: Option<&Path>,
) -> Option<String> {
    let cands = providers
        .iter()
        .filter(|p| p.provides.iter().any(|c| c == capability));
    if capability == "cxx" {
        let path = resolved_path?;
        let s = path.to_string_lossy();
        let mut best: Option<(&StdProvider, usize)> = None;
        for prov in cands {
            for m in &prov.path {
                if s.contains(m.as_str()) && best.is_none_or(|(_, len)| m.len() > len) {
                    best = Some((prov, m.len()));
                }
            }
        }
        best.map(|(p, _)| p.name.clone())
    } else {
        let t = triple?.to_ascii_lowercase();
        cands
            .filter(|p| p.triple.iter().any(|m| t.contains(m.as_str())))
            .map(|p| p.name.clone())
            .next()
    }
}

/// The host compiler's target triple via `<cc> -dumpmachine` (cached), used to
/// pick the native libc provider (e.g. `x86_64-alpine-linux-musl` → musl). `None`
/// if no compiler answers.
pub fn host_triple() -> Option<String> {
    static T: OnceLock<Option<String>> = OnceLock::new();
    T.get_or_init(|| {
        for cc in ["cc", "gcc", "clang", "c++", "clang++"] {
            if let Ok(o) = Command::new(cc).arg("-dumpmachine").output() {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_toml_parses_ok() {
        let r: Result<BTreeMap<String, RawProvider>, _> = toml::from_str(BUNDLED);
        r.expect("bundled std-providers.toml parses");
    }

    #[test]
    fn bundled_providers_parse() {
        let p = load_std_providers();
        assert!(p.iter().any(|x| x.name == "glibc"));
        assert!(p.iter().any(|x| x.name == "libstdc++"));
        let glibc = p.iter().find(|x| x.name == "glibc").unwrap();
        assert!(glibc.provides.contains(&"stdlib".to_string()));
        assert!(glibc.provides.contains(&"posix".to_string()));
    }

    #[test]
    fn resolve_libc_by_triple() {
        let p = load_std_providers();
        assert_eq!(
            resolve_provider("stdlib", &p, Some("aarch64-linux-musl"), None).as_deref(),
            Some("musl")
        );
        assert_eq!(
            resolve_provider("posix", &p, Some("x86_64-linux-gnu"), None).as_deref(),
            Some("glibc")
        );
        assert_eq!(
            resolve_provider("posix", &p, Some("aarch64-linux-android"), None).as_deref(),
            Some("bionic")
        );
        assert_eq!(resolve_provider("stdlib", &p, None, None), None);
    }

    #[test]
    fn resolve_cxx_by_path_most_specific() {
        let p = load_std_providers();
        assert_eq!(
            resolve_provider("cxx", &p, None, Some(Path::new("/usr/include/c++/13/vector"))).as_deref(),
            Some("libstdc++")
        );
        // `/c++/v1/` matches both libc++ (v1) and libstdc++ (/c++/); most specific wins.
        assert_eq!(
            resolve_provider("cxx", &p, None, Some(Path::new("/usr/lib/c++/v1/vector"))).as_deref(),
            Some("libc++")
        );
    }
}
