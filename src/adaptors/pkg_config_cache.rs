//! Persistent pkg-config result cache.
//!
//! Results are keyed by query string and stored in
//! `<project_dir>/target/.pkg-config-cache.msgpack`.  An entry is always
//! valid for the lifetime of a single build — stale entries are harmless
//! because `freight clean` wipes the whole `target/` directory.

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
    include_dirs: Vec<PathBuf>,
    link_flags: Vec<String>,
    version: String,
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
            return Ok((
                PkgConfigResult {
                    include_dirs: e.include_dirs.clone(),
                    link_flags: e.link_flags.clone(),
                },
                e.version.clone(),
            ));
        }
        let result = pkg_config_query(query)?;
        let version = pkg_config_version(query);
        self.entries.insert(
            query.to_string(),
            CacheEntry {
                include_dirs: result.include_dirs.clone(),
                link_flags: result.link_flags.clone(),
                version: version.clone(),
            },
        );
        Ok((result, version))
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
            return Ok((
                PkgConfigResult {
                    include_dirs: e.include_dirs.clone(),
                    link_flags: e.link_flags.clone(),
                },
                e.version.clone(),
            ));
        }
        let result = pkg_config_query_with_path(query, extra_paths)?;
        let version = pkg_config_version(query);
        self.entries.insert(
            cache_key,
            CacheEntry {
                include_dirs: result.include_dirs.clone(),
                link_flags: result.link_flags.clone(),
                version: version.clone(),
            },
        );
        Ok((result, version))
    }
}

fn cache_path(project_dir: &Path) -> PathBuf {
    project_dir.join("target").join(".pkg-config-cache.msgpack")
}
