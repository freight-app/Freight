//! CMake dependency name helpers used by the build path.
//!
//! - Classify a `find_package` name as a pure toolchain/system facility
//!   ([`is_system_pkg`]) or an already-installed CMake config package
//!   ([`is_installed_cmake_package`]).
//! - Map a CMake `find_package` name to its freight / pkg-config name
//!   ([`cmake_to_freight_name`]).
//!
//! Build-time *resolution* (deciding what to build for a CMake project) is done
//! dynamically by the cmake plugin's CMake dependency provider, which calls
//! `freight cmake-provide <name>` on demand — see `plugins/cmake/cmake.freight`
//! and `build::pipeline::provide_cmake_package`.

use std::path::PathBuf;

/// CMake `find_package` names that are pure toolchain/system facilities. These
/// are wired through `[os.*]`/`[arch.*]` features or the compiler, never as
/// freight package dependencies, so they terminate resolution.
pub const CMAKE_SYSTEM_PKGS: &[&str] = &[
    "threads",
    "openmp",
    "mpi",
    "openacc",
    "cudatoolkit",
    "opengl",
    "opengles2",
    "glut",
    "x11",
    "doxygen",
    "git",
    "python",
    "python2",
    "python3",
    "pythonlibs",
    "pythoninterp",
    "pkgconfig",
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
    let wanted = [
        format!("{lower}config.cmake"),
        format!("{lower}-config.cmake"),
    ];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_cmake_names_to_freight() {
        assert_eq!(cmake_to_freight_name("ZLIB"), "zlib");
        assert_eq!(cmake_to_freight_name("PNG"), "libpng");
        assert_eq!(cmake_to_freight_name("fmt"), "fmt");
    }

    #[test]
    fn system_pkgs_are_recognised() {
        assert!(is_system_pkg("Threads"));
        assert!(is_system_pkg("OpenMP"));
        assert!(!is_system_pkg("zlib"));
    }
}
