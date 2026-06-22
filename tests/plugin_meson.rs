//! Exercises the shipped `plugins/meson` reference plugin: a Meson-built
//! `external = true` dependency is configured/built/installed by the plugin and
//! its header + static library are wired into the consuming build.

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

fn meson_available() -> bool {
    Command::new("meson")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn meson_plugin_builds_and_links_an_external_dep() {
    if !meson_available() {
        eprintln!("skipping meson plugin test: meson not installed");
        return;
    }

    let plugin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/meson");
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");

    // A Meson-built library, vendored inside the project.
    let mylib = app.join("vendor/mylib");
    write(
        &mylib.join("meson.build"),
        "project('mylib', 'c')\n\
         inc = include_directories('include')\n\
         static_library('mylib', 'src/mylib.c', include_directories: inc, install: true)\n\
         install_headers('include/mylib.h')\n",
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
             meson = {{ path = \"{}\" }}\n\
             mylib = {{ path = \"vendor/mylib\", external = true }}\n\n\
             [meson]\nbuild = \"mylib\"\n",
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
        eprintln!("skipping meson plugin test: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight run with meson plugin");
    common::assert_output_contains(&out, &["answer=42"]);
}
