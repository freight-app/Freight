//! Runtime vendor database — architectures, operating systems, compiler families.
//!
//! Entries are loaded from `vendors/*.toml` at the workspace/install root.
//! Adding a new target (arch, OS, or compiler family) is a matter of dropping
//! a TOML file into that directory — no Rust changes needed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    Arch,
    Compiler,
    Os,
}

/// In-memory index of all vendor entries, keyed by lowercase token
/// (canonical name *and* every alias).
pub struct VendorDb {
    map: HashMap<String, (String, EntryKind)>,
}

impl VendorDb {
    /// Load all `.toml` files from `dir` and build the lookup table.
    /// Files with an unrecognised `kind` are silently ignored.
    pub fn load(dir: &Path) -> Self {
        let mut map = HashMap::new();

        let Ok(entries) = std::fs::read_dir(dir) else {
            return Self { map };
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            let Ok(vf)      = toml_edit::de::from_str::<VendorFile>(&content) else { continue };

            let kind = match vf.kind.as_str() {
                "arch"     => EntryKind::Arch,
                "os"       => EntryKind::Os,
                "compiler" => EntryKind::Compiler,
                _          => continue,
            };

            // Canonical name maps to itself.
            map.insert(vf.name.to_lowercase(), (vf.name.clone(), kind));
            // Every alias maps to the canonical name.
            for alias in &vf.aliases {
                map.insert(alias.to_lowercase(), (vf.name.clone(), kind));
            }
        }

        Self { map }
    }

    /// Classify a single triple token. Returns `(canonical_name, kind)` or
    /// `None` when the token is unrecognised (vendor token, garbage, etc.).
    pub fn classify(&self, token: &str) -> Option<(String, EntryKind)> {
        self.map.get(token.to_lowercase().as_str()).cloned()
    }
}

// ── Directory resolution ──────────────────────────────────────────────────────

/// Locate the `vendors/` directory.
///
/// Search order:
///   1. `FREIGHT_VENDORS_DIR` env var
///   2. `{binary_dir}/vendors/`
///   3. `{binary_dir}/../../vendors/`  (cargo dev layout)
pub fn vendors_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("FREIGHT_VENDORS_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() { return Some(p); }
    }

    let exe     = std::env::current_exe().ok()?;
    let bin_dir = exe.parent()?;

    let c1 = bin_dir.join("vendors");
    if c1.is_dir() { return Some(c1); }

    // cargo dev layout: target/debug/freight → workspace root two levels up
    let c2 = bin_dir.join("..").join("..").join("vendors").canonicalize().ok()?;
    if c2.is_dir() { return Some(c2); }

    None
}

// ── Global database ───────────────────────────────────────────────────────────

/// Return the process-wide `VendorDb`, loading it once on first call.
/// Falls back to an empty database (all tokens unknown → host fallback) when
/// the `vendors/` directory cannot be located.
pub fn global_db() -> &'static VendorDb {
    static DB: OnceLock<VendorDb> = OnceLock::new();
    DB.get_or_init(|| {
        vendors_dir()
            .map(|d| VendorDb::load(&d))
            .unwrap_or_else(|| VendorDb { map: HashMap::new() })
    })
}

// ── Triple parsing ────────────────────────────────────────────────────────────

/// Derive `(arch, os)` from a partial or full target specifier.
///
/// Freight accepts any subset of `arch-os-compiler_family` — missing
/// components default to the host value:
///
/// | Input               | arch    | os      |
/// |---------------------|---------|---------|
/// | `aarch64-linux-gnu` | aarch64 | linux   |
/// | `aarch64`           | aarch64 | *host*  |
/// | `linux`             | *host*  | linux   |
/// | `linux-gnu`         | *host*  | linux   |
/// | `x86_64-windows`    | x86_64  | windows |
///
/// Standard 4-part GNU triples (`x86_64-unknown-linux-gnu`,
/// `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`) are also handled —
/// unrecognised vendor tokens (`unknown`, `pc`, `apple`) are skipped.
pub fn parse_triple(triple: &str) -> (String, String) {
    parse_triple_with(triple, global_db())
}

/// Pure form of `parse_triple` — accepts an explicit `VendorDb` for testing.
pub fn parse_triple_with(triple: &str, db: &VendorDb) -> (String, String) {
    let mut arch_out: Option<String> = None;
    let mut os_out:   Option<String> = None;

    for part in triple.split('-') {
        match db.classify(part) {
            Some((canonical, EntryKind::Arch)) if arch_out.is_none() => {
                arch_out = Some(canonical);
            }
            Some((canonical, EntryKind::Os)) if os_out.is_none() => {
                os_out = Some(canonical);
            }
            _ => {}
        }
    }

    let arch = arch_out.unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let os   = os_out.unwrap_or_else(|| std::env::consts::OS.to_string());
    (arch, os)
}

// ── Internal deserialisation ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct VendorFile {
    name:    String,
    kind:    String,
    #[serde(default)]
    aliases: Vec<String>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use super::{VendorDb, parse_triple_with};

    fn db() -> VendorDb {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..").join("..").join("vendors");
        VendorDb::load(&dir)
    }

    #[test]
    fn full_triple() {
        let db = db();
        assert_eq!(parse_triple_with("aarch64-linux-gnu",   &db), ("aarch64".into(), "linux".into()));
        assert_eq!(parse_triple_with("x86_64-windows-msvc", &db), ("x86_64".into(),  "windows".into()));
        assert_eq!(parse_triple_with("x86_64-macos-clang",  &db), ("x86_64".into(),  "macos".into()));
    }

    #[test]
    fn gnu_4part_triples() {
        let db = db();
        assert_eq!(parse_triple_with("x86_64-unknown-linux-gnu",  &db), ("x86_64".into(),  "linux".into()));
        assert_eq!(parse_triple_with("x86_64-pc-windows-msvc",    &db), ("x86_64".into(),  "windows".into()));
        assert_eq!(parse_triple_with("x86_64-apple-darwin",       &db), ("x86_64".into(),  "macos".into()));
        assert_eq!(parse_triple_with("aarch64-unknown-linux-gnu", &db), ("aarch64".into(), "linux".into()));
    }

    #[test]
    fn arch_only_falls_back_to_host_os() {
        let db = db();
        let (arch, os) = parse_triple_with("aarch64", &db);
        assert_eq!(arch, "aarch64");
        assert_eq!(os, std::env::consts::OS);
    }

    #[test]
    fn os_only_falls_back_to_host_arch() {
        let db = db();
        let (arch, os) = parse_triple_with("linux", &db);
        assert_eq!(arch, std::env::consts::ARCH);
        assert_eq!(os, "linux");
    }

    #[test]
    fn compiler_only_falls_back_to_host_arch_and_os() {
        let db = db();
        let (arch, os) = parse_triple_with("gnu", &db);
        assert_eq!(arch, std::env::consts::ARCH);
        assert_eq!(os, std::env::consts::OS);
    }

    #[test]
    fn os_compiler_falls_back_to_host_arch() {
        let db = db();
        let (arch, os) = parse_triple_with("linux-gnu", &db);
        assert_eq!(arch, std::env::consts::ARCH);
        assert_eq!(os, "linux");
    }

    #[test]
    fn normalises_aliases() {
        let db = db();
        assert_eq!(parse_triple_with("amd64-linux-gnu",   &db), ("x86_64".into(),  "linux".into()));
        assert_eq!(parse_triple_with("arm64-linux-gnu",   &db), ("aarch64".into(), "linux".into()));
        assert_eq!(parse_triple_with("x86_64-darwin-gnu", &db), ("x86_64".into(),  "macos".into()));
    }
}
