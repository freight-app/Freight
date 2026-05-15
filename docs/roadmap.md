# Development Roadmap

Feature branches follow the convention `feature/<name>` off `master`.

---

### CLI Bootstrap ✓ COMPLETE
- [x] Cargo workspace: `freight` (bin) + `freight-core` (lib)
- [x] `clap` wiring — all subcommands stubbed
- [x] `FreightError` enum with `thiserror`
- [x] Coloured output helpers: success `✓`, warning `⚠`, error `✗`
- [x] `freight new <name> --lang <lang>` — scaffold directory + freight.toml + hello-world src
- [x] `freight init [--lang <lang>]` — init in current dir, auto-detects language from existing files

### Manifest ✓ COMPLETE
- [x] Serde structs for every freight.toml section (`manifest/types.rs`)
- [x] Parse + validate with `toml_edit`
- [x] `freight check` — validate manifest, print clear errors or a summary
- [x] `find_manifest_dir` — walk up the directory tree to locate `freight.toml`
- [x] `Manifest::build_settings_for(profile)` — convert manifest + profile into `BuildSettings`
- [x] ABI compatibility validation for path dependencies
- [x] C/C++ standard consistency validation

### Compiler Detection ✓ COMPLETE
- [x] Probe `$PATH` for known compiler binaries
- [x] Load + evaluate compiler template `.rhai` scripts at runtime
- [x] `CompilerTemplate` struct + `assemble_flags()` method (pure, unit-tested)
- [x] `freight toolchain list` — grouped by family (gnu, llvm, intel, nvidia); guest extensions shown separately
- [x] `freight toolchain use <name>` — accepts family names and standalone primaries; rejects individual compilers that belong to a family and guest extensions
- [x] `family` field in rhai scripts groups compilers into named suites
- [x] `requires_toolchain` field marks guest/extension compilers (nvcc, hipcc, nasm, yasm, …); auto-dropped when no host toolchain is detected
- [x] Toolchain version cache (`~/.freight/toolchain-cache.json`, mtime-validated)
- [x] 23 bundled compiler templates: gcc, g++, gfortran, gdc, clang, clang++, flang, ldc2, icpx, ifx, ispc, hipcc, nvcc, nvc, nvc++, nvfortran, gas, nasm, yasm, dmd, msvc, opencl, tcc
- [x] gcc and clang scripts probe versioned binaries (`g++-14`, `clang++-17`, …) as fallbacks

### Build Engine ✓ COMPLETE
- [x] Source discovery with `walkdir` — extension → language key routing
- [x] Parallel compilation via `rayon`
- [x] Mtime dirty checking — source vs object, headers via `.d` dep files
- [x] `.d` dep file generation (`-MMD -MF`) for transitive header tracking; stdout mode (`/showIncludes`) for MSVC
- [x] Linker invocation — binary, static lib (`.a`), shared lib (`.so`)
- [x] `freight build` + `freight run` end-to-end
- [x] `freight test` — compiles test files, links against project objects (excluding `main()`), runs each test binary
- [x] `freight clean` — wipes `target/`
- [x] Multi-language builds — C + C++ in one project, each compiled with the right binary
- [x] Multi-bin fix — each `[[bin]]` links only its own entry-point object
- [x] Toolset roles — `output_obj`/`output_bin` split, `lto_link` flag category, `system_lib` format string
- [x] MSVC support — `/Fo`, `/Fe`, `/GL` + `/LTCG`, `{name}.lib` system libs, `/showIncludes` dep tracking

### Benchmarks ✓ COMPLETE
- [x] Built-in `bench` profile — `opt-level = 3`, `debug = true`, `strip = false`, `lto = false`; overridable via `[profile.bench]` in `freight.toml`
- [x] `freight bench [<filter>]` — build and run all source files under `benches/` as standalone bench binaries
- [x] Each bench binary is run 5 times; per-bench min / mean / max wall-clock table printed to stdout
- [x] Workspace support — `freight bench` at workspace root runs benches for every member
- [x] `BenchLinking`, `BenchRunning`, `BenchResult` events added to `BuildEvent` for GUI/TUI frontends
- [x] `bench_project`, `bench_project_with`, `bench_project_at`, `bench_workspace_with` public API in `freight-core`

