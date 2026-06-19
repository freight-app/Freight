//! Persistent pkg-config result cache.
//!
//! Results are keyed by query string and stored in
//! `<project_dir>/target/.pkg-config-cache.msgpack`.  Both hits *and* misses are
//! cached: a successful probe stores its flags, and a "not found" stores a
//! negative entry so a dependency that falls through to a stub/registry doesn't
//! re-run `pkg-config` on every build.  Stale entries are harmless because
//! `freight clean` wipes the whole `target/` directory (so e.g. installing a
//! previously-missing library is picked up after a clean).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::pkg_config::{
    pkg_config_query, pkg_config_query_with_path, pkg_config_version, PkgConfigResult,
};
use crate::error::FreightError;

// ── On-disk format ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
pub struct PkgConfigCache {
    entries: HashMap<String, CacheEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct CacheEntry {
    /// `false` for a cached "pkg-config doesn't provide this" miss. Defaults to
    /// `true` so caches written before negative caching still read as hits.
    #[serde(default = "default_true")]
    found: bool,
    include_dirs: Vec<PathBuf>,
    link_flags: Vec<String>,
    version: String,
}

fn default_true() -> bool {
    true
}

// ── Public API ────────────────────────────────────────────────────────────────

impl PkgConfigCache {
    /// Load from `<project_dir>/target/.pkg-config-cache.msgpack`.
    /// Returns an empty cache if the file is missing or unreadable.
    pub fn load(project_dir: &Path) -> Self {
        let path = cache_path(project_dir);
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        rmp_serde::from_slice(&bytes).unwrap_or_default()
    }

    /// Persist the cache.  Silently ignores write errors (cache is best-effort).
    pub fn save(&self, project_dir: &Path) {
        let path = cache_path(project_dir);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = rmp_serde::to_vec(self) {
            let _ = std::fs::write(path, bytes);
        }
    }

    /// Query with caching.  On a miss, runs `pkg-config` and caches the result.
    pub fn query(&mut self, query: &str) -> Result<(PkgConfigResult, String), FreightError> {
        if let Some(e) = self.entries.get(query) {
            return entry_to_result(query, e);
        }
        match pkg_config_query(query) {
            Ok(result) => {
                let version = pkg_config_version(query);
                self.entries
                    .insert(query.to_string(), CacheEntry::hit(&result, &version));
                Ok((result, version))
            }
            Err(e) => {
                self.entries.insert(query.to_string(), CacheEntry::miss());
                Err(e)
            }
        }
    }

    /// Like [`query`] but prepends `extra_paths` to `PKG_CONFIG_PATH`.
    pub fn query_with_path(
        &mut self,
        query: &str,
        extra_paths: &[PathBuf],
    ) -> Result<(PkgConfigResult, String), FreightError> {
        let cache_key = format!(
            "{query}|{}",
            extra_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(":")
        );
        if let Some(e) = self.entries.get(&cache_key) {
            return entry_to_result(query, e);
        }
        match pkg_config_query_with_path(query, extra_paths) {
            Ok(result) => {
                let version = pkg_config_version(query);
                self.entries
                    .insert(cache_key, CacheEntry::hit(&result, &version));
                Ok((result, version))
            }
            Err(e) => {
                self.entries.insert(cache_key, CacheEntry::miss());
                Err(e)
            }
        }
    }
}

impl CacheEntry {
    fn hit(result: &PkgConfigResult, version: &str) -> Self {
        Self {
            found: true,
            include_dirs: result.include_dirs.clone(),
            link_flags: result.link_flags.clone(),
            version: version.to_string(),
        }
    }
    fn miss() -> Self {
        Self {
            found: false,
            include_dirs: vec![],
            link_flags: vec![],
            version: String::new(),
        }
    }
}

/// Turn a cache entry into a query result: `Err` for a cached miss (so the caller
/// falls through to the next resolver without re-running pkg-config).
fn entry_to_result(query: &str, e: &CacheEntry) -> Result<(PkgConfigResult, String), FreightError> {
    if !e.found {
        return Err(FreightError::ManifestParse(format!(
            "pkg-config has no '{query}' (cached miss)"
        )));
    }
    Ok((
        PkgConfigResult {
            include_dirs: e.include_dirs.clone(),
            link_flags: e.link_flags.clone(),
        },
        e.version.clone(),
    ))
}

fn cache_path(project_dir: &Path) -> PathBuf {
    project_dir.join("target").join(".pkg-config-cache.msgpack")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miss_is_cached_negative() {
        // A name pkg-config can't provide → Err, recorded as a negative entry so
        // the next probe is served from cache instead of re-running pkg-config.
        let mut c = PkgConfigCache::default();
        let q = "freight-nonexistent-pkgconfig-xyz";
        assert!(c.query(q).is_err());
        let e = c.entries.get(q).expect("miss is cached");
        assert!(!e.found);
        // Served from the negative cache on the second call (still Err).
        assert!(c.query(q).is_err());
    }

    #[test]
    fn legacy_entry_without_found_reads_as_hit() {
        // Caches written before negative caching have no `found` field.
        let e: CacheEntry = serde_json::from_value(serde_json::json!({
            "include_dirs": ["/usr/include"],
            "link_flags": ["-lfoo"],
            "version": "1.0"
        }))
        .unwrap();
        assert!(e.found);
        assert!(entry_to_result("foo", &e).is_ok());
    }
}
