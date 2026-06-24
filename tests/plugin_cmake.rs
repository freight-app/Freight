//! Exercises the shipped `plugins/cmake` reference plugin end-to-end: an
//! `external = true` dependency that is a CMake project is built by the plugin
//! (via `[cmake] build = "..."`), and its headers + static library are wired
//! into the consuming build automatically.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
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
fn cmake_plugin_builds_and_links_an_external_dep() {
    if !cmake_available() {
        eprintln!("skipping cmake plugin test: cmake not installed");
        return;
    }

    let plugin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/cmake");

    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");

    // ── A tiny CMake library, vendored inside the project ────────────────────
    // (Real external deps land in `.pkgs/`; both live inside the project, which
    // is what the plugin sandbox requires.)
    let mylib = app.join("vendor/mylib");
    write(
        &mylib.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\n\
         project(mylib C)\n\
         add_library(mylib STATIC src/mylib.c)\n\
         target_include_directories(mylib PUBLIC include)\n\
         install(TARGETS mylib ARCHIVE DESTINATION lib)\n\
         install(FILES include/mylib.h DESTINATION include)\n",
    );
    write(&mylib.join("include/mylib.h"), "int mylib_answer(void);\n");
    write(
        &mylib.join("src/mylib.c"),
        "#include \"mylib.h\"\nint mylib_answer(void) { return 42; }\n",
    );

    // ── The consuming app ────────────────────────────────────────────────────
    write(
        &app.join("freight.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [[bin]]\nname = \"app\"\nsrc = \"src/main.c\"\n\n\
             [build-dependencies]\n\
             cmake = {{ path = \"{}\" }}\n\n\
             [dependencies]\n\
             mylib = {{ path = \"vendor/mylib\", external = true }}\n\n\
             [cmake]\nbuild = \"mylib\"\n",
            plugin.display()
        ),
    );
    write(
        &app.join("src/main.c"),
        "#include <mylib.h>\n#include <stdio.h>\n\
         int main(void) { printf(\"answer=%d\\n\", mylib_answer()); \
         return mylib_answer() == 42 ? 0 : 1; }\n",
    );

    let out = common::freight(&app, &["run"]);
    if common::missing_toolchain(&out) {
        eprintln!("skipping cmake plugin test: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight run with cmake plugin");
    common::assert_output_contains(&out, &["answer=42"]);
}

/// Two foreign deps in one `[cmake] build = ["liba", "libb"]` where `libb`'s
/// CMakeLists does `find_package(liba)`. `liba` (header-only, exports a CMake
/// config) is built first; its install prefix is threaded into `libb`'s configure
/// via `FREIGHT_PREFIXES`, so the `find_package` resolves freight's copy. If the
/// prefix orchestration regressed, libb's configure would fail to find liba.
#[test]
fn cmake_plugin_resolves_an_earlier_dep_via_find_package() {
    if !cmake_available() {
        eprintln!("skipping cmake find_package test: cmake not installed");
        return;
    }

    let plugin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/cmake");
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");

    // ── liba: header-only INTERFACE lib that exports a CMake package config ────
    let liba = app.join("vendor/liba");
    write(
        &liba.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\n\
         project(liba C)\n\
         add_library(liba INTERFACE)\n\
         target_include_directories(liba INTERFACE \
           $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<INSTALL_INTERFACE:include>)\n\
         install(TARGETS liba EXPORT libaTargets)\n\
         install(FILES include/liba.h DESTINATION include)\n\
         install(EXPORT libaTargets FILE libaConfig.cmake NAMESPACE liba:: \
           DESTINATION lib/cmake/liba)\n",
    );
    write(
        &liba.join("include/liba.h"),
        "static inline int liba_answer(void) { return 42; }\n",
    );

    // ── libb: static lib that find_package(liba) and uses its header ──────────
    let libb = app.join("vendor/libb");
    write(
        &libb.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\n\
         project(libb C)\n\
         find_package(liba REQUIRED)\n\
         add_library(libb STATIC src/libb.c)\n\
         target_include_directories(libb PUBLIC \
           $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<INSTALL_INTERFACE:include>)\n\
         target_link_libraries(libb PUBLIC liba::liba)\n\
         install(TARGETS libb ARCHIVE DESTINATION lib)\n\
         install(FILES include/libb.h DESTINATION include)\n",
    );
    write(&libb.join("include/libb.h"), "int libb_answer(void);\n");
    write(
        &libb.join("src/libb.c"),
        "#include \"libb.h\"\n#include \"liba.h\"\n\
         int libb_answer(void) { return liba_answer(); }\n",
    );

    // ── The consuming app — links libb only (liba is header-only) ─────────────
    write(
        &app.join("freight.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [[bin]]\nname = \"app\"\nsrc = \"src/main.c\"\n\n\
             [build-dependencies]\n\
             cmake = {{ path = \"{}\" }}\n\n\
             [dependencies]\n\
             liba = {{ path = \"vendor/liba\", external = true }}\n\
             libb = {{ path = \"vendor/libb\", external = true }}\n\n\
             [cmake]\nbuild = [\"liba\", \"libb\"]\n",
            plugin.display()
        ),
    );
    write(
        &app.join("src/main.c"),
        "#include <libb.h>\n#include <stdio.h>\n\
         int main(void) { printf(\"answer=%d\\n\", libb_answer()); \
         return libb_answer() == 42 ? 0 : 1; }\n",
    );

    let out = common::freight(&app, &["run"]);
    if common::missing_toolchain(&out) {
        eprintln!("skipping cmake find_package test: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight run with transitive cmake find_package");
    common::assert_output_contains(&out, &["answer=42"]);
}
