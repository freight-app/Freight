use std::path::PathBuf;
use std::sync::Arc;

/// Structured event emitted by the build engine.
///
/// Consumers receive these through the [`Progress`] callback passed to
/// `build_project_at` and related functions. The default CLI callback
/// translates them to coloured stdout lines; GUI/TUI frontends can render
/// them however they like.
#[derive(Debug, Clone)]
pub enum BuildEvent {
    /// Top-level build started for a package.
    BuildStarted { name: String, profile: String },
    /// A source file is being compiled.
    Compiling { path: PathBuf },
    /// A source file's object is up-to-date and was skipped.
    Fresh { path: PathBuf },
    /// Linking a binary or shared lib.
    Linking { name: String },
    /// Archiving a static lib.
    Archiving { name: String },
    /// Running (or skipping cached) `build.freight` script.
    RunningScript { cached: bool },
    /// A line of output captured from a tool a build plugin ran via `run(...)`.
    /// `source` is the tool name; `is_err` marks the stderr stream.
    ScriptOutput {
        source: String,
        text: String,
        is_err: bool,
    },
    /// Fetching a git/http/registry dep.
    FetchingDep { name: String, source: String },
    /// Starting a source build of a cached dep (no prebuilt available).
    DepBuildStarted { name: String },
    /// One source file compiled inside a dep build (contributes one dot to the inline bar).
    DepCompiling,
    /// All source files for a dep build are done.
    DepBuildDone,
    /// Building a foreign dep (cmake/make/meson/…).
    BuildingForeignDep { name: String, backend: String },
    /// Non-fatal warning from any part of the build.
    Warning(String),
    /// Linking a test binary.
    TestLinking { name: String },
    /// Running a test binary.
    TestRunning { name: String },
    /// Result of a single test binary.
    TestResult { name: String, passed: bool },
    /// Linking a benchmark binary.
    BenchLinking { name: String },
    /// Running a benchmark binary.
    BenchRunning { name: String },
    /// Result of a single benchmark binary (wall-clock mean in nanoseconds).
    BenchResult { name: String, mean_ns: u64 },
    /// Wall-clock time to compile one source file (only emitted when `--time-passes` is active).
    Timing { path: PathBuf, ns: u64 },
    /// An intermediate file was emitted (asm, llvm-ir, llvm-bc, preprocessed).
    Emitted { target: String, path: PathBuf },
}

/// A shared, thread-safe progress sink.
///
/// Implemented as an `Arc<dyn Fn>` so it can be cloned cheaply into rayon
/// parallel iterators without requiring a reference lifetime.
pub type Progress = Arc<dyn Fn(BuildEvent) + Send + Sync>;

/// Returns a no-op [`Progress`] that discards all events.
pub fn silent() -> Progress {
    Arc::new(|_| {})
}
