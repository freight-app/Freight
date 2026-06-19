//! CPU-feature (ISA extension) table.
//!
//! Each entry maps a feature name (used in `[arch.<arch>] features = [...]`) to
//! the compiler flag that enables it and the intrinsic headers it unlocks. This
//! is the arch counterpart to [`super::system_libs`]: `[os.*] features` link
//! system libraries (`-l…`), while `[arch.*] features` enable SIMD / ISA
//! extensions at compile time (`-mavx2`, `-march=armv8-a+sve`, …).
//!
//! The data is bundled (`cpu-features.toml`, embedded at compile time) and
//! user-extensible via `$FREIGHT_HOME/toolchains/cpu-features/*.toml`.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::toolchain::cache::freight_home;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CpuFeature {
    /// Feature name (matches the entry in `[arch.*] features`).
    pub name: String,
    /// Compiler flag that enables it, or `None` when the feature is baseline for
    /// the arch (e.g. NEON on AArch64) — the headers are still recognized.
    pub flag: Option<String>,
    /// `|`-separated arch names this feature belongs to (for misuse validation).
    pub arch: Option<String>,
    /// Intrinsic headers this feature unlocks (include-hygiene attribution).
    pub headers: Vec<String>,
}

// ── Data file format ────────────────────────────────────────────────────────────

const BUNDLED_CPU_FEATURES: &str = include_str!("cpu-features.toml");

#[derive(Debug, Clone, Deserialize)]
struct RawCpuFeature {
    #[serde(default)]
    flag: Option<String>,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default)]
    headers: Vec<String>,
}

fn parse_doc(src: &str) -> BTreeMap<String, RawCpuFeature> {
    toml::from_str(src).unwrap_or_default()
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load all CPU-feature entries: the bundled table plus any user `.toml` files
/// under `$FREIGHT_HOME/toolchains/cpu-features/` (a user entry with the same
/// name replaces the built-in).
pub fn load_cpu_features() -> Vec<CpuFeature> {
    let mut table = parse_doc(BUNDLED_CPU_FEATURES);

    if let Some(dir) = freight_home().map(|h| h.join("toolchains").join("cpu-features")) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut files: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect();
            files.sort();
            for path in files {
                if let Ok(src) = std::fs::read_to_string(&path) {
                    for (name, feat) in parse_doc(&src) {
                        table.insert(name, feat);
                    }
                }
            }
        }
    }

    table
        .into_iter()
        .map(|(name, f)| CpuFeature {
            name,
            flag: f.flag,
            arch: f.arch,
            headers: f.headers,
        })
        .collect()
}

/// Find a CPU feature by name (case-insensitive).
pub fn find_cpu_feature<'a>(name: &str, table: &'a [CpuFeature]) -> Option<&'a CpuFeature> {
    table.iter().find(|f| f.name.eq_ignore_ascii_case(name))
}

/// Resolve CPU-feature names to compiler flags. A known feature contributes its
/// table `flag` (baseline features with no flag contribute nothing); an unknown
/// name falls back to `-m<name>` so common x86 toggles work even when unlisted.
pub fn resolve_cpu_feature_flags(names: &[String]) -> Vec<String> {
    let table = load_cpu_features();
    let mut flags = Vec::new();
    for n in names {
        match find_cpu_feature(n, &table) {
            Some(f) => {
                if let Some(flag) = &f.flag {
                    flags.push(flag.clone());
                }
            }
            None => flags.push(format!("-m{n}")),
        }
    }
    flags
}

/// True if `arch` (a build arch name) is allowed by a feature's `arch` field.
/// A feature with no `arch` field is allowed everywhere.
pub fn feature_allows_arch(feature: &CpuFeature, arch: &str) -> bool {
    match &feature.arch {
        None => true,
        Some(expr) => expr.split('|').any(|a| a.trim().eq_ignore_ascii_case(arch)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_cpu_features_parse() {
        let table = parse_doc(BUNDLED_CPU_FEATURES);
        assert!(!table.is_empty());
        let avx2 = table.get("avx2").expect("avx2 present");
        assert_eq!(avx2.flag.as_deref(), Some("-mavx2"));
        assert!(avx2.headers.iter().any(|h| h == "immintrin.h"));
        let sve = table.get("sve").expect("sve present");
        assert_eq!(sve.flag.as_deref(), Some("-march=armv8-a+sve"));
        // A dotted name must be a quoted table key, not nested tables.
        let sse = table.get("sse4.2").expect("sse4.2 present (quoted key)");
        assert_eq!(sse.flag.as_deref(), Some("-msse4.2"));
    }

    #[test]
    fn known_feature_uses_table_flag() {
        let flags = resolve_cpu_feature_flags(&["avx2".to_string(), "sve".to_string()]);
        assert!(flags.contains(&"-mavx2".to_string()));
        assert!(flags.contains(&"-march=armv8-a+sve".to_string()));
    }

    #[test]
    fn unknown_feature_falls_back_to_dash_m() {
        let flags = resolve_cpu_feature_flags(&["sse2".to_string()]);
        assert_eq!(flags, vec!["-msse2".to_string()]);
    }

    #[test]
    fn arch_gating() {
        let table = load_cpu_features();
        let avx2 = find_cpu_feature("avx2", &table).unwrap();
        assert!(feature_allows_arch(avx2, "x86_64"));
        assert!(!feature_allows_arch(avx2, "aarch64"));
    }
}
