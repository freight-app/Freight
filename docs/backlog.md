# Freight — Feature Backlog

Items that are worth building but haven't been prioritised yet.
Each entry notes the rough scope and why it matters.

---

## Developer experience

### `freight fmt`
Run `clang-format` (or a configured formatter) over all sources in `src/`.
Auto-detect the formatter binary; fall back to a built-in style when none is
installed. Reads a `[fmt]` section in `freight.toml` for style overrides.

### `freight lint`
Invoke `clang-tidy` (C/C++), `flint++`, or other static analysers.
Configurable via `[lint]` in `freight.toml`. Integrate check results with
`freight check` in CI mode (`--ci` flag → non-zero exit on any warning).

### `freight bench`
Build with profile `bench` (release + debug symbols, no strip) and run
binaries whose names match `bench_*` or live in `benches/`. Time each run,
print a simple table. Optional integration with `criterion` via a flag.

### Better diagnostics
Parse compiler error output (GCC/Clang column markers, MSVC error codes) and
re-emit with clickable `file:line:col` references. Show a snippet of the
source line. Reduce wall-of-text compile errors to a concise summary.

### `freight outdated`
Compare locked dependency versions (path: current rev, registry: semver range)
against the latest available. Print a coloured table. Analogous to `cargo outdated`.

---

## Build system

### Profile inheritance
Allow `[profile.custom]` to inherit from `dev` or `release` with selective
overrides, e.g. `inherits = "release"; debug = true`. Avoids duplicating the
full flag set for a profiling or coverage profile.

### Sanitizer presets via CLI
`freight test --sanitize address,undefined` — override the profile's sanitize
list from the command line without editing `freight.toml`. Useful for one-off
checks without polluting the manifest.

### `rerun_if` implementation in `build.freight`
`rerun_if_changed("path/to/file")` and `rerun_if_env_changed("VAR")` — skip
re-running the build script when none of the declared inputs changed. Currently
the script always re-runs on every build.

### Precompiled headers (PCH)
`[compiler] pch = "include/stdafx.h"` — compile the header once and inject
`-include-pch` for subsequent TUs. Large projects with a shared heavy header
can see 2–4× build speedups.

### Unity / jumbo builds
`[compiler] unity = true` — concatenate all TUs into a single translation
unit per language. Trades incremental build speed for full-build speed and
better cross-TU inlining without LTO overhead.

### `FREIGHT_SYSROOT` auto-propagation
When `FREIGHT_SYSROOT` is set, automatically inject `--sysroot=` even without
a `[compiler] sysroot` entry. Reduces boilerplate for SDK-based cross builds.

---

## Dependencies

### Workspace support improvements
Per-member feature flags (`freight build -p mylib --features tls`), workspace-
level `[patch]` overrides, and `freight workspace graph` visualisation.

### `freight graph`
Emit the dependency graph as DOT or Mermaid so it can be rendered. Useful for
large projects to audit transitive deps.

### Private registry support
`FREIGHT_REGISTRY_URL` env override so teams can point at an internal registry
without editing `freight.toml`. Credential storage in `~/.freight/credentials.toml`.

---

## Tooling integration

### VS Code extension
Activates on `freight.toml`, delegates to `freight lsp` for diagnostics,
completions, and go-to-definition. Publish to the VS Code Marketplace.

### `compile_commands.json` incremental update
Currently regenerated from scratch on every run. Cache the previous output and
only emit entries for sources that changed, so large projects don't incur a
full re-scan on every `freight build`.

### `--emit asm`
`freight build --emit asm` — compile to `.s` and open (or write to
`target/{profile}/asm/`) so developers can inspect codegen without a
separate `objdump` / `godbolt` workflow.

### `--time-passes`
Instrument each compilation step and print a table of per-file build times
sorted descending. Helps identify which TUs dominate build time.

---

## Packaging

### `freight package`
Bundle the project into a distributable tarball (`{name}-{version}.tar.gz`)
with sources, `freight.toml`, and a generated `Makefile` fallback for systems
without freight installed.

### macOS / Windows distribution
`freight install` for binary crates (analogous to `cargo install`). Cross-
compiled `.exe` via a Windows sysroot / `x86_64-w64-mingw32` toolchain.
