/// Integration-test helpers shared across all test files.
///
/// Tests run the `freight` binary directly against the example projects so
/// every test exercises the real build pipeline end-to-end.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ── Paths ─────────────────────────────────────────────────────────────────────

pub fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

pub fn example(groups: &[&str]) -> PathBuf {
    let mut p = examples_dir();
    for g in groups {
        p = p.join(g);
    }
    p
}

// ── Invocation ────────────────────────────────────────────────────────────────

/// Run `freight <args>` inside `dir`.
pub fn freight(dir: &Path, args: &[&str]) -> Output {
    let bin = freight_bin();
    Command::new(&bin)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run freight ({}): {e}", bin.display()))
}

fn freight_bin() -> PathBuf {
    // Prefer the binary freshly built for this workspace; fall back to PATH.
    // CARGO_MANIFEST_DIR = .../crates/freight  →  nth(2) = workspace root
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf();
    let dev = workspace.join("target/debug/freight");
    if dev.exists() {
        return dev;
    }
    let rel = workspace.join("target/release/freight");
    if rel.exists() {
        return rel;
    }
    // Fall back to whatever is on PATH.
    PathBuf::from("freight")
}

// ── Assertions ────────────────────────────────────────────────────────────────

pub fn assert_success(out: &Output, context: &str) {
    assert!(
        out.status.success(),
        "{context} exited {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

pub fn assert_failure(out: &Output, context: &str) {
    assert!(
        !out.status.success(),
        "{context} unexpectedly succeeded (expected failure)\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Assert that combined stdout+stderr contains every substring in `needles`.
pub fn assert_output_contains(out: &Output, needles: &[&str]) {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    for needle in needles {
        assert!(
            combined.contains(needle),
            "expected output to contain {:?}\nfull output:\n{combined}",
            needle,
        );
    }
}

/// Assert that combined stdout+stderr does NOT contain `needle`.
pub fn assert_output_missing(out: &Output, needle: &str) {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !combined.contains(needle),
        "output should NOT contain {:?}\nfull output:\n{combined}",
        needle,
    );
}

/// Run the built binary for `example_dir` and return its output.
/// Assumes a single binary whose name matches the package name.
pub fn run_binary(example_dir: &Path, binary_name: &str, args: &[&str]) -> Output {
    // Try dev build first, then release.
    let dev = example_dir.join(format!("target/dev/{binary_name}"));
    let bin = if dev.exists() {
        dev
    } else {
        example_dir.join(format!("target/release/{binary_name}"))
    };
    assert!(bin.exists(), "binary not found at {}", bin.display());
    Command::new(&bin)
        .args(args)
        .current_dir(example_dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run binary ({}): {e}", bin.display()))
}
