# Build Pipeline

All freight workflows — `build`, `test`, `bench` — run through the same
ten-stage pipeline implemented in `src/build/mod.rs::run_pipeline_at`.  The
public functions `build_project_at`, `test_project_at`, and `bench_project_at`
are thin wrappers that construct a `PipelineConfig` and delegate to it.

## Stages

| # | Stage | Key code |
|---|-------|----------|
| 1 | **Load** | `load_project_at` — parse `freight.toml`, detect toolchain, discover sources |
| 2 | **Features** | `features::resolve_features` — activate/deactivate features, compute defines |
| 3 | **Fetch** | `ensure_git_deps_fetched` + `dep_cmds::fetch_registry_deps` — clone/download missing deps |
| 4 | **Resolve** | `resolve_dep_graph` + `check_slot_conflicts` — topo-sort dep graph, drop conflicting slots |
| 5 | **Build deps** | `build_resolved_deps` + `adaptors::build_foreign_deps` — compile source deps, run cmake/make/meson |
| 6 | **Proto** | `proto::run_protoc` — generate `.pb.cc` / `.pb.h` from `.proto` files (skipped if no `[language.proto]`) |
| 7 | **Header units** | `header_units::precompile_dep_headers` — precompile dep headers as BMIs (C++20 builds only, `Build` goal only) |
| 8 | **PCH** | `pch::compile_pch` — compile precompiled header if `[compiler] pch` is set |
| 9 | **Compile** | `build_sources` — compile all project sources in parallel via rayon |
| 10 | **Goal** | Goal-specific phase — link, run tests, or run benchmarks |

## Goal phase details

### `Build`

- Links compiled objects into binaries / static libs / shared libs via `link_targets`.
- Writes `freight.lock`.
- Writes `compile_commands.json` to `.freight/lsp/<profile>/` and merges dep databases.

### `Test`

- Compiles files in `tests/` as standalone binaries (each file = one test binary).
- Excludes `[[bin]]` entry-point objects from linking (they contain `main()`).
- Links each test binary with `link_test_binary`.
- Runs each binary and reports pass/fail via `BuildEvent::TestResult`.
- Includes dev-dependencies (the only goal that does so).

### `Bench`

- Same structure as `Test` but discovers files in `benches/`.
- Runs each binary `BENCH_RUNS` (5) times and records wall-clock min/mean/max.
- Reports results via `BuildEvent::BenchResult`.

## Configuration

`PipelineConfig` (defined in `src/build/pipeline.rs`):

```rust
pub struct PipelineConfig {
    pub profile: String,            // "dev", "release", or custom
    pub features: Vec<String>,      // feature flags from CLI
    pub use_defaults: bool,         // activate [features] default list
    pub target_override: Option<String>,    // cross-compilation triple
    pub sanitize_override: Vec<String>,     // override profile sanitizers
    pub goal: PipelineGoal,         // Build | Test { filter } | Bench { filter }
}
```

## Dep target directories

- Root project artifacts → `<project>/target/`
- Source-built deps → `<root>/target/deps/<name>/`

The `parent_graph` parameter to `run_pipeline_at` anchors the `.pkgs/` pool to
the root project so all transitive deps share a flat directory structure rather
than nesting inside each other's `.pkgs/` directories.
