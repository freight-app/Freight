//! Header → package ownership for include hygiene (Phase 3).
//!
//! Some declared dependencies are *system* libraries whose public headers live
//! in a default compiler search dir (`/usr/include`) with no dedicated
//! subdirectory — `<zlib.h>`, `<sqlite3.h>`, `<cblas.h>`. Phase 2 alone would
//! flag those as undeclared even when the project legitimately declares the
//! owning package, because there is no project/dep include dir they resolve
//! under. This module supplies the missing attribution so a system-resolved
//! header can be tied back to a *declared* package.
//!
//! Two complementary sources (see `docs/include-hygiene.md`):
//!
//! - **Tier A** — a curated, per-OS ownership table (this file's [`seed`], plus
//!   an optional downloaded/cached override). Keyed by **freight package /
//!   slot name** (not the OS package), which keeps it distro-portable. This is
//!   the only way to attribute the bare-`/usr/include` long tail.
//! - **Tier B** — [`pkg_config_dedicated_dirs`]: a declared dep's pkg-config
//!   `--cflags` include dirs, **excluding** default system roots. Safe to fold
//!   into the allowlist (a dedicated `/usr/include/SDL2` won't over-allow),
//!   version-correct, and needs no curated data.
//!
//! Ownership is many-to-many: interchangeable implementations (BLAS: OpenBLAS /
//! ATLAS / MKL …) all provide `cblas.h`. That is modelled as a **slot** with
//! several **providers**; declaring any one provider attributes the slot's
//! headers. A shared header is therefore an *OR*, never a conflict.
//!
//! Fail-open: if no ownership data is available, this module simply attributes
//! nothing — it can only ever *add* "declared" headers, never manufacture
//! undeclared ones.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

/// A slot: a capability several interchangeable packages can provide.
#[derive(Clone, Default)]
pub struct Slot {
    /// Package names that satisfy this slot (e.g. BLAS implementations).
    pub providers: Vec<String>,
    /// Header glob patterns the slot owns.
    pub headers: Vec<String>,
}

/// Tier-A ownership data for one OS.
#[derive(Clone, Default)]
pub struct OwnershipData {
    /// Package name → header glob patterns it owns directly.
    pub packages: HashMap<String, Vec<String>>,
    /// Slot name → providers + owned header globs.
    pub slots: HashMap<String, Slot>,
}

impl OwnershipData {
    /// The set of header globs attributed to a set of *declared* dependency
    /// names: each name's direct globs, plus the globs of any slot it names or
    /// provides.
    pub fn owned_globs_for(&self, declared: &[String]) -> Vec<String> {
        let declared_set: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
        let mut globs: Vec<String> = Vec::new();
        for name in &declared_set {
            if let Some(g) = self.packages.get(*name) {
                globs.extend(g.iter().cloned());
            }
            // The name *is* a slot (e.g. a project depends on `blas` directly).
            if let Some(slot) = self.slots.get(*name) {
                globs.extend(slot.headers.iter().cloned());
            }
        }
        // Slots whose provider is declared.
        for slot in self.slots.values() {
            if slot.providers.iter().any(|p| declared_set.contains(p.as_str())) {
                globs.extend(slot.headers.iter().cloned());
            }
        }
        globs.sort();
        globs.dedup();
        globs
    }

    /// Every package name freight knows about (direct owners + all slot
    /// providers), sorted and de-duplicated. Used to offer `[dependencies]`
    /// completions for common system libraries in `freight.toml`.
    pub fn known_packages(&self) -> Vec<String> {
        let mut out: BTreeSet<String> = self.packages.keys().cloned().collect();
        for slot in self.slots.values() {
            out.extend(slot.providers.iter().cloned());
        }
        out.into_iter().collect()
    }

    /// Packages a user could declare to legitimately obtain `header` — used to
    /// turn an undeclared-include diagnostic into "declare one of: …".
    pub fn candidates_for_header(&self, header: &str) -> Vec<String> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (pkg, globs) in &self.packages {
            if globs.iter().any(|g| glob_match(g, header)) {
                out.insert(pkg.clone());
            }
        }
        for slot in self.slots.values() {
            if slot.headers.iter().any(|g| glob_match(g, header)) {
                out.extend(slot.providers.iter().cloned());
            }
        }
        out.into_iter().collect()
    }
}

