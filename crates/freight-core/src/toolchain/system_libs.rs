//! Loader for built-in system-library stubs (`toolchains/system-libs/*.toml`).
//!
//! Each stub is a minimal `freight.toml`-compatible manifest that describes a
//! well-known OS library (pthread, ws2_32, …). Freight uses these as the final
//! step in `resolve_version_dep` when pkg-config, conan, and vcpkg all fail.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::supports::eval_supports;

use super::detect::templates_dir;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SystemLibStub {
    /// Package name (matches the dep key in `freight.toml`).
    pub name: String,
    /// Linker flag: `-l<link_name>`.
    pub link_name: String,
    /// Header filenames that users should `#include` (display / TUI only).
    pub headers: Vec<String>,
}

// ── Loader ────────────────────────────────────────────────────────────────────

/// Load all system-lib stubs from `toolchains/system-libs/` that match the
/// current host platform (via their `supports` expression).
pub fn load_system_lib_stubs() -> Vec<SystemLibStub> {
    let Some(toolchains) = templates_dir() else { return vec![] };
    load_from(&toolchains.join("system-libs"))
}

pub fn load_from(dir: &Path) -> Vec<SystemLibStub> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut stubs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        let Ok(raw) = toml::from_str::<RawStub>(&src) else { continue };
        // Skip stubs whose platform expression doesn't match the current host.
        if let Some(expr) = &raw.package.supports {
            if !eval_supports(expr) {
                continue;
            }
        }
        let link_name = raw.lib.as_ref()
            .and_then(|l| l.link.clone())
            .unwrap_or_else(|| raw.package.name.clone());
        let headers = raw.lib.as_ref()
            .map(|l| l.hdrs.clone())
            .unwrap_or_default();
        stubs.push(SystemLibStub {
            name: raw.package.name,
            link_name,
            headers,
        });
    }
    stubs.sort_by(|a, b| a.name.cmp(&b.name));
    stubs
}

/// Find the stub for `name` from a pre-loaded slice (case-insensitive).
pub fn find_stub<'a>(name: &str, stubs: &'a [SystemLibStub]) -> Option<&'a SystemLibStub> {
    stubs.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

// ── Deserialisation helpers (minimal subset of freight.toml) ─────────────────

#[derive(Deserialize)]
struct RawStub {
    package: RawPackage,
    #[serde(rename = "lib")]
    lib: Option<RawLib>,
}

#[derive(Deserialize)]
struct RawPackage {
    name: String,
    #[serde(default)]
    supports: Option<String>,
}

#[derive(Deserialize)]
struct RawLib {
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    hdrs: Vec<String>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_stub(dir: &Path, filename: &str, content: &str) {
        let mut f = std::fs::File::create(dir.join(filename)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn loads_matching_stub() {
        let dir = tempfile::tempdir().unwrap();
        let os = std::env::consts::OS;
        write_stub(dir.path(), "testlib.toml", &format!(
            "[package]\nname = \"testlib\"\nsupports = \"{os}\"\n\n[lib]\nlink = \"testlink\"\nhdrs = [\"test.h\"]\n"
        ));
        let stubs = load_from(dir.path());
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].link_name, "testlink");
        assert_eq!(stubs[0].headers, vec!["test.h"]);
    }

    #[test]
    fn skips_non_matching_stub() {
        let dir = tempfile::tempdir().unwrap();
        // Use a definitely-wrong platform expression.
        write_stub(dir.path(), "nowhere.toml",
            "[package]\nname = \"nowhere\"\nsupports = \"!linux & !windows & !macos & !freebsd\"\n"
        );
        let stubs = load_from(dir.path());
        assert!(stubs.is_empty());
    }
}
