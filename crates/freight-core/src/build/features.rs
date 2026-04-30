//! Feature resolution for freight projects.
//!
//! Features map directly to preprocessor defines: feature `tls` → `-DTLS`,
//! feature `with_json` → `-DWITH_JSON`. The `default` key in `[features]` is
//! a special list of features enabled when no explicit selection is made; it
//! does not produce a define of its own.

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::error::FreightError;

/// Compute the set of active features for a project.
///
/// - If `use_defaults` is true, features listed under `default` are included.
/// - `requested` adds features on top (e.g. from a parent dep's `features = [...]`).
/// - Feature lists are expanded transitively.
/// - Unknown feature names → `Err`.
pub fn resolve_features(
    all_features: &HashMap<String, Vec<String>>,
    requested: &[String],
    use_defaults: bool,
) -> Result<BTreeSet<String>, FreightError> {
    if all_features.is_empty() && requested.is_empty() {
        return Ok(BTreeSet::new());
    }

    let defaults = all_features.get("default").map(|v| v.as_slice()).unwrap_or(&[]);

    let mut active: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();

    if use_defaults {
        for f in defaults { queue.push_back(f); }
    }
    for f in requested { queue.push_back(f.as_str()); }

    while let Some(feat) = queue.pop_front() {
        if feat == "default" { continue; }
        if active.contains(feat) { continue; }
        if !all_features.contains_key(feat) {
            return Err(FreightError::ManifestParse(format!("unknown feature '{feat}'")));
        }
        active.insert(feat.to_string());
        if let Some(implied) = all_features.get(feat) {
            for f in implied { queue.push_back(f.as_str()); }
        }
    }

    Ok(active)
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
        let active = resolve_features(&HashMap::new(), &[], true).unwrap();
        assert!(active.is_empty());
    }

    #[test]
    fn defaults_activated() {
        let f = map(&[("default", &["logging"]), ("logging", &[]), ("tls", &[])]);
        let active = resolve_features(&f, &[], true).unwrap();
        assert!(active.contains("logging"));
        assert!(!active.contains("tls"));
    }

    #[test]
    fn transitive_expansion() {
        let f = map(&[("default", &["full"]), ("full", &["tls", "json"]), ("tls", &[]), ("json", &[])]);
        let active = resolve_features(&f, &[], true).unwrap();
        assert_eq!(active, ["full", "json", "tls"].iter().map(|s| s.to_string()).collect());
    }

    #[test]
    fn default_not_in_active_set() {
        let f = map(&[("default", &["logging"]), ("logging", &[])]);
        let active = resolve_features(&f, &[], true).unwrap();
        assert!(!active.contains("default"));
    }

    #[test]
    fn no_defaults_with_flag_false() {
        let f = map(&[("default", &["logging"]), ("logging", &[]), ("tls", &[])]);
        let active = resolve_features(&f, &["tls".to_string()], false).unwrap();
        assert!(!active.contains("logging"));
        assert!(active.contains("tls"));
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
}
