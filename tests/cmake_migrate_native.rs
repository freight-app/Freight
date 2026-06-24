//! End-to-end: `freight init --migrate --native` extracts a single-library CMake
//! project's build data via the File API and writes a freight-native manifest that
//! then builds with no hand-editing.

use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn native_migration_of_single_library_cmake_project() {
    if !have("cmake") || !(have("cc") || have("gcc") || have("clang")) {
        eprintln!("skipping: cmake or C compiler missing");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("mylib");

    // A single-library CMake project, plus a test executable under test/ that must
    // be ignored by the migration.
    write(
        &proj.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\n\
         project(mylib C)\n\
         add_library(mylib STATIC src/a.c src/b.c)\n\
         target_include_directories(mylib PUBLIC include)\n\
         target_compile_definitions(mylib PRIVATE MYLIB_BUILD)\n\
         add_subdirectory(test)\n",
    );
    write(&proj.join("include/mylib.h"), "int mylib_a(void);\nint mylib_b(void);\n");
    write(&proj.join("src/a.c"), "#include <mylib.h>\nint mylib_a(void){return 1;}\n");
    write(&proj.join("src/b.c"), "#include <mylib.h>\nint mylib_b(void){return 2;}\n");
    // A test executable in a test/ subdir — must NOT end up in the manifest.
    write(
        &proj.join("test/CMakeLists.txt"),
        "add_executable(t t.c)\ntarget_link_libraries(t mylib)\n",
    );
    write(&proj.join("test/t.c"), "int main(void){return 0;}\n");

    let init = Command::new(env!("CARGO_BIN_EXE_freight"))
        .args(["init", "--migrate", "--native"])
        .current_dir(&proj)
        .output()
        .expect("run freight init --migrate --native");
    assert!(
        init.status.success(),
        "init --migrate --native failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr),
    );

    let manifest = fs::read_to_string(proj.join("freight.toml")).unwrap();
    // Native (not a foreign self-build) and authoritative sources.
    assert!(!manifest.contains("build = \"cmake\""), "should be native, got:\n{manifest}");
    assert!(manifest.contains("[lib]"), "{manifest}");
    assert!(manifest.contains("auto-discover = false"), "{manifest}");
    assert!(manifest.contains("src/a.c"), "{manifest}");
    assert!(manifest.contains("src/b.c"), "{manifest}");
    assert!(manifest.contains("includes = [\"include\"]"), "{manifest}");
    // The test executable must not have leaked in.
    assert!(!manifest.contains("[[bin]]"), "test exe should be ignored:\n{manifest}");

    // The generated native manifest builds.
    let build = Command::new(env!("CARGO_BIN_EXE_freight"))
        .arg("build")
        .current_dir(&proj)
        .output()
        .expect("run freight build");
    assert!(
        build.status.success(),
        "native-migrated project should build.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
}

/// A library plus an executable that links it migrates to a single freight package
/// with `[lib]` + `[[bin]]`, which builds and runs.
#[test]
fn native_migration_of_library_plus_executable() {
    if !have("cmake") || !(have("cc") || have("gcc") || have("clang")) {
        eprintln!("skipping: cmake or C compiler missing");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("greetapp");

    write(
        &proj.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\n\
         project(greetapp C)\n\
         add_library(greet STATIC src/greet.c)\n\
         target_include_directories(greet PUBLIC include)\n\
         add_executable(app app/main.c)\n\
         target_link_libraries(app greet)\n\
         target_include_directories(app PRIVATE include)\n",
    );
    write(&proj.join("include/greet.h"), "int greet(void);\n");
    write(&proj.join("src/greet.c"), "#include <greet.h>\nint greet(void){return 42;}\n");
    write(
        &proj.join("app/main.c"),
        "#include <greet.h>\nint main(void){return greet()==42?0:1;}\n",
    );

    let init = Command::new(env!("CARGO_BIN_EXE_freight"))
        .args(["init", "--migrate", "--native"])
        .current_dir(&proj)
        .output()
        .expect("run init");
    assert!(init.status.success());

    let manifest = fs::read_to_string(proj.join("freight.toml")).unwrap();
    assert!(manifest.contains("[lib]"), "{manifest}");
    assert!(manifest.contains("srcs = [\"src/greet.c\"]"), "{manifest}");
    assert!(manifest.contains("[[bin]]"), "{manifest}");
    assert!(manifest.contains("src  = \"app/main.c\""), "{manifest}");

    let build = Command::new(env!("CARGO_BIN_EXE_freight"))
        .arg("build")
        .current_dir(&proj)
        .output()
        .expect("run build");
    assert!(
        build.status.success(),
        "lib + bin migration should build.\nstderr: {}",
        String::from_utf8_lossy(&build.stderr),
    );
}