### Dependencies ✓ COMPLETE
- [x] Path dependency resolution — compile dep, archive to `.a`, link into project
- [x] System dependency linking — `{ system = "..." }` → `-l{name}` (or `{name}.lib` for MSVC)
- [x] `LibType::System` — no build artifact; injects `-l{link}` flag only
- [x] 24 system-lib stubs in `toolchains/system-libs/` — pthread, libm, dl, rt, ws2_32, kernel32, d3d11, d3d12, bcrypt, and more; filtered by `supports` expression
- [x] `repo = "system"` dep key — bypasses pkg-config/vcpkg, resolves via stubs
- [x] Full resolver chain: `pkg-config → conan → vcpkg → system-lib stub`; `repo` pins one step
- [x] `supports.rs` — shared boolean platform-expression parser (`HostEnv`, `eval_supports()`) used by stubs and the `freight add` TUI
- [x] Dependency graph with topological sort (Kahn's algorithm)
- [x] Cycle detection with error
- [x] `.deps/<name>/` folder convention for version-pinned deps
- [x] Transitive dep checks — errors if a dep's dep is not present, does not fetch recursively
- [x] Dep include dirs accumulated in topo order for multi-level dep builds
- [x] `provides = [...]` slot-based substitution — shallower dep wins; same-depth conflict = hard `SlotConflict` error; root project (depth 0) always wins; dropped deps filtered before compilation

### Foreign Build System Integration ✓ COMPLETE
- [x] Auto-detect foreign build system from dep directory — CMake > Meson > Autotools > SCons > Make
- [x] CMake, Meson, Make, Autotools, SCons foreign deps: configure → build → install
- [x] Git dependencies — `{ git = "https://..." }` clones into `.deps/<name>/`, then treated as path dep
- [x] Foreign dep include + archive auto-discovery after build
- [x] HTTP tarball deps — `{ http = "...", sha256 = "..." }` with SHA-256 verification
- [x] GitHub release deps — `{ github = "owner/repo", tag = "v1.0" }` shorthand
- [x] Download sentinel — `.deps/<name>/.freight-fetched` prevents re-downloading
- [x] pkg-config deps — standalone or with system fallback
- [x] `backend = "none"` explicit header-only override
- [x] Header-only auto-detection when no build system and no source files found
- [x] **pkg-config**: `pkgconf` fallback when `pkg-config` binary is absent; cross-compile env var lookup (`PKG_CONFIG_PATH_<target>`, `TARGET_PKG_CONFIG_PATH`, …); `PKG_CONFIG_LIBDIR` / `PKG_CONFIG_SYSROOT_DIR` passthrough; `PKG_CONFIG_ALL_STATIC` static-link mode
- [x] **CMake**: Ninja generator auto-selected when `ninja` is on `$PATH`; `CMAKE_SYSTEM_NAME` + `CMAKE_SYSTEM_PROCESSOR` injected from target triple for cross-builds; `cmake --build --parallel N` on CMake ≥ 3.12; `cmake --install` step with explicit prefix
- [x] **Autotools**: `--host=<triple>` passed to `configure` for cross-builds; parallel `make -j{N}`; fast-build configure skip when `config.status` + `Makefile` are up-to-date; `--enable-static --disable-shared`; Emscripten `emconfigure`/`emmake` for wasm/emscripten targets

### Features System ✓ COMPLETE
- [x] `[features]` table — keys map to lists of implied feature names
- [x] `"default"` key lists features active when no explicit selection is made
- [x] Active features produce `-D<NAME_UPPER>` compiler flags for all sources
- [x] Feature closure: BFS expansion of transitive implications
- [x] Cycle detection in `[features]` with clear error
- [x] Per-dep feature selection: `mylib = { path = "../mylib", features = ["tls"] }`
- [x] `default-features = false` to opt out of dep defaults
- [x] `build/features.rs` — `resolve_features()` + `to_defines()` (pure, unit-tested)

### Assembly & Target Config ✓ COMPLETE
- [x] NASM template — `.asm`/`.nasm`, arch-specific output format via `[arch_flags]`
- [x] YASM template — drop-in NASM-compatible x86/x86_64 assembler
- [x] GAS template (`gas.rhai`) — binutils `as`, `.s`/`.S`, `requires_toolchain = ["c"]`; gcc and clang also handle `.s`/`.S` natively
- [x] `[target]` section — `arch` and `cpu_extensions` (generates `-m<ext>` flags)
- [x] `[arch_flags]` in templates — keyed by `"arch.os"` first, `"arch"` fallback

### C++20 Modules ✓ COMPLETE
- [x] Scan source files for `export module` / `import` statements
- [x] Classify files as MIU / MImplU / Regular TU
- [x] Global module fragment support (`module;` + `#include` before `export module`)
- [x] Build module DAG — Kahn's topo sort into parallel batches
- [x] Cycle detection with `DependencyCycle` error
- [x] GCC one-step MIU compilation: `-fmodule-output={pcm_path}`
- [x] Clang two-step MIU compilation: `--precompile` → `.pcm`, then `-c` → `.o`
- [x] Incremental: MIUs skipped when both `.o` and `.pcm` are up-to-date

### Cross-Compilation ✓ COMPLETE
- [x] `[compiler] target` → `--target={triple}` via template `structure.target`
- [x] `[compiler] sysroot` → `--sysroot={path}` via template `structure.sysroot`
- [x] `targets = [...]` dep filter — gated by `compiler.target`
- [x] `os = ...` dep filter — gated by host OS; accepts family aliases (`unix`, `bsd`)
- [x] `arch = ...` dep filter — gated by `std::env::consts::ARCH`
- [x] `freight toolchain add <path>` — validates and installs a local `.rhai` script


### Documentation Generator ✓ COMPLETE
- [x] `doc/extract.rs` — line-scanner extractor for C/C++, Rust, Fortran, D, Ada
- [x] `doc/markdown.rs` — math protection + Markdown conversion helpers
- [x] `doc/render_md.rs` — GFM Markdown renderer with per-file pages and index
- [x] `doc/render_json.rs` — JSON + MessagePack renderers for tooling/doc apps
- [x] `freight doc` dependency TUI plus `freight doc --format md|json|msgpack|all`
- [x] `freight doc --man [--out-dir DIR]` — man pages via clap_mangen
- [x] `crates/freight-doc/` — standalone `freight-doc` binary
- [x] `examples/doc-example/` — C, C++, Fortran sources with LaTeX math in comments; multi-lib project showcasing path deps in the TUI
- [x] TUI DocView: colored rendering — item name (yellow/bold), signature (green), section labels (magenta/bold), table borders (dark gray), param names (cyan/bold)
- [x] TUI DocView: box-drawing parameter table with separator row between each param, word-wrapped description column
- [x] TUI DocView: brief shown between signature and parameters; body shown before param table
- [x] TUI DocView: LaTeX math conversion (`$...$`, `$$...$$`) → Unicode (Greek, operators, super/subscripts, `\frac`)
- [x] TUI DocView: structs/enums/typedefs show clean `kind name` instead of truncated first-line signature
- [x] `doc/extract.rs`: signatures trimmed of leading whitespace at storage (handles indented declarations)

### Rhai Toolchain Scripts ✓ COMPLETE
- [x] `toolchain/engine.rs` — embedded Rhai engine with registered API
- [x] Thread-local `ToolchainDef` builder; `fn check()` and `fn load()` hooks
- [x] All 11 original compiler templates ported to Rhai; 7 additional templates added
- [x] `CompilerTemplate::from_rhai(src)` — converts `ToolchainDef` into `CompilerTemplate`
- [x] `toolchain_add` updated to require `.rhai` extension
- [x] Toolset roles wired into `compile.rs` / `link.rs` — `ar_binary()`, `output_bin_flag()`
- [x] `output_obj` / `output_bin` separate structure fields with fallback to `output`
- [x] `lto_link` flag category — `assemble_link_flags()` prefers it over `lto`
- [x] `system_lib` format string — defaults to `"-l{name}"`, MSVC uses `"{name}.lib"`
- [x] `dep_file_mode = "stdout"` — `/showIncludes` stdout parsing, writes synthetic `.d`
- [x] `msvc.rhai` — full MSVC (cl.exe / link.exe) toolchain script

### Debugger Integration ✓ COMPLETE
- [x] `DebuggerTemplate` struct — `name`, `binary`, `[launch]` separator, `[dap]` config, `settings`, `default_args`
- [x] `detect_debuggers()` — probes `$PATH`, extracts version, finds DAP adapter binary
- [x] `toolchains/gnu/gdb.rhai` and `toolchains/llvm/lldb.rhai` — `kind = "debugger"`
- [x] `freight toolchain list` — second table section for debuggers
- [x] `freight debug [<binary>] [--debugger <name>] [-- <args>]` — builds with debug profile, execs debugger
- [x] `freight debug --launch-json` — writes/merges `.vscode/launch.json`
- [x] Debugger config is a developer concern — lives in `~/.freight/config.toml` and `<project>/.freight/config.toml`, not in `freight.toml`
- [x] `GlobalConfig::load()` + `apply_local()` — global config with per-project override

### Formatter & Linter Integration ✓ COMPLETE
- [x] `ToolTemplate` struct — `kind`, `name`, `extensions`, `run["fix"|"check"]`, `settings`, `values`
- [x] `load_formatter_templates()` / `load_linter_templates()` — `kind` pre-check routes to correct loader
- [x] `detect_tools()` — probes `$PATH`, extracts version
- [x] `select_formatter()` / `select_linter()` — picks by `[formatter] name` or first detected
- [x] `collect_sources()` — walks `src/` for files matching the template's extensions
- [x] `values["key"] = [...]` in templates — valid choices exposed to the LSP and printed as hints
- [x] `freight fmt [--check]` — format in-place or report-only
- [x] `freight lint [--fix]` — static analysis with optional auto-fix
- [x] Formatter/linter config is a **project concern** — lives in `[formatter]` / `[linter]` in `freight.toml`
- [x] 4 bundled formatter templates: `clang-format`, `astyle`, `uncrustify`, `fprettify`
- [x] 4 bundled linter templates: `clang-tidy`, `cppcheck`, `cpplint`, `flawfinder`

### Registry (in progress)

**Client (freight-core)**
- [x] `freight.lock` read/write — deterministic dep pinning (version 1 format, sha256 checksums)
- [x] `freight.lock` auto-generated on every `freight build`
- [x] `freight tree` — dependency tree with dep type labels
- [x] `freight add` / `freight remove` — manifest mutation + lock update
- [x] `freight update [package]` — refreshes lockfile checksums for path deps
- [x] `freight fetch` — verifies path deps exist
- [x] `PackageRepo` trait — `repo_key()`, `lookup()`, `search()`; multiple registry support
- [x] `registries_in_order()` — tries configured registries in declaration order; first hit wins; default freight.dev appended if no entry named `"freight"` is present
- [x] `--repo <name>` in `freight add` selects a named registry; without `--repo` all registries are tried in order
- [x] Bearer token auth — `Authorization: Bearer <token>` header in all outbound HTTP requests
- [x] `[[registry]]` config in `~/.freight/config.toml` — `name`, `url`, `token` fields; local config prepends and deduplicates
- [x] `freight search [--repo]` — tabular results table from registry
- [x] `freight info [<name>] [--repo]` — version list + description from registry (or current project)
- [x] `freight login [--registry] [--token]` — interactive token prompt; saves to `~/.freight/credentials.toml`
- [x] `freight publish [--dry-run] [--repo]` — tar bundle + cargo binary wire upload
- [x] `freight yank <name@version> [--undo] [--repo]` — yank / unyank via registry API
- [x] `freight register [--registry] [--username] [--email]` — create registry account; auto-saves returned token
- [ ] `freight fetch` — download version deps from freight.dev (registry dep lockfile support)
- [ ] `freight add` — resolve + lock exact version from freight.dev (semver resolution)

**Server (`freight-registry` — standalone repository)**
> Extracted to its own repo. Implements the full cargo-compatible registry wire protocol over Axum + SQLite.

- [x] Axum 0.7 HTTP server — `--bind` (default `0.0.0.0:7878`) and `--base-url` flags
- [x] SQLite via sqlx 0.8 — WAL mode, foreign keys, startup schema migration
- [x] `GET /api/v1/packages/{name}` — versions + metadata JSON
- [x] `GET /api/v1/packages/{name}/{version}/download` — tarball stream + `X-Checksum-SHA256` header
- [x] `GET /api/v1/search?q=<query>` — substring search over name and description
- [x] `PUT /api/v1/packages` — publish (cargo binary wire format: `[u32 JSON len][JSON][u32 tar len][tar]`)
- [x] `DELETE/PUT /api/v1/packages/{name}/{version}/yank` — yank / unyank
- [x] `GET/PUT/DELETE /api/v1/packages/{name}/owners` — multi-owner management; last-owner removal guard
- [x] `POST /api/v1/users/login` — Argon2id password verification, creates token (90-day default)
- [x] `POST /api/v1/users/register` — open HTTP registration; creates user + 90-day token in one round-trip
- [x] `GET /api/v1/me` — authenticated user info
- [x] User accounts — Argon2id password hashing; `user add/list/remove` CLI
- [x] API tokens — SHA-256 stored in DB; optional expiry; last-used tracking; `token add/list/revoke` CLI
- [x] Package ownership — first publisher auto-claims; `user_owns_package` guards all writes
- [x] Per-IP rate limiting via `governor` — 120 req/min read, 10 req/min write
- [x] Upload size cap — `DefaultBodyLimit`; `--max-upload-mb` flag (default 50 MB)
- [x] Input validation — package names, semver versions, usernames, passwords (`validate.rs`)
- [x] Audit log — fire-and-forget async inserts for login, publish, yank, unyank
- [ ] Token scopes, email verification, TOTP/2FA — see `freight-registry/TODO.md`
- [ ] S3-compatible storage backend, PostgreSQL option
- [ ] Proper versioned sqlx migrations, web UI, Docker image, mirror/proxy mode

### Language Server (in progress — `feature/lsp-server`)
- [x] Crate scaffold: `crates/freight-lsp/` (lib + bin), `tower-lsp 0.20`, stdio transport
- [x] Document store backed by `DashMap<Url, String>` — full-sync updates
- [x] Diagnostics via `freight-core`'s `validate()` + `validate_dep_compat()`
- [x] Completion: section-aware (section headers, `backend`, `warnings`, `std`, `lib.type`, field snippets)
- [x] Hover docs keyed by dotted path (`compiler.backend`, `lib.type`, …)
- [x] Go-to-definition for `path = "..."` dependencies
- [x] `freight lsp` CLI subcommand
- [ ] VS Code extension that activates on `freight.toml`
- [ ] Inlay hints showing resolved compiler flags per profile
- [ ] Code actions: "add `[[bin]]` target", "convert version dep → detailed table"

### Examples ✓ COMPLETE
- [x] `hello-cpp/` — multi-file C++ with tests
- [x] `multi-lang/` — C + C++ mixed project with tests
- [x] `with-deps/` — path dependency (static lib)
- [x] `c-simple/` — pure C, Collatz benchmark
- [x] `multi-bin/` — two binaries from one source tree
- [x] `cpp-modules/` — C++20 named modules, ASCII ray tracer
- [x] `tri-lang/` — Fortran + C + C++ N-body gravity
- [x] `asm-hello/` — C + NASM assembly
- [x] `with-cmake-dep/` — foreign CMake dep (auto-detected)
- [x] `with-make-dep/` — foreign Make dep (auto-detected)
- [x] `with-git-dep/` — git dependency cloned and built automatically
- [x] `doc-example/` — C, C++, Fortran sources with LaTeX math in doc comments
