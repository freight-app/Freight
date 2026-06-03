/// Integration tests: `freight clean` removes artifacts; incremental rebuild
/// reuses unchanged objects.
mod common;
use common::*;

// ── clean ─────────────────────────────────────────────────────────────────────

#[test]
fn clean_removes_target_dir() {
    let dir = example(&["c", "hello"]);
    // Ensure there is something to clean.
    let _ = freight(&dir, &["build"]);
    let target = dir.join("target");
    assert!(target.exists(), "target dir should exist after build");

    let out = freight(&dir, &["clean"]);
    assert_success(&out, "freight clean");
    assert!(!target.exists(), "target dir should be gone after clean");
}

#[test]
fn rebuild_after_clean_succeeds() {
    let dir = example(&["c", "hello"]);
    let _ = freight(&dir, &["clean"]);
    let out = freight(&dir, &["build"]);
    assert_success(&out, "rebuild after clean");
}

// ── incremental ───────────────────────────────────────────────────────────────

#[test]
fn second_build_is_incremental() {
    let dir = example(&["cpp", "hello"]);
    // Cold build.
    let cold = freight(&dir, &["build"]);
    assert_success(&cold, "cpp/hello cold build");

    // Warm build — no source changes, nothing to recompile.
    let warm = freight(&dir, &["build"]);
    assert_success(&warm, "cpp/hello warm build");

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
