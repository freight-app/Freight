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
    /// CPU extension names supported by this compiler (e.g. `"avx2"`, `"sse4.2"`).
    /// Queried once via `-Q --help=target` and cached alongside the version.
    #[serde(default)]
    pub(crate) cpu_extensions: Vec<String>,
}

impl ToolchainCache {
    /// Load the cache from disk. Returns an empty cache on any I/O or parse error.
    pub fn load() -> Self {
        let Some(path) = cache_path() else {
            return Self::default();
        };
        let Ok(data) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
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
        let Some(mtime) = mtime_secs(binary_path) else {
            return;
        };
        let key = binary_path.to_string_lossy().into_owned();
        self.entries.insert(
            key,
            CacheEntry {
                version: version.to_string(),
                mtime_secs: mtime,
                cpu_extensions: vec![],
            },
        );
    }

    /// Return the cached extension list for `binary_path` if the entry is fresh.
    pub fn get_extensions(&self, binary_path: &Path) -> Option<&[String]> {
        let key = binary_path.to_string_lossy();
        let entry = self.entries.get(key.as_ref())?;
        let current_mtime = mtime_secs(binary_path)?;
        if entry.mtime_secs == current_mtime {
            Some(&entry.cpu_extensions)
        } else {
            None
        }
    }

    /// Record the extension list for `binary_path`. No-ops if mtime can't be read.
    pub fn set_extensions(&mut self, binary_path: &Path, extensions: Vec<String>) {
        let key = binary_path.to_string_lossy().into_owned();
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.cpu_extensions = extensions;
        }
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

/// One registry entry under `[[registries]]` in a config file.
///
/// Registries are tried in the order they appear. The first one that returns
/// a result for a given package name wins. The built-in `freight.dev` registry
/// is appended after any configured entries if no entry named `"freight"` exists.
///
/// Tokens are **never** stored in config files — they live in the OS keychain.
/// Use `Credentials::save` / `Credentials::load` / `Credentials::delete`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// Identifier used in `repo = "…"` dep fields and `--repo` CLI flag.
    pub name: String,
    /// Base URL of the registry HTTP API.
    pub url: String,
    /// Bearer token populated at runtime from the OS keychain or env vars.
    /// Never serialized to disk.
    #[serde(skip)]
    pub token: Option<String>,
}

/// Persistent global config stored at `~/.freight/config.toml`.
///
/// Developer settings that apply across all projects on the machine.
/// Loaded from `~/.freight/config.toml`; overridden per-project by
/// `.freight/config.toml` in the project root (same format, local wins).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Default compiler backend. `None` = first detected compiler for each language.
    pub default_backend: Option<String>,
    /// Default debugger backend. `None` = first detected debugger.
    pub default_debugger: Option<String>,
    /// Cross-compilation target triple (e.g. `"aarch64-linux-gnu"`). Machine-local.
    pub target: Option<String>,
    /// Path to the cross-compilation sysroot. Machine-local absolute path.
    pub sysroot: Option<String>,
    /// Whether freight derives CPU tuning flags from the configured target/sysroot.
    #[serde(default, rename = "auto-cpu-tuning", alias = "auto_cpu_tuning")]
    pub auto_cpu_tuning: Option<bool>,
    /// Ordered list of package registries to search.
    /// Tried in declaration order; first hit wins.
    /// The public `freight.dev` registry is always appended last unless an entry
    /// named `"freight"` is already present.
    #[serde(default)]
    pub registries: Vec<RegistryConfig>,
    /// Developer debugger preferences, keyed by debugger name under `[debugger.<name>]`.
    #[serde(default)]
    pub debugger: DebuggerConfig,
    /// Command aliases under `[alias]` (e.g. `b = "build"`, `br = ["build", "--release"]`).
    /// An alias may not shadow a built-in subcommand. Mirrors Cargo's `[alias]`.
    #[serde(default)]
    pub alias: std::collections::HashMap<String, AliasValue>,
}

