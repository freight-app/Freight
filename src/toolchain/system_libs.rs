//! System-library stubs.
//!
//! Each stub describes a well-known OS library (pthread, ws2_32, …): the linker
//! name it maps to and the headers it provides. Freight uses these as the final
//! step in `resolve_version_dep` when pkg-config fails, and to link versionless
//! OS libraries declared via `[os.*]/[arch.*] features`.
//!
//! The stub data is **data-driven**: the built-in set lives in the bundled
//! `system-libs.toml` (embedded at compile time), and users can add or override
//! entries with `.toml` files in `$FREIGHT_HOME/toolchains/system-libs/`
//! (default `~/.freight/toolchains/system-libs/`) — no recompile needed.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::supports::eval_supports;
use crate::toolchain::cache::freight_home;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SystemLibStub {
    /// Package name (matches the feature name in `[os.*] features = [...]`).
    pub name: String,
    /// Linker flag: `-l<link_name>` (GCC/Clang) or `<link_name>.lib` (MSVC).
    pub link_name: String,
    /// Header filenames this library provides (include-hygiene attribution / TUI).
    pub headers: Vec<String>,
}

// ── Data file format ────────────────────────────────────────────────────────────

/// The built-in stub table, embedded at compile time.
const BUNDLED_STUBS: &str = include_str!("system-libs.toml");

#[derive(Debug, Clone, Deserialize)]
struct RawStub {
    /// Linker name; defaults to the table key when omitted.
    #[serde(default)]
    link: Option<String>,
    /// Boolean platform expression evaluated against the host.
    supports: String,
    #[serde(default)]
    headers: Vec<String>,
}

/// Parse a stub `.toml` document into a name→entry map; empty on parse error.
fn parse_stub_doc(src: &str) -> BTreeMap<String, RawStub> {
    toml::from_str(src).unwrap_or_default()
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Return all system-lib stubs that match the current host platform.
///
/// Built-in entries are loaded from the bundled table, then overridden/extended
/// by any user `.toml` files under `$FREIGHT_HOME/toolchains/system-libs/`
/// (a user entry with the same name replaces the built-in). Only stubs whose
/// `supports` expression matches the host are returned.
pub fn load_system_lib_stubs() -> Vec<SystemLibStub> {
    let mut table = parse_stub_doc(BUNDLED_STUBS);

    if let Some(dir) = freight_home().map(|h| h.join("toolchains").join("system-libs")) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut files: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect();
            files.sort(); // deterministic override order
            for path in files {
                if let Ok(src) = std::fs::read_to_string(&path) {
                    for (name, stub) in parse_stub_doc(&src) {
                        table.insert(name, stub); // user entry wins
                    }
                }
            }
        }
    }

    table
        .into_iter()
        .filter(|(_, s)| eval_supports(&s.supports))
        .map(|(name, s)| SystemLibStub {
            link_name: s.link.unwrap_or_else(|| name.clone()),
            name,
            headers: s.headers,
        })
        .collect()
}

/// Find the stub for `name` from a pre-loaded slice (case-insensitive).
pub fn find_stub<'a>(name: &str, stubs: &'a [SystemLibStub]) -> Option<&'a SystemLibStub> {
    stubs.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_stubs_parse() {
        let table = parse_stub_doc(BUNDLED_STUBS);
        assert!(!table.is_empty(), "bundled system-libs.toml must parse");
        let pthread = table.get("pthread").expect("pthread stub present");
        assert_eq!(pthread.supports, "unix");
        assert!(pthread.headers.iter().any(|h| h == "pthread.h"));
        // `link` defaults to the table key when omitted.
        assert!(pthread.link.is_none());
        let ws2 = table.get("ws2_32").expect("ws2_32 stub present");
        assert_eq!(ws2.supports, "windows");
    }

    #[test]
    fn pthread_loaded_on_unix() {
        if cfg!(unix) {
            let stubs = load_system_lib_stubs();
            let s = find_stub("pthread", &stubs).expect("pthread should be present on unix");
            assert_eq!(s.link_name, "pthread");
            assert!(s.headers.contains(&"pthread.h".to_string()));
        }
    }

    #[test]
    fn windows_stubs_not_loaded_on_unix() {
        if cfg!(unix) {
            let stubs = load_system_lib_stubs();
            assert!(find_stub("ws2_32", &stubs).is_none());
            assert!(find_stub("kernel32", &stubs).is_none());
        }
    }

    #[test]
    fn find_stub_case_insensitive() {
        let stubs = load_system_lib_stubs();
        if let Some(s) = stubs.first() {
            assert!(find_stub(&s.name.to_uppercase(), &stubs).is_some());
        }
    }
}
