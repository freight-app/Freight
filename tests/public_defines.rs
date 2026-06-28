//! End-to-end for exported (public/interface) compile defines: a library declares
//! defines via `[lib].defines` (always-on) or a `pub-define:` feature entry, and
//! they apply both to the library's own build AND to every dependent's build — so a
//! consumer compiles in the same configuration without restating the define.

use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

fn have_cc() -> bool {
    ["cc", "gcc", "clang"].iter().any(|t| {
        Command::new(t)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// A public header gated on a define: without the define a `#error` fires, so the
/// build only succeeds if the define reaches both the lib's and the consumer's
/// compilation.
const GATED_HEADER: &str =
    "#ifndef GREET_PUBLIC\n#error \"GREET_PUBLIC not defined\"\n#endif\nint greet(void);\n";
const GREET_SRC: &str = "#include \"greet.h\"\nint greet(void){return 7;}\n";
const CONSUMER_MAIN: &str = "#include \"greet.h\"\nint main(void){return greet()==7?0:1;}\n";

fn greet_lib(dir: &Path, lib_toml: &str) {
    write(&dir.join("freight.toml"), lib_toml);
    write(&dir.join("include/greet.h"), GATED_HEADER);
    write(&dir.join("src/greet.c"), GREET_SRC);
}

fn consumer(dir: &Path, dep_rel: &str) {
    write(
        &dir.join("freight.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [[bin]]\nname = \"app\"\nsrc = \"src/main.c\"\n\n\
             [dependencies]\ngreet = {{ path = \"{dep_rel}\" }}\n"
        ),
    );
    write(&dir.join("src/main.c"), CONSUMER_MAIN);
}

fn build(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_freight"))
        .arg("build")
        .current_dir(dir)
        .output()
        .expect("run freight build")
}

/// `[lib].defines` is applied to the lib's own build and propagated to the consumer,
/// which compiles cleanly without restating the define.
#[test]
fn lib_defines_propagate_to_consumer() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    greet_lib(
        &tmp.path().join("greet"),
        "[package]\nname = \"greet\"\nversion = \"0.1.0\"\n\n\
         [lib]\ntype = \"static\"\ndefines = [\"GREET_PUBLIC\"]\n",
    );
    let app = tmp.path().join("app");
    consumer(&app, "../greet");

    let out = build(&app);
    assert!(
        out.status.success(),
        "consumer should build via the exported [lib].defines.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Without exporting the define, the consumer's compilation trips the header's
/// `#error` — proving the propagation is what makes the clean case work.
#[test]
fn without_export_consumer_fails() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // Lib keeps the define PRIVATE via [compiler].defines (own build only).
    greet_lib(
        &tmp.path().join("greet"),
        "[package]\nname = \"greet\"\nversion = \"0.1.0\"\n\n\
         [lib]\ntype = \"static\"\n\n\
         [compiler]\ndefines = [\"GREET_PUBLIC\"]\n",
    );
    let app = tmp.path().join("app");
    consumer(&app, "../greet");

    let out = build(&app);
    assert!(
        !out.status.success(),
        "consumer must fail: a private [compiler].defines is not exported.\nstdout: {}",
        String::from_utf8_lossy(&out.stdout),
    );
}

/// A `pub-define:` feature entry exports its define when the feature is active
/// (here via `default`), reaching the consumer with no define restated.
#[test]
fn pub_define_feature_propagates_to_consumer() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    greet_lib(
        &tmp.path().join("greet"),
        "[package]\nname = \"greet\"\nversion = \"0.1.0\"\n\n\
         [lib]\ntype = \"static\"\n\n\
         [features]\ndefault = [\"pub\"]\npub = [\"pub-define:GREET_PUBLIC\"]\n",
    );
    let app = tmp.path().join("app");
    consumer(&app, "../greet");

    let out = build(&app);
    assert!(
        out.status.success(),
        "consumer should build via the exported pub-define: feature.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
}