/// The value of an `[alias]` entry: a single string (split on whitespace) or an
/// explicit argument list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AliasValue {
    One(String),
    Many(Vec<String>),
}

impl AliasValue {
    /// Expand to the argument tokens this alias contributes.
    pub fn into_args(self) -> Vec<String> {
        match self {
            AliasValue::One(s) => s.split_whitespace().map(str::to_string).collect(),
            AliasValue::Many(v) => v,
        }
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;

    #[test]
    fn alias_string_splits_on_whitespace() {
        let v = AliasValue::One("build --release".to_string());
        assert_eq!(v.into_args(), vec!["build", "--release"]);
    }

    #[test]
    fn alias_array_is_verbatim() {
        let v = AliasValue::Many(vec!["build".to_string(), "--release".to_string()]);
        assert_eq!(v.into_args(), vec!["build", "--release"]);
    }

    #[test]
    fn local_alias_overrides_global() {
        let mut base = GlobalConfig::default();
        base.alias
            .insert("b".to_string(), AliasValue::One("build".to_string()));
        let mut local = GlobalConfig::default();
        local
            .alias
            .insert("b".to_string(), AliasValue::One("bench".to_string()));
        base.apply_local(local);
        assert_eq!(base.alias.get("b").cloned().unwrap().into_args(), vec!["bench"]);
    }
}

impl GlobalConfig {
    /// Load the effective global config by merging all config layers in order:
    ///
    /// 1. `/etc/freight/config.toml`  — system-wide defaults (lowest priority)
    /// 2. `~/.freight/config.toml`    — user overrides
    ///
    /// Each layer is applied with [`apply_local`] so absent fields fall through
    /// to the layer below. Returns defaults when no config files are found.
    ///
    /// The system config path can be overridden with `FREIGHT_SYSTEM_CONFIG`.
    pub fn load() -> Self {
        let mut config = Self::load_system();
        if let Some(path) = Self::path() {
            if let Ok(data) = std::fs::read_to_string(path) {
                if let Ok(user) = toml_edit::de::from_str::<Self>(&data) {
                    config.apply_local(user);
                }
            }
        }
        // Populate tokens from env vars or the OS keychain (never from disk).
        for reg in &mut config.registries {
            reg.token = Credentials::load(&reg.name);
        }
        config
    }

    /// Load the system-wide config from `/etc/freight/config.toml`
    /// (or `$FREIGHT_SYSTEM_CONFIG` if set).
    fn load_system() -> Self {
        let path = std::env::var_os("FREIGHT_SYSTEM_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/etc/freight/config.toml"));
        let Ok(data) = std::fs::read_to_string(path) else {
            return Self::default();
        };
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
        if local.default_backend.is_some() {
            self.default_backend = local.default_backend;
        }
        if local.default_debugger.is_some() {
            self.default_debugger = local.default_debugger;
        }
        if local.target.is_some() {
            self.target = local.target;
        }
        if local.sysroot.is_some() {
            self.sysroot = local.sysroot;
        }
        if local.auto_cpu_tuning.is_some() {
            self.auto_cpu_tuning = local.auto_cpu_tuning;
        }
        // Local registries take priority: prepend them, keeping base registries that
        // aren't shadowed by name.
        if !local.registries.is_empty() {
            let mut merged = local.registries;
            for base in self.registries.drain(..) {
                if !merged.iter().any(|r| r.name == base.name) {
                    merged.push(base);
                }
            }
            self.registries = merged;
        }
        for (name, local_inst) in local.debugger.debuggers {
            let inst = self.debugger.debuggers.entry(name).or_default();
            inst.args.extend(local_inst.args);
            inst.settings.extend(local_inst.settings);
        }
        // Local aliases override global ones of the same name.
        for (name, value) in local.alias {
            self.alias.insert(name, value);
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
            .map_err(|e| crate::error::FreightError::Io(std::io::Error::other(e.to_string())))?;
        std::fs::write(&path, toml)?;
        Ok(())
    }

