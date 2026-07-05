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

/// `FetchContent_MakeAvailable` is satisfied by the provider when freight knows
/// the dep: the declared GIT_REPOSITORY is a dead host, so a successful build
/// proves no download happened — freight's installed copy was provided and
/// `FetchContent_SetPopulated` short-circuited population. A second content
/// (`vendored`) that freight can NOT provide must fall through to FetchContent's
/// normal population (a local SOURCE_DIR add_subdirectory).
#[test]
fn provider_satisfies_fetchcontent_and_falls_back_when_unknown() {
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
    write(
        &app.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.24)\nproject(app C)\n\
         include(FetchContent)\n\
         FetchContent_Declare(jsonlike GIT_REPOSITORY https://invalid.invalid/jsonlike.git GIT_TAG v1)\n\
         FetchContent_Declare(vendored SOURCE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/third_party/vendored)\n\
         FetchContent_MakeAvailable(jsonlike vendored)\n\
         add_executable(app main.c)\n\
         target_link_libraries(app jsonlike vendored)\n\
         install(TARGETS app RUNTIME DESTINATION bin)\n",
    );
    write(
        &app.join("main.c"),
        "int jl(void);\nint vend(void);\nint main(void){return jl()+vend()==3?0:1;}\n",
    );

    // The freight-known dep, fetched under .pkgs (exports jsonlikeConfig.cmake).
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

    // The unknown dep: a local vendored tree FetchContent must add_subdirectory.
    let vendored = app.join("third_party/vendored");
    write(
        &vendored.join("CMakeLists.txt"),
        "add_library(vendored STATIC vend.c)\n\
         target_include_directories(vendored PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})\n",
    );
    write(&vendored.join("vend.c"), "int vend(void){return 2;}\n");

    let out = Command::new(env!("CARGO_BIN_EXE_freight"))
        .arg("build")
        .current_dir(&app)
        .output()
        .expect("run freight build");
    assert!(
        out.status.success(),
        "FetchContent should be provided by freight (jsonlike) and fall back \
         locally (vendored).\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // The provider records what it satisfied in the per-build report file.
    let report = find_file(&app, "freight-report.txt")
        .expect("the cmake plugin should write freight-report.txt");
    let report = fs::read_to_string(report).unwrap();
    assert!(
        report.contains("fetchcontent-provided jsonlike"),
        "provider should record satisfying jsonlike via freight:\n{report}",
    );
}

/// First file named `name` anywhere under `dir`.
fn find_file(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|f| f == name) {
            return Some(path);
        }
    }
    None
}

/// The provider resolves a freight-native `{ path = "..." }` dependency — not just
/// deps fetched into `.pkgs/`. A foreign CMake app `find_package`s a sibling freight
/// library declared by path; the provider builds it and exports its `Config.cmake`.
#[test]
fn provider_resolves_native_path_dependency() {
    if !have("cmake") || !(have("c++") || have("g++") || have("clang++")) {
        eprintln!("skipping: cmake or C++ compiler missing");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();

    // A sibling freight-native library.
    let greet = tmp.path().join("greet");
    write(
        &greet.join("freight.toml"),
        "[package]\nname = \"greet\"\nversion = \"0.1.0\"\n\n[lib]\nname = \"greet\"\n",
    );
    write(
        &greet.join("include/greet.h"),
        "const char* greeting(void);\n",
    );
    write(
        &greet.join("src/greet.cpp"),
        "#include \"greet.h\"\nconst char* greeting(void){return \"hi\";}\n",
    );

    // A foreign CMake app that find_package()s greet via a path dependency.
    let app = tmp.path().join("greetapp");
    write(
        &app.join("freight.toml"),
        "[package]\nname = \"greetapp\"\nversion = \"0.1.0\"\nbuild = \"cmake\"\n\n\
         [dependencies]\ngreet = { path = \"../greet\" }\n",
    );
    write(
        &app.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.24)\nproject(greetapp CXX)\n\
         find_package(greet CONFIG REQUIRED)\n\
         add_executable(greetapp main.cpp)\n\
         target_link_libraries(greetapp greet::greet)\n\
         install(TARGETS greetapp RUNTIME DESTINATION bin)\n",
    );
    write(
        &app.join("main.cpp"),
        "#include \"greet.h\"\nint main(){return greeting()?0:1;}\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_freight"))
        .arg("build")
        .current_dir(&app)
        .output()
        .expect("run freight build");
    assert!(
        out.status.success(),
        "find_package on a native path dep should resolve via the provider.\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        app.join("target/cmake-export/greet/lib/cmake/greet/greetConfig.cmake")
            .is_file(),
        "the path dep should have been exported as a Config.cmake package",
    );
}
