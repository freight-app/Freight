//! Decide which build system is in use for a directory.

use std::path::Path;

use crate::Format;

/// Return the detected source build system for `project_dir`.
///
/// Priority order when multiple are present: CMake ≻ Meson ≻ Makefile.
/// CMake wins because it is usually the canonical build in mixed projects;
/// a bare `Makefile` is most often a thin wrapper and least informative.
pub fn detect_format(project_dir: &Path) -> Option<Format> {
    if project_dir.join("CMakeLists.txt").is_file() {
        return Some(Format::Cmake);
    }
    if project_dir.join("meson.build").is_file() {
        return Some(Format::Meson);
    }
    if project_dir.join("Makefile").is_file() || project_dir.join("GNUmakefile").is_file() {
        return Some(Format::Makefile);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detects_cmake() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("CMakeLists.txt"), "").unwrap();
        assert_eq!(detect_format(dir.path()), Some(Format::Cmake));
    }

    #[test]
    fn detects_meson() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("meson.build"), "").unwrap();
        assert_eq!(detect_format(dir.path()), Some(Format::Meson));
    }

    #[test]
    fn detects_makefile() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Makefile"), "").unwrap();
        assert_eq!(detect_format(dir.path()), Some(Format::Makefile));
    }

    #[test]
    fn detects_gnumakefile() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("GNUmakefile"), "").unwrap();
        assert_eq!(detect_format(dir.path()), Some(Format::Makefile));
    }

    #[test]
    fn cmake_wins_over_makefile() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("CMakeLists.txt"), "").unwrap();
        fs::write(dir.path().join("Makefile"), "").unwrap();
        assert_eq!(detect_format(dir.path()), Some(Format::Cmake));
    }

    #[test]
    fn nothing_detected_in_empty_dir() {
        let dir = tempdir().unwrap();
        assert_eq!(detect_format(dir.path()), None);
    }
}
