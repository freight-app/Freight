//! CMake dependency helpers.
//!
//! - **Scan** a project's `CMakeLists.txt` (+ `cmake/*.cmake` modules) for
//!   `find_package(<Name>)` names and map each to the freight/pkg-config name —
//!   used by `freight init` to harvest a project's dependencies.
//! - **Classify availability**: is a name already installed on the host
//!   (pkg-config or an installed `<Name>Config.cmake`), or available in a freight
//!   registry ([`ConfiguredRegistries`])?
//!
//! Build-time *resolution* (deciding what to build for a CMake project) is done
//! dynamically by the cmake plugin's CMake dependency provider, which calls
//! `freight cmake-provide <name>` on demand — see `plugins/cmake/cmake.freight`
//! and `build::pipeline::provide_cmake_package`.

use std::path::{Path, PathBuf};

/// CMake `find_package` names that are pure toolchain/system facilities. These
/// are wired through `[os.*]`/`[arch.*]` features or the compiler, never as
/// freight package dependencies, so they terminate resolution.
pub const CMAKE_SYSTEM_PKGS: &[&str] = &[
    "threads", "openmp", "mpi", "openacc", "cudatoolkit", "opengl", "opengles2",
    "glut", "x11", "doxygen", "git", "python", "python2", "python3", "pythonlibs",
    "pythoninterp", "pkgconfig",
];

/// Whether a CMake `find_package` name is a pure toolchain/system facility.
pub fn is_system_pkg(name: &str) -> bool {
    CMAKE_SYSTEM_PKGS.contains(&name.to_lowercase().as_str())
}

