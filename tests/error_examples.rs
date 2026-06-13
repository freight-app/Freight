/// Integration tests: `freight build` on broken examples must fail with
/// informative diagnostics.
mod common;
use common::*;

// ── Compile error ─────────────────────────────────────────────────────────────

#[test]
fn compile_error_fails() {
    let dir = example(&["broken", "compile-error"]);
    let out = freight(&dir, &["build"]);
    assert_failure(&out, "broken/compile-error should not build");
}

#[test]
fn compile_error_reports_file() {
    let dir = example(&["broken", "compile-error"]);
    let out = freight(&dir, &["build"]);
    // Freight surfaces the compiler's diagnostics; at minimum the source file
    // name must appear somewhere in the output.
    assert_output_contains(&out, &["main.cpp"]);
}

// ── Link error ────────────────────────────────────────────────────────────────

#[test]
fn link_error_fails() {
    let dir = example(&["broken", "link-error"]);
    let out = freight(&dir, &["build"]);
    assert_failure(&out, "broken/link-error should not link");
}

#[test]
fn link_error_mentions_symbol() {
    let dir = example(&["broken", "link-error"]);
    let out = freight(&dir, &["build"]);
    // The linker must complain about the undefined symbol.
    assert_output_contains(&out, &["compute_answer"]);
}

// ── Bad dependency ────────────────────────────────────────────────────────────

#[test]
fn bad_dep_fails_at_resolution() {
    let dir = example(&["broken", "bad-dep"]);
    let out = freight(&dir, &["build"]);
    assert_failure(&out, "broken/bad-dep should fail at dep resolution");
}

#[test]
fn bad_dep_names_missing_package() {
    let dir = example(&["broken", "bad-dep"]);
    let out = freight(&dir, &["build"]);
    // Freight's dep resolver must name the package it cannot find.
    assert_output_contains(&out, &["libdoesnotexist"]);
}

// ── Undeclared include (hygiene Phase 2 enforcement) ──────────────────────────

#[test]
fn undeclared_include_blocks_build_under_deny() {
    let dir = example(&["broken", "undeclared-include"]);
    let out = freight(&dir, &["build"]);
    assert_failure(
        &out,
        "broken/undeclared-include should be blocked by the include-hygiene pass",
    );
}

#[test]
fn undeclared_include_names_the_header() {
    let dir = example(&["broken", "undeclared-include"]);
    let out = freight(&dir, &["build"]);
    // The pass must name the offending header and not flag the stdlib one.
    assert_output_contains(&out, &["<pthread.h>", "undeclared"]);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !combined.contains("<stdio.h>"),
        "standard-library <stdio.h> must not be flagged, but it was:\n{combined}"
    );
}

#[test]
fn declared_owner_suppresses_system_header() {
    // `<zlib.h>` lives bare in /usr/include; declaring `zlib` must attribute it
    // (Phase 3 ownership) so only the still-undeclared `<pthread.h>` is named.
    // Skipped where zlib's header isn't installed (the check can't confirm it).
    if !std::path::Path::new("/usr/include/zlib.h").exists() {
        return;
    }
    let dir = example(&["broken", "undeclared-include-owned"]);
    let out = freight(&dir, &["build"]);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !combined.contains("<zlib.h>"),
        "declared `zlib` should suppress <zlib.h>, but it was flagged:\n{combined}"
    );
    assert!(
        combined.contains("<pthread.h>"),
        "the undeclared <pthread.h> should still be reported:\n{combined}"
    );
}

// ── Runtime crash ─────────────────────────────────────────────────────────────

#[test]
fn runtime_crash_builds_successfully() {
    let dir = example(&["broken", "runtime-crash"]);
    let out = freight(&dir, &["build"]);
    // The project has deliberate runtime errors but no compile/link errors.
    assert_success(&out, "broken/runtime-crash must build cleanly");
}

#[test]
fn runtime_crash_exits_nonzero() {
    let dir = example(&["broken", "runtime-crash"]);
    // Build unconditionally so this test doesn't race with the build test.
    let build = freight(&dir, &["build"]);
    assert_success(&build, "broken/runtime-crash build for run test");
    // Running without args triggers the null-dereference path.
    let run = run_binary(&dir, "runtime-crash", &[]);
    assert_failure(&run, "broken/runtime-crash should crash at runtime");
}
