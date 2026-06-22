//! A package that *is* a foreign (CMake) project — `[package] build = "cmake"`
//! with no native targets — is built by freight via the bundled `cmake` plugin
//! (the `build_foreign_self` path, now routed through `run_build_system`).

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

fn cmake_available() -> bool {
    Command::new("cmake")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn foreign_self_cmake_package_builds_via_plugin() {
    if !cmake_available() {
        eprintln!("skipping foreign_self test: cmake not installed");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("mathlib");
    write(
        &proj.join("freight.toml"),
        "[package]\nname = \"mathlib\"\nversion = \"0.1.0\"\nbuild = \"cmake\"\n",
    );
    write(
        &proj.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(mathlib C)\n\
         add_library(mathlib STATIC src/mathlib.c)\n\
         target_include_directories(mathlib PUBLIC include)\n\
         install(TARGETS mathlib ARCHIVE DESTINATION lib)\n\
         install(DIRECTORY include/ DESTINATION include)\n",
    );
    write(&proj.join("include/mathlib.h"), "int add(int, int);\n");
    write(
        &proj.join("src/mathlib.c"),
        "#include \"mathlib.h\"\nint add(int a, int b){return a+b;}\n",
    );

    let out = common::freight(&proj, &["build"]);
    if common::missing_toolchain(&out) {
        eprintln!("skipping foreign_self test: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight build on a foreign cmake package");
    assert!(
        proj.join("target/debug/libmathlib.a").exists(),
        "built library should be placed in target/debug/"
    );
}
