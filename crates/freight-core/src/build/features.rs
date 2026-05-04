//! Feature resolution for freight projects.
//!
//! Features map directly to preprocessor defines: feature `tls` → `-DTLS`,
//! feature `with_json` → `-DWITH_JSON`. The `default` key in `[features]` is
//! a special list of features enabled when no explicit selection is made; it
//! does not produce a define of its own.
//!
//! `dep:name` entries inside a feature's value list activate an optional dep
//! instead of producing a define — matching Cargo's syntax.

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::error::FreightError;

// ── Public types ──────────────────────────────────────────────────────────────

/// Output of feature resolution.
pub struct FeatureResolution {
    /// Active feature names — each produces a `-D<NAME>` define.
    pub active: BTreeSet<String>,
    /// Names of optional deps activated via `dep:name` entries in feature lists.
    pub activated_deps: BTreeSet<String>,
}

// ── Resolution ────────────────────────────────────────────────────────────────

/// Compute the set of active features and activated optional deps for a project.
///
/// - If `use_defaults` is true, features listed under `default` are included.
/// - `requested` adds features on top (e.g. from a parent dep's `features = [...]`
///   or from the active profile's `features` list).
/// - Feature lists are expanded transitively.
/// - `dep:name` entries in a feature list add `name` to `activated_deps` rather
///   than producing a define or being treated as a feature name.
/// - Unknown feature names → `Err`.
pub fn resolve_features(
    all_features: &HashMap<String, Vec<String>>,
    requested: &[String],
    use_defaults: bool,
) -> Result<FeatureResolution, FreightError> {
    if all_features.is_empty() && requested.is_empty() {
        return Ok(FeatureResolution {
            active: BTreeSet::new(),
            activated_deps: BTreeSet::new(),
        });
    }

    let defaults = all_features.get("default").map(|v| v.as_slice()).unwrap_or(&[]);

    let mut active: BTreeSet<String> = BTreeSet::new();
    let mut activated_deps: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();

    if use_defaults {
        for f in defaults { queue.push_back(f); }
    }
    for f in requested { queue.push_back(f.as_str()); }

    while let Some(feat) = queue.pop_front() {
        if feat == "default" { continue; }

        // dep:name → activate the optional dep, don't produce a define.
        if let Some(dep_name) = feat.strip_prefix("dep:") {
            activated_deps.insert(dep_name.to_string());
            continue;
        }

        if active.contains(feat) { continue; }
        if !all_features.contains_key(feat) {
            return Err(FreightError::ManifestParse(format!("unknown feature '{feat}'")));
        }
        active.insert(feat.to_string());
        if let Some(implied) = all_features.get(feat) {
            for f in implied { queue.push_back(f.as_str()); }
        }
    }

    Ok(FeatureResolution { active, activated_deps })
}

/// Convert active feature names to define strings (WITHOUT `-D` prefix).
/// `"tls"` → `"TLS"`, `"with_json"` → `"WITH_JSON"`.
/// The `-D` prefix is added by `assemble_flags` via the compiler template.
/// Hyphens are normalised to underscores before uppercasing.
pub fn to_defines(features: &BTreeSet<String>) -> Vec<String> {
    features.iter()
        .map(|f| f.replace('-', "_").to_uppercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs.iter().map(|(k, vs)| {
            (k.to_string(), vs.iter().map(|v| v.to_string()).collect())
        }).collect()
    }

    #[test]
    fn empty_features() {
        let r = resolve_features(&HashMap::new(), &[], true).unwrap();
        assert!(r.active.is_empty());
        assert!(r.activated_deps.is_empty());
    }

    #[test]
    fn defaults_activated() {
        let f = map(&[("default", &["logging"]), ("logging", &[]), ("tls", &[])]);
        let r = resolve_features(&f, &[], true).unwrap();
        assert!(r.active.contains("logging"));
        assert!(!r.active.contains("tls"));
    }

    #[test]
    fn transitive_expansion() {
        let f = map(&[("default", &["full"]), ("full", &["tls", "json"]), ("tls", &[]), ("json", &[])]);
        let r = resolve_features(&f, &[], true).unwrap();
        assert_eq!(r.active, ["full", "json", "tls"].iter().map(|s| s.to_string()).collect());
    }

    #[test]
    fn default_not_in_active_set() {
        let f = map(&[("default", &["logging"]), ("logging", &[])]);
        let r = resolve_features(&f, &[], true).unwrap();
        assert!(!r.active.contains("default"));
    }

    #[test]
    fn no_defaults_with_flag_false() {
        let f = map(&[("default", &["logging"]), ("logging", &[]), ("tls", &[])]);
        let r = resolve_features(&f, &["tls".to_string()], false).unwrap();
        assert!(!r.active.contains("logging"));
        assert!(r.active.contains("tls"));
    }

    #[test]
    fn unknown_feature_errors() {
        let f = map(&[("default", &["missing"]), ("logging", &[])]);
        assert!(resolve_features(&f, &[], true).is_err());
    }

    #[test]
    fn to_defines_uppercases() {
        let features: BTreeSet<String> = ["tls", "with-json"].iter().map(|s| s.to_string()).collect();
        let mut defines = to_defines(&features);
        defines.sort();
        assert_eq!(defines, vec!["TLS", "WITH_JSON"]);
    }

    #[test]
    fn dep_prefix_activates_optional_dep() {
        let f = map(&[
            ("default", &["openblas"]),
            ("openblas", &["dep:openblas"]),
            ("mkl",      &["dep:mkl"]),
        ]);
        let r = resolve_features(&f, &[], true).unwrap();
        // "openblas" feature is active (produces a define)
        assert!(r.active.contains("openblas"));
        // the optional dep "openblas" is activated
        assert!(r.activated_deps.contains("openblas"));
        // "mkl" feature not requested → its dep not activated
        assert!(!r.activated_deps.contains("mkl"));
    }

    #[test]
    fn dep_prefix_not_treated_as_define() {
        let f = map(&[
            ("logging", &["dep:spdlog"]),
        ]);
        let r = resolve_features(&f, &["logging".to_string()], false).unwrap();
        // "logging" is a real feature → produces a define
        assert!(r.active.contains("logging"));
        // "dep:spdlog" is NOT in active (no define for it)
        assert!(!r.active.contains("dep:spdlog"));
        assert!(!r.active.contains("spdlog"));
        // but the dep is activated
        assert!(r.activated_deps.contains("spdlog"));
    }

    #[test]
    fn profile_features_activate_deps() {
        // Simulate what build_project_at does: merge profile features into requested.
        let f = map(&[
            ("default",  &[]),
            ("openblas", &["dep:openblas"]),
            ("mkl",      &["dep:mkl"]),
        ]);
        let profile_features = vec!["mkl".to_string()];
        let r = resolve_features(&f, &profile_features, true).unwrap();
        assert!(r.active.contains("mkl"));
        assert!(r.activated_deps.contains("mkl"));
        assert!(!r.activated_deps.contains("openblas"));
    }
}