    fn path() -> Option<PathBuf> {
        freight_home().map(|h| h.join("config.toml"))
    }
}

// ── Keychain-backed credentials ───────────────────────────────────────────────

/// Keychain-backed token store for registry authentication.
///
/// Tokens are stored in the OS credential store (macOS Keychain, GNOME Keyring /
/// KDE Wallet via the Secret Service, Windows Credential Manager). They are never
/// written to any plain-text file.
///
/// Lookup order for [`Credentials::load`]:
/// 1. `FREIGHT_TOKEN_<NAME_UPPERCASE>` — e.g. `FREIGHT_TOKEN_FREIGHT`
/// 2. `FREIGHT_TOKEN` — single-registry shorthand
/// 3. OS keychain entry for service `"freight"`, account = registry name
pub struct Credentials;

impl Credentials {
    const SERVICE: &'static str = "freight";

    /// Store `token` in the OS keychain for the named registry.
    pub fn save(registry_name: &str, token: &str) -> anyhow::Result<()> {
        keyring_core::Entry::new(Self::SERVICE, registry_name)?.set_password(token)?;
        Ok(())
    }

    /// Retrieve the token for `registry_name`, checking env vars first.
    /// Returns `None` when no credential is found (not an error).
    pub fn load(registry_name: &str) -> Option<String> {
        // 1. FREIGHT_TOKEN_<NAME>
        let env_key = format!(
            "FREIGHT_TOKEN_{}",
            registry_name.to_ascii_uppercase().replace('-', "_")
        );
        if let Ok(t) = std::env::var(&env_key) {
            if !t.is_empty() {
                return Some(t);
            }
        }
        // 2. FREIGHT_TOKEN
        if let Ok(t) = std::env::var("FREIGHT_TOKEN") {
            if !t.is_empty() {
                return Some(t);
            }
        }
        // 3. OS keychain
        keyring_core::Entry::new(Self::SERVICE, registry_name)
            .ok()
            .and_then(|e| e.get_password().ok())
            .filter(|t| !t.is_empty())
    }

    /// Remove the token for `registry_name` from the OS keychain.
    /// Returns `Ok(())` if the entry didn't exist.
    pub fn delete(registry_name: &str) -> anyhow::Result<()> {
        match keyring_core::Entry::new(Self::SERVICE, registry_name)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// The freight home directory: `$FREIGHT_HOME` or `~/.freight`.
pub fn freight_home() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("FREIGHT_HOME") {
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
        assert!(cache
            .get_version(Path::new("/usr/bin/nonexistent"))
            .is_none());
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
        std::env::set_var("FREIGHT_HOME", dir.path());

        let mut cache = ToolchainCache::default();
        cache.set_version(&bin, "15.0.0");
        cache.save();

        let reloaded = ToolchainCache::load();
        assert_eq!(reloaded.get_version(&bin), Some("15.0.0"));

        std::env::remove_var("FREIGHT_HOME");
    }

    #[test]
    fn config_deserializes_default_debugger() {
        let cfg: GlobalConfig = toml_edit::de::from_str(
            r#"
default_backend = "clang"
default_debugger = "lldb"
"#,
        )
        .unwrap();

        assert_eq!(cfg.default_backend.as_deref(), Some("clang"));
        assert_eq!(cfg.default_debugger.as_deref(), Some("lldb"));
    }

    #[test]
    fn local_config_overrides_default_debugger() {
        let mut base = GlobalConfig {
            default_debugger: Some("gdb".into()),
            ..Default::default()
        };
        let local = GlobalConfig {
            default_debugger: Some("lldb".into()),
            ..Default::default()
        };

        base.apply_local(local);
        assert_eq!(base.default_debugger.as_deref(), Some("lldb"));
    }
}