/// Whether an installed CMake **config package** exists for `name` on this host —
/// i.e. `find_package(<name> CONFIG)` would succeed. This catches libraries whose
/// CMake name differs from their pkg-config name (e.g. `find_package(c-ares)` is
/// `libcares` to pkg-config) and header/config-only packages with no `.pc` at all.
/// Searches the standard config locations plus `CMAKE_PREFIX_PATH`.
pub fn is_installed_cmake_package(name: &str) -> bool {
    let lower = name.to_lowercase();
    // Config file names CMake accepts, lower-cased for comparison.
    let wanted = [format!("{lower}config.cmake"), format!("{lower}-config.cmake")];

    let mut bases: Vec<PathBuf> = [
        "/usr/lib",
        "/usr/lib64",
        "/usr/local/lib",
        "/usr/local/lib64",
        "/usr/share",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Ok(p) = std::env::var("CMAKE_PREFIX_PATH") {
        for part in p.split(if cfg!(windows) { ';' } else { ':' }) {
            if !part.is_empty() {
                bases.push(PathBuf::from(part).join("lib"));
                bases.push(PathBuf::from(part).join("share"));
            }
        }
    }

    for base in bases {
        // CMake looks under <base>/cmake/<Name>/ (the dir casing varies).
        for dir_name in [name.to_string(), lower.clone()] {
            let dir = base.join("cmake").join(&dir_name);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let f = e.file_name().to_string_lossy().to_lowercase();
                    if wanted.contains(&f) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Map a CMake `find_package` package name to the freight / pkg-config package
/// name used to look it up (registry or `pkg-config --modversion`). Defaults to
/// the lowercased CMake name; the table covers the common cases where they
/// differ (`PNG` → `libpng`, …).
pub fn cmake_to_freight_name(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "png" => "libpng".into(),
        "jpeg" => "libjpeg".into(),
        "curl" => "libcurl".into(),
        "freetype" => "freetype2".into(),
        "tiff" => "libtiff-4".into(),
        "webp" => "libwebp".into(),
        "xml2" | "libxml2" => "libxml-2.0".into(),
        "zstd" => "libzstd".into(),
        "lz4" => "liblz4".into(),
        other => other.into(),
    }
}

/// Scan a `CMakeLists.txt` for `find_package(<Name> ...)` calls, returning the
/// distinct package names in source order (system/toolchain packages dropped).
pub fn detect_cmake_packages(cmake_file: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(cmake_file) else {
        return Vec::new();
    };
    detect_cmake_packages_in(&text)
}

/// `.cmake` module files a project keeps under `cmake/` (recursive) plus any at
/// the project root. Large projects (gRPC, …) put their `find_package` calls in
/// these modules rather than the top-level `CMakeLists.txt`.
fn cmake_module_files(project_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for pat in ["cmake/**/*.cmake", "*.cmake"] {
        let full = project_dir.join(pat);
        if let Ok(paths) = glob::glob(&full.to_string_lossy()) {
            out.extend(paths.flatten());
        }
    }
    out
}

/// `find_package` names across a whole project: its `CMakeLists.txt` plus its
/// `cmake/` modules. This is what catches deps in real projects that factor
/// `find_package` into `cmake/<dep>.cmake` files.
pub fn detect_cmake_packages_in_project(project_dir: &Path) -> Vec<String> {
    let mut names = detect_cmake_packages(&project_dir.join("CMakeLists.txt"));
    for f in cmake_module_files(project_dir) {
        for n in detect_cmake_packages(&f) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    names
}

/// [`detect_cmake_packages`] over already-read CMake text (the testable core).
pub fn detect_cmake_packages_in(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let needle = "find_package";
    let mut base = 0usize;
    while let Some(rel) = text[base..].find(needle) {
        let after = base + rel + needle.len();
        // Skip whitespace, require an opening paren, then read the first token.
        let mut i = after;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'(' {
            i += 1;
            while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                i += 1;
            }
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
            {
                i += 1;
            }
            if i > start {
                let name = &text[start..i];
                let lower = name.to_lowercase();
                if !CMAKE_SYSTEM_PKGS.contains(&lower.as_str())
                    && !names.iter().any(|n| n == name)
                {
                    names.push(name.to_string());
                }
            }
        }
        base = after;
    }
    names
}

// ── FetchContent detection ──────────────────────────────────────────────────

/// A dependency declared via CMake's `FetchContent_Declare(name ...)`. Either a
/// git source (`GIT_REPOSITORY` + optional `GIT_TAG`) or an archive (`URL` +
/// optional `URL_HASH SHA256=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchContentDep {
    pub name: String,
    pub url: String,
    pub is_git: bool,
    /// `GIT_TAG` value — a tag/branch or a commit. `None` if unspecified.
    pub git_ref: Option<String>,
    /// `true` when `git_ref` looks like a commit SHA (hex, length ≥ 7) → emit as
    /// `rev`; otherwise emit as `tag`.
    pub ref_is_rev: bool,
    /// `SHA256=` from `URL_HASH`, for an archive source.
    pub sha256: Option<String>,
}

/// `FetchContent_Declare` deps across a project (CMakeLists.txt + `cmake/` modules).
pub fn detect_fetchcontent_in_project(project_dir: &Path) -> Vec<FetchContentDep> {
    let mut out: Vec<FetchContentDep> = Vec::new();
    let mut push = |deps: Vec<FetchContentDep>| {
        for d in deps {
            if !out.iter().any(|e| e.name == d.name) {
                out.push(d);
            }
        }
    };
    if let Ok(text) = std::fs::read_to_string(project_dir.join("CMakeLists.txt")) {
        push(detect_fetchcontent_in(&text));
    }
    for f in cmake_module_files(project_dir) {
        if let Ok(text) = std::fs::read_to_string(&f) {
            push(detect_fetchcontent_in(&text));
        }
    }
    out
}

/// Classify a `GIT_TAG` value: a bare hex string of length ≥ 7 is treated as a
/// commit SHA (→ `rev`); anything else as a tag/branch (→ `tag`).
fn looks_like_commit(s: &str) -> bool {
    s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Parse `FetchContent_Declare(...)` blocks from CMake text (the testable core).
pub fn detect_fetchcontent_in(text: &str) -> Vec<FetchContentDep> {
    let mut out: Vec<FetchContentDep> = Vec::new();
    let needle = "FetchContent_Declare";
    let bytes = text.as_bytes();
    let mut base = 0usize;
    while let Some(rel) = text[base..].find(needle) {
        let after = base + rel + needle.len();
        base = after;
        // Skip whitespace, require '(', then capture to the matching ')'.
        let mut i = after;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'(' {
            continue;
        }
        i += 1;
        let arg_start = i;
        let mut depth = 1;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                i += 1;
            }
        }
        let args = &text[arg_start..i];
        base = i;
        if let Some(dep) = parse_fetchcontent_args(args) {
            if !out.iter().any(|e| e.name == dep.name) {
                out.push(dep);
            }
        }
    }
    out
}

/// Turn the whitespace-separated argument list of a `FetchContent_Declare` into a
/// `FetchContentDep`. Returns `None` when there is no name or no source URL.
fn parse_fetchcontent_args(args: &str) -> Option<FetchContentDep> {
    let toks: Vec<&str> = args
        .split_whitespace()
        .map(|t| t.trim_matches('"'))
        .filter(|t| !t.is_empty())
        .collect();
    let name = toks.first()?.to_string();
    let mut git_url = None;
    let mut archive_url = None;
    let mut git_ref = None;
    let mut sha256 = None;
    let mut idx = 1;
    while idx < toks.len() {
        match toks[idx].to_ascii_uppercase().as_str() {
            "GIT_REPOSITORY" => {
                git_url = toks.get(idx + 1).map(|s| s.to_string());
                idx += 2;
            }
            "GIT_TAG" => {
                git_ref = toks.get(idx + 1).map(|s| s.to_string());
                idx += 2;
            }
            "URL" => {
                archive_url = toks.get(idx + 1).map(|s| s.to_string());
                idx += 2;
            }
            "URL_HASH" => {
                if let Some(h) = toks.get(idx + 1) {
                    sha256 = h
                        .split_once('=')
                        .filter(|(algo, _)| algo.eq_ignore_ascii_case("SHA256"))
                        .map(|(_, hash)| hash.to_string());
                }
                idx += 2;
            }
            _ => idx += 1,
        }
    }
    let (url, is_git) = match (git_url, archive_url) {
        (Some(g), _) => (g, true),
        (None, Some(a)) => (a, false),
        (None, None) => return None,
    };
    let ref_is_rev = git_ref.as_deref().map(looks_like_commit).unwrap_or(false);
    Some(FetchContentDep {
        name,
        url,
        is_git,
        git_ref,
        ref_is_rev,
        sha256,
    })
}

// ── add_subdirectory vendoring detection ────────────────────────────────────

/// `add_subdirectory(<dir> …)` source-dir arguments across a project (CMakeLists.txt
/// and `cmake/` modules), in source order, de-duplicated. Variable-expanded paths
/// like `${...}` are skipped — only literal paths are returned.
pub fn detect_add_subdirectory_in_project(project_dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |paths: Vec<String>| {
        for p in paths {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    };
    if let Ok(text) = std::fs::read_to_string(project_dir.join("CMakeLists.txt")) {
        push(detect_add_subdirectory_in(&text));
    }
    for f in cmake_module_files(project_dir) {
        if let Ok(text) = std::fs::read_to_string(&f) {
            push(detect_add_subdirectory_in(&text));
        }
    }
    out
}

/// Parse `add_subdirectory(<dir> …)` literal source-dir paths from CMake text.
pub fn detect_add_subdirectory_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let needle = "add_subdirectory";
    let mut base = 0usize;
    while let Some(rel) = text[base..].find(needle) {
        let after = base + rel + needle.len();
        base = after;
        let mut i = after;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'(' {
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        // Read the first argument (the source dir): quoted or a bare path token.
        let quoted = i < bytes.len() && bytes[i] == b'"';
        if quoted {
            i += 1;
        }
        let start = i;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if quoted {
                if c == '"' {
                    break;
                }
            } else if c.is_whitespace() || c == ')' {
                break;
            }
            i += 1;
        }
        let path = text[start..i].trim();
        // Skip variable-expanded paths — we can't resolve them statically.
        if !path.is_empty() && !path.contains('$') && !out.contains(&path.to_string()) {
            out.push(path.to_string());
        }
        base = i;
    }
    out
}

// ── Transitive registry-first resolution ────────────────────────────────────

/// A registry hit: the resolved version, and whether it represents a library
/// already **installed on the host** (the local system registry) versus one that
/// must be **downloaded + built** (a network freight registry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryHit {
    pub version: String,
    /// `true` for the local system registry (already on the host → no build);
    /// `false` for a downloadable freight registry (freight builds it).
    pub installed: bool,
}

