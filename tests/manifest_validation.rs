/// Integration tests: `freight check` / `freight build` on manifests with
/// deliberate structural problems must emit the right diagnostics.
///
/// These tests write temporary freight.toml files to temp dirs so they don't
/// touch the examples tree.
mod common;
use common::*;

use std::fs;
use tempfile::TempDir;

fn scratch(toml: &str) -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("freight.toml"), toml).unwrap();
    fs::create_dir(tmp.path().join("src")).unwrap();
    // Provide a stub main so the compiler doesn't complain about missing sources
    // before the manifest validator gets a chance to fire.
    fs::write(
        tmp.path().join("src/main.c"),
        "int main(void) { return 0; }\n",
    )
    .unwrap();
    tmp
}

// ── Missing required fields ───────────────────────────────────────────────────

#[test]
fn missing_package_name_is_rejected() {
    let tmp = scratch(
        r#"
[package]
version = "0.1.0"

[language.c]
std = "c11"

[[bin]]
name = "app"
src  = "src/main.c"
"#,
    );
    let out = freight(tmp.path(), &["build"]);
    assert_failure(&out, "missing package.name should be rejected");
    assert_output_contains(&out, &["name"]);
}

// ── Invalid values ────────────────────────────────────────────────────────────

#[test]
fn invalid_opt_level_is_rejected() {
    let tmp = scratch(
        r#"
[package]
name    = "bad-opt"
version = "0.1.0"

[language.c]
std = "c11"

[[bin]]
name = "app"
src  = "src/main.c"

[compiler]
opt-level = 99
"#,
    );
    let out = freight(tmp.path(), &["build"]);
    assert_failure(&out, "opt-level 99 should be rejected");
    assert_output_contains(&out, &["opt-level"]);
}

#[test]
fn unknown_language_key_is_rejected() {
    let tmp = scratch(
        r#"
[package]
name    = "bad-lang"
version = "0.1.0"

[language.brainfuck]

[[bin]]
name = "app"
src  = "src/main.c"
"#,
    );
    let out = freight(tmp.path(), &["build"]);
    assert_failure(&out, "unknown language key should be rejected");
}

// ── Feature cycles ────────────────────────────────────────────────────────────

#[test]
fn feature_cycle_is_rejected() {
    let tmp = scratch(
        r#"
[package]
name    = "cycle"
version = "0.1.0"

[language.c]
std = "c11"

[[bin]]
name = "app"
src  = "src/main.c"

[features]
a = ["b"]
b = ["a"]
"#,
    );
    let out = freight(tmp.path(), &["build"]);
    assert_failure(&out, "feature cycle should be rejected");
    assert_output_contains(&out, &["cycle"]);
}
