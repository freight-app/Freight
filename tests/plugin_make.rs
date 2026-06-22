//! Exercises the shipped `plugins/make` reference plugin: a plain-Makefile
//! `external = true` dependency is built in-tree by the plugin and its header +
//! static library are wired into the consuming build.

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

fn make_available() -> bool {
    Command::new("make")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn make_plugin_builds_and_links_an_external_dep() {
    if !make_available() {
        eprintln!("skipping make plugin test: make not installed");
        return;
    }

    let plugin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/make");
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");

    // A Makefile-built library, vendored inside the project.
    let mylib = app.join("vendor/mylib");
    write(
        &mylib.join("Makefile"),
        "all: libmylib.a\n\n\
         libmylib.a:\n\tcc -c -Iinclude src/mylib.c -o mylib.o\n\tar rcs libmylib.a mylib.o\n",
    );
    write(&mylib.join("include/mylib.h"), "int mylib_answer(void);\n");
    write(
        &mylib.join("src/mylib.c"),
        "#include \"mylib.h\"\nint mylib_answer(void) { return 42; }\n",
    );

    write(
        &app.join("freight.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [[bin]]\nname = \"app\"\nsrc = \"src/main.c\"\n\n\
             [dependencies]\n\
             make = {{ path = \"{}\" }}\n\
             mylib = {{ path = \"vendor/mylib\", external = true }}\n\n\
             [make]\nbuild = \"mylib\"\n",
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
        eprintln!("skipping make plugin test: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight run with make plugin");
    common::assert_output_contains(&out, &["answer=42"]);
}