/// Looks a freight package name up across the configured registries. Abstracted
/// so the resolver is unit-testable without a network.
pub trait RegistryResolver {
    /// The hit for `freight_name` if it exists in any registry, else `None`.
    /// Implementations must treat an unreachable registry as `None` (best-effort:
    /// a missing registry never blocks resolution).
    fn lookup(&self, freight_name: &str) -> Option<RegistryHit>;
}

/// [`RegistryResolver`] over the project's registries. The local **system**
/// registry (locally-installed libraries) is consulted first and never times out,
/// so "rely less on external deps" prefers a package that's already on the host.
/// Network registries follow, best-effort: a "not found" (404) is a normal miss,
/// but the first transport error (offline, 5xx) flips them off so a resolve pass
/// doesn't hang retrying an unreachable registry for every name. The local
/// registry stays available even after the network goes dark.
pub struct ConfiguredRegistries {
    /// The local system registry (directory of stubs); always checked, never errors.
    system: Option<crate::registry::DirectoryRegistry>,
    /// Network registries, in priority order.
    repos: Vec<Box<dyn crate::registry::PackageRepo>>,
    reachable: std::cell::Cell<bool>,
}

impl ConfiguredRegistries {
    pub fn new(config: &crate::toolchain::cache::GlobalConfig) -> Self {
        Self {
            system: crate::registry::DirectoryRegistry::system(),
            repos: crate::registry::repos::registries_in_order(config),
            reachable: std::cell::Cell::new(true),
        }
    }
}

