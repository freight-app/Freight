# TODO

---

## Big tasks

### B1 — Registry server (`freight-registry` crate)
Stand up the `freight.dev` registry — package publish, search, yank, and version resolution.
Includes JWT/OAuth auth (GitHub OAuth or OIDC), a `FREIGHT_REGISTRY_URL` env override for
private registries, and credential storage in `~/.freight/credentials.toml`.
Blocks: `freight publish`, `freight search`, `freight info`, `freight login`, registry-backed
`version = "x.y"` resolution.

### B2 — VS Code extension
Activate on `freight.toml`, delegate to `freight lsp` for diagnostics, completions, and
go-to-definition. LSP currently implemented; remaining work is the extension packaging and
Marketplace publish. Also add inlay hints and `freight.toml` schema validation.

### B3 — Progress callbacks / structured build events ✓ done
`BuildEvent` enum + `Progress = Arc<dyn Fn(BuildEvent) + Send + Sync>` threaded through the
entire build pipeline. CLI translates events to coloured output. GUI/TUI/LSP can subscribe
without parsing stdout. All `println!`/`eprintln!` removed from `freight-core` build paths.

### B4 — Slot-based substitution
`provides = [...]` currently only detects conflicts. Full substitution: the dep declared
closest to the root wins; same-depth conflicts remain a hard error. Required before large
dependency graphs with BLAS/LAPACK-style provider aliases are usable.

### B5 — Package lookup chain expansion ✓ done
Version deps resolve via `pkg-config → conan → vcpkg`. `repo = "conan"|"vcpkg"|"pkg-config"`
pins a specific resolver. System PM detection (apt/brew/dnf/pacman/zypper/winget) emits
install hints on failure. `conan.rs` and `system_pm.rs` modules added.
Remaining: internal system cache registry (index on first install; skip probing on rebuild).

### B6 — `freight bench`
`bench` profile (release + debug, no strip), run binaries matching `bench_*` or in `benches/`,
print a timing table. Optional Criterion integration via a flag.

---

## Small tasks

### S1 — Tab completion
Generate shell completions (bash, zsh, fish) for all subcommands and their options.
When the user presses tab under `freight add`, offer known dep keys; under
`freight toolchain use`, offer detected compiler names.

### S2 — `freight outdated`
Compare locked path-dep revs and registry versions against latest available.
Print a coloured table. Analogous to `cargo outdated`.

### S3 — Better compiler diagnostics
Parse GCC/Clang column markers and MSVC error codes, re-emit with clickable
`file:line:col` references and a source snippet. Reduce wall-of-text compile
errors to a concise summary.

### S4 — `freight graph`
Emit the dependency graph as DOT or Mermaid to stdout or a file. Useful for
auditing transitive deps in large projects.

### S5 — `--emit asm`
`freight build --emit asm` — write `.s` files to `target/{profile}/asm/` so
developers can inspect codegen without a separate `objdump` workflow.

### S6 — `--time-passes`
Instrument each compilation step and print a per-file build time table sorted
descending. Helps identify which TUs dominate build time.

### S7 — Profile inheritance
`[profile.custom] inherits = "release"; debug = true` — avoid duplicating the
full flag set for profiling or coverage profiles.

### S8 — Sanitizer preset override via CLI
`freight test --sanitize address,undefined` — override the profile's sanitize
list from the command line without editing `freight.toml`.

### S9 — `rerun_if` in `build.freight`
`rerun_if_changed("path")` and `rerun_if_env_changed("VAR")` — skip re-running
the build script when declared inputs haven't changed.

### S10 — `compile_commands.json` incremental update
Cache the previous output; only re-emit entries for sources that changed.
Avoids a full re-scan on every `freight build` in large projects.

### S11 — `FREIGHT_SYSROOT` env auto-propagation
When `FREIGHT_SYSROOT` is set, inject `--sysroot=` automatically without
requiring `[compiler] sysroot` in the manifest.

### S12 — Template evaluation cache
Cache parsed `.rhai` template results to `~/.freight/template-cache.json`.
Cache key = per-file content hash + base-file hash. Requires `Serialize`/
`Deserialize` on `CompilerTemplate` and all nested types.

### S13 — macOS / Windows distribution packages
`freight package --target x86_64-windows` producing a `.zip` instead of `.tar.gz`.
Cross-compiled `.exe` via `x86_64-w64-mingw32` toolchain or a Windows sysroot.

### S14 — Unity / jumbo builds
`[compiler] unity = true` — concatenate all TUs per language into one translation
unit. Trades incremental speed for full-build speed and better cross-TU inlining.

### S15 — Workspace per-member feature flags
`freight build -p mylib --features tls`, workspace-level `[patch]` overrides,
`freight workspace graph` visualisation.
