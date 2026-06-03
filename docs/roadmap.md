# Development Roadmap

Feature branches follow the convention `feature/<name>` off `master`.

---

### CLI Bootstrap ✓ COMPLETE
- [x] Cargo workspace: `freight` (bin) + `freight` (lib)
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
- [x] `bench_project`, `bench_project_with`, `bench_project_at`, `bench_workspace_with` public API in `freight`

### Dependencies ✓ COMPLETE
- [x] Path dependency resolution — compile dep, archive to `.a`, link into project
- [x] System dependency linking — `{ system = "..." }` → `-l{name}` (or `{name}.lib` for MSVC)
- [x] `LibType::System` — no build artifact; injects `-l{link}` flag only
- [x] 24 system-lib stubs in `toolchains/system-libs/` — pthread, libm, dl, rt, ws2_32, kernel32, d3d11, d3d12, bcrypt, and more; filtered by `supports` expression
- [x] `repo = "system"` dep key — bypasses pkg-config, resolves via stubs
- [x] Resolver chain: `pkg-config → system-lib stubs → registry`; `repo` pins one step
- [x] `supports.rs` — shared boolean platform-expression parser (`HostEnv`, `eval_supports()`) used by stubs and the `freight add` TUI
- [x] Dependency graph with topological sort (Kahn's algorithm)
- [x] Cycle detection with error
- [x] `.pkgs/<name>/` folder convention for version-pinned deps
- [x] Transitive dep checks — errors if a dep's dep is not present, does not fetch recursively
- [x] Dep include dirs accumulated in topo order for multi-level dep builds
- [x] `provides = [...]` slot-based substitution — shallower dep wins; same-depth conflict = hard `SlotConflict` error; root project (depth 0) always wins; dropped deps filtered before compilation

### Foreign Build System Integration ✓ COMPLETE
- [x] Auto-detect foreign build system from dep directory — CMake > Meson > Autotools > SCons > Make
- [x] CMake, Meson, Make, Autotools, SCons foreign deps: configure → build → install
- [x] Git dependencies — `{ git = "https://..." }` clones into `.pkgs/<name>/`, then treated as path dep
- [x] Foreign dep include + archive auto-discovery after build
- [x] HTTP tarball deps — `{ http = "...", sha256 = "..." }` with SHA-256 verification
- [x] GitHub release deps — `{ github = "owner/repo", tag = "v1.0" }` shorthand
- [x] Download sentinel — `.pkgs/<name>/.freight-fetched` prevents re-downloading
- [x] pkg-config deps — standalone or with system fallback
- [x] `type = "none"` explicit header-only / prebuilt override
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
- [x] `examples/misc/doc/` — C, C++, Fortran sources with LaTeX math in comments; multi-lib project showcasing path deps in the TUI
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

### TUI Package Browser ✓ COMPLETE
- [x] `freight add` (no args) opens a ratatui 0.29 / crossterm 0.28 interactive package browser
- [x] Search box with 350 ms debounce — triggers a registry search while you type
- [x] Scrollable package list with name, version, description columns
- [x] Detail panel on the right: version list, dependencies, description
- [x] Keyboard navigation: `↑`/`↓` / `j`/`k`, `PgUp`/`PgDn`, `g`/`G`, `Enter` to add, `Esc`/`q` to cancel
- [x] Mouse support: scroll wheel, left-click row to select
- [x] `--repo <name>` forwarded from `freight add` to scope the search to one registry
- [x] On `Enter`, calls `cmd_add` with the selected name@version — identical to typing it
- [x] `freight add <URL>` auto-detection — raw `https://`/`http://` URLs routed without flags:
  - archive extensions (`.tar.gz`, `.zip`, …) → URL dep; name derived from last path segment
  - all other HTTPS URLs → git dep; name derived from last path segment (`.git` stripped)

### Registry ✓ COMPLETE

**Client (freight_core)**
- [x] `freight.lock` read/write — deterministic dep pinning (version 1 format, sha256 checksums)
- [x] `freight.lock` auto-generated on every `freight build`
- [x] `freight tree` — dependency tree with dep type labels
- [x] `freight add` / `freight remove` — manifest mutation + lock update
- [x] `freight update [package]` — refreshes lockfile checksums for path deps
- [x] `freight fetch` — verifies path deps exist
- [x] `PackageRepo` trait — `repo_key()`, `lookup()`, `search()`, `fetch_readme()`; multiple registry support
- [x] `registries_in_order()` — tries configured registries in declaration order; first hit wins; freight.dev appended when no registries configured
- [x] `--repo <name>` in `freight add` selects a named registry; without `--repo` all registries are tried in order
- [x] Bearer token auth — `Authorization: Bearer <token>` header in all outbound HTTP requests
- [x] `[[registries]]` config in `~/.freight/config.toml` — `name`, `url`, `token` fields; local config prepends and deduplicates
- [x] `freight search [--repo]` — tabular results table from registry
- [x] `freight info [<name>] [--repo]` — version list, dependencies, and README excerpt from registry (or current project)
- [x] `freight login [--registry] [--token]` — interactive token prompt; saves to `~/.freight/credentials.toml`
- [x] `freight publish [--dry-run] [--repo]` — tar bundle + cargo binary wire upload; extracts dependencies and README from tarball
- [x] `freight yank <name@version> [--undo] [--repo]` — yank / unyank via registry API
- [x] `freight register [--registry] [--username] [--email]` — create registry account; auto-saves returned token
- [x] 5 s connect / 30 s read timeouts on all outbound registry HTTP calls

**Server (`freight-registry` — standalone repository)**
> See `freight-registry` repo for the full implementation and its own TODO/docs.

- [x] Axum HTTP server, SQLite + PostgreSQL, versioned sqlx migrations
- [x] Full package CRUD — publish, download, search, yank/unyank, delete (admin)
- [x] Registry channels (`stable`, `experimental`, …) — `?channel=` on all package endpoints
- [x] Prebuilt binary tarballs — upload/download by target triple; `?arch=&os=&backend=` filter on list
- [x] Per-package README — extracted from tarball on publish, served at `GET /api/v1/packages/:name/readme`
- [x] Per-version dependencies — extracted from `freight.toml` inside tarball, returned in package metadata
- [x] User accounts, API tokens with scopes, TOTP/2FA, refresh tokens, email verification, password reset
- [x] Package ownership, org/team accounts, multi-owner management
- [x] Per-IP rate limiting, login lockout, audit log with TTL pruning
- [x] S3-compatible storage backend, Prometheus metrics, health endpoint
- [x] Dockerfile, Docker Compose, systemd unit file
- [x] Mirror/proxy mode — transparent upstream fallback for unknown packages

### Language Server (in progress — `feature/lsp-server`)
- [x] Crate scaffold: `crates/freight-lsp/` (lib + bin), `tower-lsp 0.20`, stdio transport
- [x] Document store backed by `DashMap<Url, String>` — full-sync updates
- [x] Diagnostics via `freight`'s `validate()` + `validate_dep_compat()`
- [x] Completion: section-aware (section headers, `backend`, `warnings`, `std`, `lib.type`, field snippets)
- [x] Hover docs keyed by dotted path (`compiler.backend`, `lib.type`, …)
- [x] Go-to-definition for `path = "..."` dependencies
- [x] `freight lsp` CLI subcommand
- [ ] VS Code extension that activates on `freight.toml`
- [ ] Inlay hints showing resolved compiler flags per profile
- [ ] Code actions: "add `[[bin]]` target", "convert version dep → detailed table"

### Examples ✓ COMPLETE
- [x] `c/hello/` — pure C hello world
- [x] `cpp/hello/` — multi-file C++ hello world
- [x] `cpp/static-lib/` — path dependency/static library pattern
- [x] `cpp/multi-bin/` — multiple binaries from one source tree
- [x] `cpp/modules/` — C++20 named modules
- [x] `mixed/c-cpp/` — C + C++ mixed project
- [x] `mixed/tri-lang/` — Fortran + C + C++ N-body gravity
- [x] `assembly/hello/` — C + NASM/GAS assembly
- [x] `deps/cmake/` — foreign CMake dep (auto-detected)
- [x] `deps/make/` — foreign Make dep (auto-detected)
- [x] `deps/git/` — git dependency cloned and built automatically
- [x] `misc/doc/` — C, C++, Fortran sources with LaTeX math in doc comments
