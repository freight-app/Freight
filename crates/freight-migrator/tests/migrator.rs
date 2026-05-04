//! Integration tests for `freight migrate` over fixture projects.
//!
//! Each fixture lives under `tests/migrator_fixtures/<format>/` and contains a
//! representative input file. We run the migrator end-to-end (parse → emit)
//! and assert against key facts in the emitted `freight.toml`. Full byte-exact
//! golden comparison is deliberately avoided so that cosmetic emit tweaks
//! don't force test churn.

use std::fs;
use std::path::{Path, PathBuf};

use freight_migrator::{self, Format};
use tempfile::tempdir;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("migrator_fixtures")
        .join(rel)
}

fn copy_fixture_into(dir: &Path, fixture_subdir: &str, filename: &str) {
    let src = fixture(&format!("{fixture_subdir}/{filename}"));
    let dst = dir.join(filename);
    fs::copy(&src, &dst).expect("copying fixture file");
}

#[test]
fn cmake_fixture_round_trips_through_emit() {
    let dir = tempdir().unwrap();
    copy_fixture_into(dir.path(), "cmake", "CMakeLists.txt");

    freight_migrator::run_migrate(dir.path(), Some(Format::Cmake), false, false).unwrap();

    let toml = fs::read_to_string(dir.path().join("freight.toml")).unwrap();

    assert!(toml.contains("name        = \"demo\""));
    assert!(toml.contains("version     = \"0.3.1\""));
    assert!(toml.contains("[language.cpp]"));
    assert!(toml.contains("std = \"c++20\""));
    assert!(toml.contains("[[bin]]"));
    assert!(toml.contains("name = \"demo\""));
    assert!(toml.contains("src  = \"src/main.cpp\""));
    assert!(toml.contains("m = { system = \"m\" }"));
    assert!(toml.contains("pthread = { system = \"pthread\" }"));
    // find_package(OpenSSL) adds openssl; target_link_libraries(OpenSSL::SSL) adds ssl
    assert!(toml.contains("openssl = { system = \"openssl\" }"));
    assert!(toml.contains("ssl = { system = \"ssl\" }"));
    assert!(toml.contains("paths = [\"include/\"]"));
    assert!(toml.contains("# FREIGHT: add_subdirectory(vendor/zlib)"));
}

#[test]
fn makefile_fixture_round_trips_through_emit() {
    let dir = tempdir().unwrap();
    copy_fixture_into(dir.path(), "make", "Makefile");

    freight_migrator::run_migrate(dir.path(), Some(Format::Makefile), false, false).unwrap();

    let toml = fs::read_to_string(dir.path().join("freight.toml")).unwrap();

    assert!(toml.contains("name        = "));
    assert!(toml.contains("[language.c]"));
    assert!(toml.contains("std = \"c17\""));
    assert!(toml.contains("[[bin]]"));
    assert!(toml.contains("name = \"demo\""));
    assert!(toml.contains("src  = \"src/main.c\""));
    assert!(toml.contains("m = { system = \"m\" }"));
    assert!(toml.contains("pthread = { system = \"pthread\" }"));
    assert!(toml.contains("USE_FOO"));
    assert!(toml.contains("paths = [\"include/\"]"));
}

#[test]
fn meson_fixture_round_trips_through_emit() {
    let dir = tempdir().unwrap();
    copy_fixture_into(dir.path(), "meson", "meson.build");

    freight_migrator::run_migrate(dir.path(), Some(Format::Meson), false, false).unwrap();

    let toml = fs::read_to_string(dir.path().join("freight.toml")).unwrap();

    assert!(toml.contains("name        = \"demo\""));
    assert!(toml.contains("version     = \"0.3.1\""));
    assert!(toml.contains("[language.cpp]"));
    assert!(toml.contains("std = \"c++20\""));
    assert!(toml.contains("src  = \"src/main.cpp\""));
    assert!(toml.contains("openssl = { system = \"openssl\" }"));
    assert!(toml.contains("paths = [\"include/\"]"));
}

#[test]
fn migrate_errors_when_manifest_exists() {
    let dir = tempdir().unwrap();
    copy_fixture_into(dir.path(), "cmake", "CMakeLists.txt");
    fs::write(dir.path().join("freight.toml"), "existing\n").unwrap();

    let result = freight_migrator::run_migrate(dir.path(), Some(Format::Cmake), false, false);
    assert!(result.is_err(), "should refuse without --force");
}

#[test]
fn migrate_with_force_overwrites() {
    let dir = tempdir().unwrap();
    copy_fixture_into(dir.path(), "cmake", "CMakeLists.txt");
    fs::write(dir.path().join("freight.toml"), "existing\n").unwrap();

    freight_migrator::run_migrate(dir.path(), Some(Format::Cmake), false, true).unwrap();
    let toml = fs::read_to_string(dir.path().join("freight.toml")).unwrap();
    assert!(toml.contains("[package]"));
    assert!(!toml.starts_with("existing"));
}

#[test]
fn auto_detection_picks_cmake_when_both_present() {
    let dir = tempdir().unwrap();
    copy_fixture_into(dir.path(), "cmake", "CMakeLists.txt");
    copy_fixture_into(dir.path(), "make", "Makefile");

    freight_migrator::run_migrate(dir.path(), None, false, false).unwrap();
    let toml = fs::read_to_string(dir.path().join("freight.toml")).unwrap();
    // Cmake fixture project is C++; Make fixture is C. If cmake won, we
    // should see the C++ language block.
    assert!(toml.contains("[language.cpp]"));
}
