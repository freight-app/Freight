//! Exercises the shipped `plugins/autotools` reference plugin: an
//! `external = true` dependency with a `./configure` script is built out-of-source
//! by the plugin (`configure && make && make install`) and wired in. Uses a
//! hand-written `configure` (no autoconf needed), which also exercises the
//! `run(tool, args, cwd)` working-directory variant.

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
fn autotools_plugin_builds_and_links_an_external_dep() {
    if !make_available() {
        eprintln!("skipping autotools plugin test: make not installed");
        return;
    }

    let plugin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/autotools");
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");

    // A configure-based library, vendored inside the project. `configure` writes
    // an out-of-source Makefile honouring --prefix (recipe tabs added at runtime).
    let mylib = app.join("vendor/mylib");
    write(
        &mylib.join("configure"),
        r#"#!/bin/sh
prefix=/usr/local
for arg in "$@"; do
  case "$arg" in --prefix=*) prefix="${arg#--prefix=}" ;; esac
done
srcdir=`dirname "$0"`
tab=`printf '\t'`
{
  echo "all:"
  echo "${tab}cc -c -I${srcdir}/include ${srcdir}/src/mylib.c -o mylib.o"
  echo "${tab}ar rcs libmylib.a mylib.o"
  echo "install:"
  echo "${tab}mkdir -p ${prefix}/include ${prefix}/lib"
  echo "${tab}cp ${srcdir}/include/mylib.h ${prefix}/include/"
  echo "${tab}cp libmylib.a ${prefix}/lib/"
} > Makefile
"#,
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
             autotools-builder = {{ path = \"{}\" }}\n\
             mylib = {{ path = \"vendor/mylib\", external = true }}\n\n\
             [autotools]\nbuild = \"mylib\"\n",
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
        eprintln!("skipping autotools plugin test: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight run with autotools plugin");
    common::assert_output_contains(&out, &["answer=42"]);
}
