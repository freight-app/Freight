use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::manifest::types::DebuggerConfig;

/// Persistent cache of detected compiler versions, stored at `~/.freight/toolchain-cache.json`.
///
/// Each entry is keyed by the compiler's absolute path and validated against
/// the binary's mtime — a changed mtime invalidates the entry and triggers
/// a fresh `--version` query.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ToolchainCache {
    pub(crate) entries: HashMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CacheEntry {
    pub(crate) version: String,
    pub(crate) mtime_secs: u64,
}

impl ToolchainCache {
    /// Load the cache from disk. Returns an empty cache on any I/O or parse error.
    pub fn load() -> Self {
        let Some(path) = cache_path() else { return Self::default() };
        let Ok(data) = std::fs::read_to_string(&path) else { return Self::default() };
        serde_json::from_str(&data).unwrap_or_default()
    }

    /// Persist the cache to disk. Silently ignores write errors.
    pub fn save(&self) {
        let Some(path) = cache_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Return the cached version string for `binary_path` if the binary's mtime
    /// matches what was recorded. Returns `None` if missing or stale.
    pub fn get_version(&self, binary_path: &Path) -> Option<&str> {
        let key = binary_path.to_string_lossy();
        let entry = self.entries.get(key.as_ref())?;
        let current_mtime = mtime_secs(binary_path)?;
        if entry.mtime_secs == current_mtime {
            Some(&entry.version)
        } else {
            None
        }
    }

    /// Record a version string for `binary_path`. No-ops if mtime can't be read.
    pub fn set_version(&mut self, binary_path: &Path, version: &str) {
        let Some(mtime) = mtime_secs(binary_path) else { return };
        let key = binary_path.to_string_lossy().into_owned();
        self.entries.insert(key, CacheEntry { version: version.to_string(), mtime_secs: mtime });
    }

    /// Remove all entries whose binary no longer exists on disk.
    pub fn evict_missing(&mut self) {
        self.entries.retain(|path, _| Path::new(path).exists());
    }
}

fn cache_path() -> Option<PathBuf> {
    freight_home().map(|h| h.join("toolchain-cache.json"))
}

// ── Global config ─────────────────────────────────────────────────────────────

/// Persistent global config stored at `~/.freight/config.toml`.
///
/// Developer settings that apply across all projects on the machine.
/// Loaded from `~/.freight/config.toml`; overridden per-project by
/// `.freight/config.toml` in the project root (same format, local wins).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Default compiler backend. `None` = first detected compiler for each language.
    pub default_backend: Option<String>,
    /// Cross-compilation target triple (e.g. `"aarch64-linux-gnu"`). Machine-local.
    pub target: Option<String>,
    /// Path to the cross-compilation sysroot. Machine-local absolute path.
    pub sysroot: Option<String>,
    /// Developer debugger preferences, keyed by debugger name under `[debugger.<name>]`.
    #[serde(default)]
    pub debugger: DebuggerConfig,
}

impl GlobalConfig {
    /// Load the global config from `~/.freight/config.toml`.
    /// Returns defaults on any error.
    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        let Ok(data) = std::fs::read_to_string(&path) else { return Self::default() };
        toml_edit::de::from_str(&data).unwrap_or_default()
    }

    /// Load the project-local config from `<project_dir>/.freight/config.toml`.
    /// Returns `None` when the file doesn't exist.
    pub fn load_local(project_dir: &Path) -> Option<Self> {
        let path = project_dir.join(".freight").join("config.toml");
        let data = std::fs::read_to_string(path).ok()?;
        toml_edit::de::from_str(&data).ok()
    }

    /// Apply `local` on top of `self`. Local scalar fields override global ones
    /// when set; debugger args are extended, boolean settings are overridden.
    pub fn apply_local(&mut self, local: Self) {
        if local.default_backend.is_some() { self.default_backend = local.default_backend; }
        if local.target.is_some()          { self.target          = local.target; }
        if local.sysroot.is_some()         { self.sysroot         = local.sysroot; }
        for (name, local_inst) in local.debugger.debuggers {
            let inst = self.debugger.debuggers.entry(name).or_default();
            inst.args.extend(local_inst.args);
            inst.settings.extend(local_inst.settings);
        }
    }

    /// Persist the config to disk.
    pub fn save(&self) -> Result<(), crate::error::FreightError> {
        let path = Self::path().ok_or_else(|| {
            crate::error::FreightError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "cannot determine ~/.freight directory",
            ))
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml = toml_edit::ser::to_string_pretty(self)
            .map_err(|e| crate::error::FreightError::Io(std::io::Error::new(
                std::io::ErrorKind::Other, e.to_string(),
            )))?;
        std::fs::write(&path, toml)?;
        Ok(())
    }

    fn path() -> Option<PathBuf> {
        freight_home().map(|h| h.join("config.toml"))
    }
}

/// The freight home directory: `$CRANE_HOME` or `~/.freight`.
pub fn freight_home() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("CRANE_HOME") {
        let p = PathBuf::from(h);
        if p.parent().map(|pp| pp.exists()).unwrap_or(false) {
            return Some(p);
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".freight"))
}

fn mtime_secs(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let t = meta.modified().ok()?;
    let dur = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cache_hit_on_matching_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakecc");
        fs::write(&bin, "fake").unwrap();

        let mut cache = ToolchainCache::default();
        cache.set_version(&bin, "13.2.0");

        assert_eq!(cache.get_version(&bin), Some("13.2.0"));
    }

    #[test]
    fn cache_miss_after_binary_modified() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakecc");
        fs::write(&bin, "fake").unwrap();

        let mut cache = ToolchainCache::default();
        cache.set_version(&bin, "13.2.0");

        // Directly corrupt the stored mtime to simulate a stale entry
        // (avoids relying on filesystem timer resolution in tests).
        let key = bin.to_string_lossy().into_owned();
        cache.entries.get_mut(&key).unwrap().mtime_secs = 0;

        assert!(
            cache.get_version(&bin).is_none(),
            "stale entry should not be returned"
        );
    }

    #[test]
    fn cache_miss_for_unknown_binary() {
        let cache = ToolchainCache::default();
        assert!(cache.get_version(Path::new("/usr/bin/nonexistent")).is_none());
    }

    #[test]
    fn evict_missing_removes_gone_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakecc");
        fs::write(&bin, "fake").unwrap();

        let mut cache = ToolchainCache::default();
        cache.set_version(&bin, "13.2.0");
        assert_eq!(cache.entries.len(), 1);

        fs::remove_file(&bin).unwrap();
        cache.evict_missing();

        assert!(cache.entries.is_empty());
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakecc");
        fs::write(&bin, "fake").unwrap();

        // Use temp dir as cache home
        std::env::set_var("CRANE_HOME", dir.path());

        let mut cache = ToolchainCache::default();
        cache.set_version(&bin, "15.0.0");
        cache.save();

        let reloaded = ToolchainCache::load();
        assert_eq!(reloaded.get_version(&bin), Some("15.0.0"));

        std::env::remove_var("CRANE_HOME");
    }
}
