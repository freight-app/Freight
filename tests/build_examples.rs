/// Integration tests: `freight build` on well-formed examples must succeed
/// and produce the expected binary.
mod common;
use common::*;

// ── C ─────────────────────────────────────────────────────────────────────────

#[test]
fn c_hello_builds() {
    let dir = example(&["c", "hello"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "c/hello build");

    let run = run_binary(&dir, "c-simple", &[]);
    assert_success(&run, "c/hello run");
}

// ── C++ ───────────────────────────────────────────────────────────────────────

#[test]
fn cpp_hello_builds() {
    let dir = example(&["cpp", "hello"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "cpp/hello build");

    let run = run_binary(&dir, "hello-cpp", &[]);
    assert_success(&run, "cpp/hello run");
}

#[test]
fn cpp_hello_release_builds() {
    let dir = example(&["cpp", "hello"]);
    let out = freight(&dir, &["build", "--release"]);
    assert_success(&out, "cpp/hello release build");
}

#[test]
fn cpp_modules_builds() {
    let dir = example(&["cpp", "modules"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "cpp/modules build");
}

#[test]
fn cpp_static_lib_builds() {
    let dir = example(&["cpp", "static-lib"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "cpp/static-lib build");

    let run = run_binary(&dir, "demo", &[]);
    assert_success(&run, "cpp/static-lib run");
}

#[test]
fn cpp_multi_bin_builds() {
    let dir = example(&["cpp", "multi-bin"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "cpp/multi-bin build");
}

// ── Fortran ───────────────────────────────────────────────────────────────────

#[test]
fn fortran_hello_builds() {
    let dir = example(&["fortran", "hello"]);
    let out = freight(&dir, &["build"]);
    if missing_toolchain(&out) {
        eprintln!("skipping fortran/hello: no Fortran compiler installed");
        return;
    }
    assert_success(&out, "fortran/hello build");

    let run = run_binary(&dir, "fortran-hello", &[]);
    assert_success(&run, "fortran/hello run");
}

// ── Assembly ──────────────────────────────────────────────────────────────────

#[test]
fn assembly_hello_builds() {
    let dir = example(&["assembly", "hello"]);
    let out = freight(&dir, &["build"]);
    if missing_toolchain(&out) {
        eprintln!("skipping assembly/hello: no assembler installed");
        return;
    }
    assert_success(&out, "assembly/hello build");

    let run = run_binary(&dir, "asm-hello", &[]);
    assert_success(&run, "assembly/hello run");
}

// ── Mixed language ────────────────────────────────────────────────────────────

#[test]
fn mixed_c_cpp_builds() {
    let dir = example(&["mixed", "c-cpp"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "mixed/c-cpp build");
}

#[test]
fn mixed_tri_lang_builds() {
    let dir = example(&["mixed", "tri-lang"]);
    let out = freight(&dir, &["build"]);
    if missing_toolchain(&out) {
        eprintln!("skipping mixed/tri-lang: a required compiler is not installed");
        return;
    }
    assert_success(&out, "mixed/tri-lang build");

    let run = run_binary(&dir, "tri-lang", &[]);
    assert_success(&run, "mixed/tri-lang run");
}

// ── Features ──────────────────────────────────────────────────────────────────

#[test]
fn cpp_features_default_builds() {
    let dir = example(&["cpp", "features"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "cpp/features default build");
}

#[test]
fn cpp_features_explicit_builds() {
    let dir = example(&["cpp", "features"]);
    let out = freight(&dir, &["build", "--features", "logging,json"]);
    assert_success(&out, "cpp/features explicit features build");
}

#[test]
fn cpp_features_no_defaults_builds() {
    let dir = example(&["cpp", "features"]);
    let out = freight(&dir, &["build", "--no-default-features"]);
    assert_success(&out, "cpp/features no-default-features build");
}

// ── Misc ──────────────────────────────────────────────────────────────────────

#[test]
fn misc_platform_deps_builds() {
    let dir = example(&["misc", "platform-deps"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "misc/platform-deps build");

    let run = run_binary(&dir, "platform-deps", &[]);
    assert_success(&run, "misc/platform-deps run");
}

// ── Cargo-parity: targets, patch, workspace inheritance, aliases ───────────────

#[test]
fn required_features_and_default_run() {
    let dir = example(&["c", "required-features"]);
    let _ = freight(&dir, &["clean"]); // pristine state for the gate assertions

    // Plain build: only `toolkit` links; `diag` is gated out.
    let out = freight(&dir, &["build"]);
    assert_success(&out, "required-features default build");
    assert!(
        dir.join("target/debug/toolkit").exists(),
        "toolkit should build"
    );
    assert!(
        !dir.join("target/debug/diag").exists(),
        "diag must be gated out without --features extras"
    );

    // With the feature, both binaries link.
    let out = freight(&dir, &["build", "--features", "extras"]);
    assert_success(&out, "required-features extras build");
    assert!(
        dir.join("target/debug/diag").exists(),
        "diag should build with --features extras"
    );

    // default-run selects `toolkit` without --bin.
    let run = freight(&dir, &["run"]);
    assert_success(&run, "default-run");
    assert_output_contains(&run, &["toolkit: primary tool"]);
}

#[test]
fn example_targets_build_and_run() {
    let dir = example(&["misc", "examples-target"]);
    let out = freight(&dir, &["build", "--examples"]);
    assert_success(&out, "build --examples");
    assert!(
        dir.join("target/debug/examples/basic").exists(),
        "basic example"
    );
    assert!(
        dir.join("target/debug/examples/fancy").exists(),
        "fancy example"
    );

    let run = freight(&dir, &["run", "--example", "fancy"]);
    assert_success(&run, "run --example fancy");
    assert_output_contains(&run, &["(2 + 3) * 4 = 20"]);
}

#[test]
fn patch_overrides_dependency_source() {
    let dir = example(&["deps", "patch"]);
    let out = freight(&dir, &["run"]);
    assert_success(&out, "deps/patch run");
    assert_output_contains(&out, &["PATCHED greeter"]);
    assert_output_missing(&out, "UPSTREAM greeter");
}

#[test]
fn workspace_inheritance_resolves() {
    let dir = example(&["misc", "workspace-inherit", "app"]);
    let out = freight(&dir, &["run"]);
    assert_success(&out, "workspace-inherit app run");
    assert_output_contains(&out, &["workspace greeter library"]);
}

#[test]
fn command_alias_expands_to_build() {
    let dir = example(&["misc", "aliases"]);
    let out = freight(&dir, &["b"]); // [alias] b = "build"
    assert_success(&out, "alias `b` should expand to build");
}