/// Load the ownership table for the current OS: the in-core [`seed`] merged
/// under any downloaded/cached override file (the override wins). Always
/// succeeds — a missing or malformed override degrades to the seed.
pub fn load() -> OwnershipData {
    let mut data = seed();
    if let Some(path) = override_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(extra) = parse_override(&text) {
                merge(&mut data, extra);
            }
        }
    }
    data
}

/// Path to the optional downloaded/cached ownership file for this OS, if a home
/// directory is known. The file is not required; it is the Tier-A update channel.
fn override_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(
        base.join("freight")
            .join(format!("header-ownership-{}.toml", std::env::consts::OS)),
    )
}

fn merge(into: &mut OwnershipData, extra: OwnershipData) {
    for (k, v) in extra.packages {
        into.packages.entry(k).or_default().extend(v);
    }
    for (k, v) in extra.slots {
        let slot = into.slots.entry(k).or_default();
        slot.providers.extend(v.providers);
        slot.headers.extend(v.headers);
    }
}

/// Parse a downloaded override file. Format mirrors the seed:
/// ```toml
/// [packages]
/// zlib = ["zlib.h", "zconf.h"]
/// [slots.blas]
/// providers = ["openblas", "atlas"]
/// headers = ["cblas.h", "blas.h"]
/// ```
fn parse_override(text: &str) -> Option<OwnershipData> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let mut data = OwnershipData::default();
    if let Some(pkgs) = value.get("packages").and_then(|v| v.as_table()) {
        for (name, globs) in pkgs {
            let list = globs
                .as_array()
                .map(|a| a.iter().filter_map(|g| g.as_str().map(String::from)).collect())
                .unwrap_or_default();
            data.packages.insert(name.clone(), list);
        }
    }
    if let Some(slots) = value.get("slots").and_then(|v| v.as_table()) {
        for (name, slot) in slots {
            let providers = slot
                .get("providers")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|g| g.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let headers = slot
                .get("headers")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|g| g.as_str().map(String::from)).collect())
                .unwrap_or_default();
            data.slots
                .insert(name.clone(), Slot { providers, headers });
        }
    }
    Some(data)
}

/// pkg-config `--cflags` include dirs for `dep`, **excluding** default system
/// roots (Tier B). Returns only dedicated subdirs that are safe to add to the
/// allowlist without over-allowing all of `/usr/include`.
pub fn pkg_config_dedicated_dirs(dep: &str) -> Vec<PathBuf> {
    let Ok(result) = crate::adaptors::pkg_config::pkg_config_query(dep) else {
        return Vec::new();
    };
    result
        .include_dirs
        .into_iter()
        .filter(|d| !is_default_system_dir(d))
        .collect()
}

/// Whether `dir` is a default compiler search root (folding it into the
/// allowlist would attribute every header under it).
fn is_default_system_dir(dir: &std::path::Path) -> bool {
    matches!(
        dir.to_str(),
        Some("/usr/include") | Some("/usr/local/include") | Some("/include")
    )
}

/// Match a header path against a glob pattern. `*` matches any run of
/// characters (including `/`); everything else is literal. Sufficient for the
/// `name.h` / `dir/*` ownership patterns used here.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn helper(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => {
                // Try to consume zero-or-more chars for `*`.
                helper(&p[1..], t) || (!t.is_empty() && helper(p, &t[1..]))
            }
            Some(&c) => !t.is_empty() && t[0] == c && helper(&p[1..], &t[1..]),
        }
    }
    helper(pattern.as_bytes(), text.as_bytes())
}

