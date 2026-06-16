/// Integration tests: `freight clean` removes artifacts; incremental rebuild
/// reuses unchanged objects.
mod common;
use common::*;

// ── clean ─────────────────────────────────────────────────────────────────────

#[test]
fn clean_removes_target_dir() {
    // Isolated copy: this test deletes target/, so it must not share a dir with
    // any concurrently-running test.
    let proj = example_copy(&["c", "hello"]);
    let dir = proj.path();
    // Ensure there is something to clean.
    let _ = freight(dir, &["build"]);
    let target = dir.join("target");
    assert!(target.exists(), "target dir should exist after build");

    let out = freight(dir, &["clean"]);
    assert_success(&out, "freight clean");
    assert!(!target.exists(), "target dir should be gone after clean");
}

#[test]
fn rebuild_after_clean_succeeds() {
    let proj = example_copy(&["c", "hello"]);
    let dir = proj.path();
    let _ = freight(dir, &["clean"]);
    let out = freight(dir, &["build"]);
    assert_success(&out, "rebuild after clean");
}

// ── incremental ───────────────────────────────────────────────────────────────

#[test]
fn second_build_is_incremental() {
    // Use a plain (non-modules) C++ project: C++20/23 named-module units are
    // currently rebuilt every time (see "Known limitations"), so they aren't a
    // valid subject for the incremental check.
    let proj = example_copy(&["cpp", "static-lib"]);
    let dir = proj.path();
    // Cold build.
    let cold = freight(dir, &["build"]);
    assert_success(&cold, "cpp/static-lib cold build");

    // Warm build — no source changes, nothing to recompile.
    let warm = freight(dir, &["build"]);
    assert_success(&warm, "cpp/static-lib warm build");

    let cold_out = format!(
        "{}\n{}",
        String::from_utf8_lossy(&cold.stdout),
        String::from_utf8_lossy(&cold.stderr),
    );
    let warm_out = format!(
        "{}\n{}",
        String::from_utf8_lossy(&warm.stdout),
        String::from_utf8_lossy(&warm.stderr),
    );
    // The warm build should not say "Compiling" for source files.
    assert!(
        !warm_out.contains("Compiling"),
        "warm build should not recompile unchanged sources\ncold:\n{cold_out}\nwarm:\n{warm_out}"
    );
}
