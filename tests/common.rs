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

/// Copy an example project's sources into a fresh temp dir, so a test that
/// mutates `target/` (build, then `clean`) can't race another test using the
/// same example. Build/output dirs are skipped. Hold the returned `TempDir` for
/// the test's lifetime; call `.path()` for the project dir.
pub fn example_copy(groups: &[&str]) -> tempfile::TempDir {
    let src = example(groups);
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_sources(&src, tmp.path());
    tmp
}

fn copy_sources(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read example dir").flatten() {
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some("target" | ".pkgs" | ".deps" | ".freight" | ".freight-build")
        ) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            std::fs::create_dir_all(&to).expect("mkdir");
            copy_sources(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
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
    // Cargo sets this to the freshly built `freight` binary for integration
    // tests, regardless of repo layout (monorepo member or standalone repo).
    PathBuf::from(env!("CARGO_BIN_EXE_freight"))
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

/// True when a build failed only because the language toolchain isn't installed
/// (e.g. no gfortran / no assembler). Lets toolchain-specific example tests skip
/// gracefully on machines that don't have every compiler, rather than fail.
pub fn missing_toolchain(out: &Output) -> bool {
    if out.status.success() {
        return false;
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    combined.contains("no compiler found for language")
}

/// Run the built binary for `example_dir` and return its output.
/// Assumes a single binary whose name matches the package name.
pub fn run_binary(example_dir: &Path, binary_name: &str, args: &[&str]) -> Output {
    // Try dev build first, then release.
    let dev = example_dir.join(format!("target/debug/{binary_name}"));
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
