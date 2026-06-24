//! End-to-end for the on-demand CMake dependency provider: building a foreign
//! CMake project whose `find_package(<dep>)` is intercepted by the injected
//! `Freight.cmake` provider, which calls `freight cmake-provide <dep>` to build +
//! provide a dep from `.pkgs/`, so the parent's `find_package` resolves it.
//!
//! No separate resolver executable, no resolution report — the cmake script calls
//! freight directly during configure.

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

/// `freight cmake-provide <name>` builds a `.pkgs/` CMake dep and prints its
/// install prefix (containing the project's own `<Name>Config.cmake`).
#[test]
fn cmake_provide_builds_and_prints_prefix() {
    if !have("cmake") || !(have("cc") || have("gcc") || have("clang")) {
        eprintln!("skipping: cmake or C compiler missing");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    write(
        &app.join("freight.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nbuild = \"cmake\"\n",
    );
    let dep = app.join(".pkgs/jsonlike");
    write(
        &dep.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(jsonlike C)\n\
         add_library(jsonlike STATIC src.c)\n\
         target_include_directories(jsonlike PUBLIC $<INSTALL_INTERFACE:include>)\n\
         install(TARGETS jsonlike EXPORT t ARCHIVE DESTINATION lib)\n\
         install(EXPORT t FILE jsonlikeConfig.cmake DESTINATION lib/cmake/jsonlike)\n\
         install(FILES jl.h DESTINATION include)\n",
    );
    write(&dep.join("jl.h"), "int jl(void);\n");
    write(&dep.join("src.c"), "int jl(void){return 1;}\n");

    let out = Command::new(env!("CARGO_BIN_EXE_freight"))
        .args(["cmake-provide", "jsonlike"])
        .current_dir(&app)
        .output()
        .expect("run freight cmake-provide");
    let prefix = String::from_utf8_lossy(&out.stdout);
    let prefix = prefix.trim();
    assert!(
        !prefix.is_empty(),
        "cmake-provide should print a prefix.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        Path::new(prefix)
            .join("lib/cmake/jsonlike/jsonlikeConfig.cmake")
            .is_file(),
        "install prefix {prefix} should contain the dep's Config.cmake",
    );
}

/// Full flow: a foreign CMake project's `find_package(jsonlike)` is satisfied by
/// the provider calling back into freight — the parent configures + builds.
#[test]
fn provider_satisfies_find_package_during_build() {
    if !have("cmake") || !(have("cc") || have("gcc") || have("clang")) {
        eprintln!("skipping: cmake or C compiler missing");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");

    // A foreign CMake app that find_package()s jsonlike and links it.
    write(
        &app.join("freight.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nbuild = \"cmake\"\n",
    );
    write(
        &app.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.24)\nproject(app C)\n\
         find_package(jsonlike CONFIG REQUIRED)\n\
         add_executable(app main.c)\n\
         target_link_libraries(app jsonlike)\n\
         install(TARGETS app RUNTIME DESTINATION bin)\n",
    );
    write(
        &app.join("main.c"),
        "int jl(void);\nint main(void){return jl()==1?0:1;}\n",
    );

    // The dep, already fetched under .pkgs (as if by `freight add`).
    let dep = app.join(".pkgs/jsonlike");
    write(
        &dep.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(jsonlike C)\n\
         add_library(jsonlike STATIC src.c)\n\
         target_include_directories(jsonlike PUBLIC $<INSTALL_INTERFACE:include>)\n\
         install(TARGETS jsonlike EXPORT t ARCHIVE DESTINATION lib)\n\
         install(EXPORT t FILE jsonlikeConfig.cmake DESTINATION lib/cmake/jsonlike)\n\
         install(FILES jl.h DESTINATION include)\n",
    );
    write(&dep.join("jl.h"), "int jl(void);\n");
    write(&dep.join("src.c"), "int jl(void){return 1;}\n");

    let out = Command::new(env!("CARGO_BIN_EXE_freight"))
        .arg("build")
        .current_dir(&app)
        .output()
        .expect("run freight build");
    assert!(
        out.status.success(),
        "build should succeed via the provider.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
