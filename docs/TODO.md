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

### B4 — Slot-based substitution ✓ done
`provides = [...]` currently only detects conflicts. Full substitution: the dep declared
closest to the root wins; same-depth conflicts remain a hard error. Required before large
dependency graphs with BLAS/LAPACK-style provider aliases are usable.

### B5 — Package lookup chain expansion ✓ done
Version deps resolve via `pkg-config → conan → vcpkg → system-lib stub`. `repo = "system"|"conan"|"vcpkg"|"pkg-config"` pins a specific resolver. System PM detection (apt/brew/dnf/pacman/zypper/winget) emits install hints on failure. `conan.rs`, `system_pm.rs`, and `system_libs.rs` modules added. 24 built-in stubs in `toolchains/system-libs/` cover common OS primitives (pthread, ws2_32, libm, dl, rt, d3d11, …). Users can add stubs to `~/.freight/toolchains/system-libs/`.
Remaining: internal system cache registry (index on first install; skip probing on rebuild).

### B6 — `freight bench` ✓ done
`bench` profile (release + debug, no strip), run binaries matching `bench_*` or in `benches/`,
print a timing table. Optional Criterion integration via a flag.

---

## Small tasks

### S1 — Tab completion ✓ done
Generate shell completions (bash, zsh, fish) for all subcommands and their options.
When the user presses tab under `freight add`, offer known dep keys; under
`freight toolchain use`, offer detected compiler names.

### S2 — `freight outdated`
Compare locked path-dep revs and registry versions against latest available.
Print a coloured table. Analogous to `cargo outdated`.

### S3 — Better compiler diagnostics ✓ done
GCC/Clang column markers and MSVC error codes are parsed into concise
summaries with clickable `file:line:col` references and source snippets.

### S4 — `freight graph` ✓ done
Emits the dependency graph as DOT or Mermaid to stdout or a file. Useful for
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

### S10 — `compile_commands.json` incremental update ✓ done
Cache the previous output and source mtimes under `target/{profile}/`; only re-emit entries
for sources that changed or when command-affecting settings change.

### S11 — `FREIGHT_SYSROOT` env auto-propagation ✓ done
When `FREIGHT_SYSROOT` is set, inject `--sysroot=` automatically without
requiring `[compiler] sysroot` in the manifest.

### S12 — Template evaluation cache ✓ done
Cache parsed `.rhai` template results to `~/.freight/template-cache.msgpack`.
Cache key = per-file content hash + base-file hash. `CompilerTemplate` and
serializable nested template metadata derive `Serialize`/`Deserialize`; templates
with runtime option handlers are evaluated live so their callbacks remain active.

### S13 — macOS / Windows distribution packages ✓ done
`freight package --target x86_64-windows` produces a `.zip` instead of `.tar.gz`.
Windows targets emit and install `.exe` binaries, with shared-library packaging
using the existing Windows DLL/import-lib layout for MinGW or Windows sysroots.

### S14 — Unity / jumbo builds
`[compiler] unity = true` — concatenate all TUs per language into one translation
unit. Trades incremental speed for full-build speed and better cross-TU inlining.

### S15 — Workspace per-member feature flags
`freight build -p mylib --features tls`, workspace-level `[patch]` overrides,
`freight workspace graph` visualisation.