/// The in-core, per-OS Tier-A seed. Focused on libraries whose headers sit bare
/// in `/usr/include` (where Tier B / pkg-config can't disambiguate) plus the
/// BLAS/LAPACK slots. Other OSes start empty and rely on Tier B + the override
/// file until seeded.
pub fn seed() -> OwnershipData {
    let mut data = OwnershipData::default();
    if cfg!(target_os = "linux") {
        let pkg = |d: &mut OwnershipData, name: &str, globs: &[&str]| {
            d.packages
                .insert(name.to_string(), globs.iter().map(|s| s.to_string()).collect());
        };
        pkg(&mut data, "zlib", &["zlib.h", "zconf.h"]);
        pkg(&mut data, "sqlite3", &["sqlite3.h", "sqlite3ext.h"]);
        pkg(&mut data, "bzip2", &["bzlib.h"]);
        pkg(&mut data, "liblzma", &["lzma.h", "lzma/*"]);
        pkg(&mut data, "expat", &["expat.h", "expat_external.h"]);
        pkg(&mut data, "pcre2", &["pcre2.h"]);
        pkg(&mut data, "gmp", &["gmp.h", "gmpxx.h"]);
        pkg(&mut data, "mpfr", &["mpfr.h", "mpf2mpfr.h"]);
        pkg(&mut data, "ncurses", &["ncurses.h", "curses.h", "term.h", "ncurses/*"]);
        pkg(&mut data, "readline", &["readline/*"]);
        pkg(&mut data, "uuid", &["uuid/uuid.h"]);

        data.slots.insert(
            "blas".to_string(),
            Slot {
                providers: vec![
                    "openblas".into(),
                    "atlas".into(),
                    "blis".into(),
                    "mkl".into(),
                    "blas".into(),
                    "reference-blas".into(),
                ],
                headers: vec!["cblas.h".into(), "blas.h".into(), "f77blas.h".into()],
            },
        );
        data.slots.insert(
            "lapack".to_string(),
            Slot {
                providers: vec![
                    "lapack".into(),
                    "openblas".into(),
                    "atlas".into(),
                    "mkl".into(),
                ],
                headers: vec!["lapack.h".into(), "lapacke.h".into(), "clapack.h".into()],
            },
        );
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches() {
        assert!(glob_match("zlib.h", "zlib.h"));
        assert!(!glob_match("zlib.h", "zconf.h"));
        assert!(glob_match("openssl/*", "openssl/ssl.h"));
        assert!(glob_match("ncurses/*", "ncurses/curses.h"));
        assert!(!glob_match("ncurses/*", "ncurses.h"));
        assert!(glob_match("*", "anything/at/all.h"));
    }

    #[test]
    fn declared_blas_provider_owns_slot_headers() {
        let d = seed();
        if !cfg!(target_os = "linux") {
            return; // seed is Linux-only for now
        }
        // Declaring OpenBLAS attributes the BLAS + LAPACK slot headers.
        let globs = d.owned_globs_for(&["openblas".to_string()]);
        assert!(globs.iter().any(|g| g == "cblas.h"));
        assert!(globs.iter().any(|g| g == "lapacke.h"));
        // A header none of the declared deps own is not attributed.
        let none = d.owned_globs_for(&["zlib".to_string()]);
        assert!(none.iter().any(|g| g == "zlib.h"));
        assert!(!none.iter().any(|g| g == "cblas.h"));
    }

    #[test]
    fn candidates_lists_all_blas_providers() {
        let d = seed();
        if !cfg!(target_os = "linux") {
            return;
        }
        let cands = d.candidates_for_header("cblas.h");
        assert!(cands.contains(&"openblas".to_string()));
        assert!(cands.contains(&"mkl".to_string()));
        // zlib.h points at the single owning package.
        assert_eq!(d.candidates_for_header("zlib.h"), vec!["zlib".to_string()]);
        // An unknown header has no candidates.
        assert!(d.candidates_for_header("definitely_not_a_lib.h").is_empty());
    }

    #[test]
    fn default_system_dirs_excluded() {
        assert!(is_default_system_dir(std::path::Path::new("/usr/include")));
        assert!(is_default_system_dir(std::path::Path::new("/usr/local/include")));
        assert!(!is_default_system_dir(std::path::Path::new("/usr/include/SDL2")));
    }
}