impl RegistryResolver for ConfiguredRegistries {
    fn lookup(&self, freight_name: &str) -> Option<RegistryHit> {
        use crate::registry::PackageRepo;
        // Local system registry first — already on the host, never times out.
        if let Some(sys) = &self.system {
            if let Ok(Some(info)) = sys.lookup(freight_name, None) {
                return Some(RegistryHit { version: info.latest, installed: true });
            }
        }
        if !self.reachable.get() {
            return None;
        }
        for repo in &self.repos {
            match repo.lookup(freight_name, None) {
                Ok(Some(info)) => {
                    return Some(RegistryHit { version: info.latest, installed: false })
                }
                Ok(None) => continue,
                Err(_) => {
                    self.reachable.set(false);
                    return None;
                }
            }
        }
        None
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetchcontent_git_tag_vs_rev_and_archive() {
        let deps = detect_fetchcontent_in(
            "FetchContent_Declare(\n\
             googletest\n  GIT_REPOSITORY https://github.com/google/googletest.git\n  GIT_TAG release-1.12.1\n)\n\
             FetchContent_Declare( fmt\n  GIT_REPOSITORY https://github.com/fmtlib/fmt.git\n  GIT_TAG e69e5f977d458f2650bb346dadf2ad30c5320281 )\n\
             FetchContent_Declare(json URL https://x/json.tar.xz URL_HASH SHA256=deadbeef)\n",
        );
        assert_eq!(deps.len(), 3);

        let gt = &deps[0];
        assert_eq!(gt.name, "googletest");
        assert!(gt.is_git);
        assert_eq!(gt.git_ref.as_deref(), Some("release-1.12.1"));
        assert!(!gt.ref_is_rev, "non-hex tag must stay a tag");

        let fmt = &deps[1];
        assert!(fmt.ref_is_rev, "40-hex GIT_TAG must be detected as a commit");

        let json = &deps[2];
        assert!(!json.is_git);
        assert_eq!(json.url, "https://x/json.tar.xz");
        assert_eq!(json.sha256.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn add_subdirectory_parses_literal_paths_skips_vars() {
        let paths = detect_add_subdirectory_in(
            "add_subdirectory(src)\n\
             add_subdirectory(third_party/foo)\n\
             add_subdirectory(\"third_party/bar\" EXCLUDE_FROM_ALL)\n\
             add_subdirectory(${SOME_VAR}/x)\n\
             add_subdirectory(src)\n",
        );
        assert_eq!(paths, vec!["src", "third_party/foo", "third_party/bar"]);
    }

    #[test]
    fn fetchcontent_without_source_is_skipped() {
        // A populate-only declare with no GIT_REPOSITORY/URL yields nothing.
        let deps = detect_fetchcontent_in("FetchContent_Declare(local SOURCE_DIR ../local)\n");
        assert!(deps.is_empty());
    }

    #[test]
    fn scan_dedups_and_drops_system() {
        let names = detect_cmake_packages_in(
            "find_package(ZLIB REQUIRED)\n\
             find_package(Threads)\n\
             find_package( fmt 9 CONFIG)\n\
             find_package(ZLIB)\n",
        );
        assert_eq!(names, vec!["ZLIB".to_string(), "fmt".to_string()]);
    }

    #[test]
    fn maps_cmake_names_to_freight() {
        assert_eq!(cmake_to_freight_name("ZLIB"), "zlib");
        assert_eq!(cmake_to_freight_name("PNG"), "libpng");
        assert_eq!(cmake_to_freight_name("fmt"), "fmt");
    }
}
