# Development Roadmap

Feature branches follow the convention `feature/<name>` off `master`.

---

### CLI Bootstrap ✓ COMPLETE
- [x] Cargo workspace: `crane` (bin) + `crane-core` (lib)
- [x] `clap` wiring — all subcommands stubbed
- [x] `CraneError` enum with `thiserror`
- [x] Coloured output helpers: success `✓`, warning `⚠`, error `✗`
- [x] `crane new <name> --lang <lang>` — scaffold directory + crane.toml + hello-world src
- [x] `crane init [--lang <lang>]` — init in current dir, auto-detects language from existing files

### Manifest ✓ COMPLETE
- [x] Serde structs for every crane.toml section (`manifest/types.rs`)
- [x] Parse + validate with `toml_edit`
- [x] `crane check` — validate manifest, print clear errors or a summary
- [x] `find_manifest_dir` — walk up the directory tree to locate `crane.toml`
- [x] `Manifest::build_settings_for(profile)` — convert manifest + profile into `BuildSettings`
- [x] ABI compatibility validation for path dependencies
- [x] C/C++ standard consistency validation

### Compiler Detection ✓ COMPLETE
- [x] Probe `$PATH` for known compiler binaries
- [x] Load + evaluate compiler template `.rhai` scripts at runtime
- [x] `CompilerTemplate` struct + `assemble_flags()` method (pure, unit-tested)
- [x] `crane toolchain list`
- [x] Toolchain version cache (`~/.crane/toolchain-cache.json`, mtime-validated)
- [x] 19 bundled toolchain scripts: gcc, clang, nasm, gfortran, gnat, nvcc, dmd, hipcc, icpx, opencl, ispc, tcc, nvhpc, ifx, flang, ldc2, yasm, circle, msvc
- [x] gcc and clang scripts probe versioned binaries (`g++-14`, `clang++-17`, …) as fallbacks

### Build Engine ✓ COMPLETE
- [x] Source discovery with `walkdir` — extension → language key routing
- [x] Parallel compilation via `rayon`
- [x] Mtime dirty checking — source vs object, headers via `.d` dep files
- [x] `.d` dep file generation (`-MMD -MF`) for transitive header tracking; stdout mode (`/showIncludes`) for MSVC
- [x] Linker invocation — binary, static lib (`.a`), shared lib (`.so`)
- [x] `crane build` + `crane run` end-to-end
- [x] `crane test` — compiles test files, links against project objects (excluding `main()`), runs each test binary
- [x] `crane clean` — wipes `target/`
- [x] Multi-language builds — C + C++ in one project, each compiled with the right binary
- [x] Multi-bin fix — each `[[bin]]` links only its own entry-point object
- [x] Toolset roles — `output_obj`/`output_bin` split, `lto_link` flag category, `system_lib` format string
- [x] MSVC support — `/Fo`, `/Fe`, `/GL` + `/LTCG`, `{name}.lib` system libs, `/showIncludes` dep tracking

### Dependencies ✓ COMPLETE
- [x] Path dependency resolution — compile dep, archive to `.a`, link into project
- [x] System dependency linking — `{ system = "..." }` → `-l{name}` (or `{name}.lib` for MSVC)
- [x] Dependency graph with topological sort (Kahn's algorithm)
- [x] Cycle detection with error
- [x] `.deps/<name>/` folder convention for version-pinned deps
- [x] Transitive dep checks — errors if a dep's dep is not present, does not fetch recursively
- [x] Dep include dirs accumulated in topo order for multi-level dep builds

### Foreign Build System Integration ✓ COMPLETE
- [x] Auto-detect foreign build system from dep directory — CMake > Meson > Autotools > SCons > Make
- [x] CMake, Meson, Make, Autotools, SCons foreign deps: configure → build → install
- [x] Git dependencies — `{ git = "https://..." }` clones into `.deps/<name>/`, then treated as path dep
- [x] Foreign dep include + archive auto-discovery after build
- [x] HTTP tarball deps — `{ http = "...", sha256 = "..." }` with SHA-256 verification
- [x] GitHub release deps — `{ github = "owner/repo", tag = "v1.0" }` shorthand
- [x] Download sentinel — `.deps/<name>/.crane-fetched` prevents re-downloading
- [x] pkg-config deps — standalone or with system fallback
- [x] `build_system = "none"` explicit header-only override
- [x] Header-only auto-detection when no build system and no source files found

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
- [x] GAS (AT&T assembly) via GCC/Clang — `.s`/`.S` in `[linking.c]` extensions
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
- [x] `crane toolchain add <path>` — validates and installs a local `.rhai` script

### Migrator ✓ COMPLETE
- [x] `crane migrate [--from cmake|makefile|meson] [--dry-run] [--force]`
- [x] Auto-detection of source build system
- [x] CMake, Makefile, Meson importers — all parse to shared `ImportedProject` IR
- [x] `emit::to_toml` serializes to `crane.toml` with stable output ordering
- [x] Platform guards routed to `[platform.<os>]` overlays
- [x] `find_package()` → `{ system = "..." }` dep with review comment
- [x] `--dry-run` prints generated `crane.toml` to stdout
- [x] `examples/migrated-from-cmake/` — before/after worked example

### Documentation Generator ✓ COMPLETE
- [x] `doc/extract.rs` — line-scanner extractor for C/C++, Rust, Fortran, D, Ada
- [x] `doc/markdown.rs` — math protection + MD→HTML + MD→LaTeX via pulldown-cmark
- [x] `doc/render.rs` — HTML renderer with MathJax 3 CDN
- [x] `doc/render_md.rs` — GFM Markdown renderer with per-file pages and index
- [x] `doc/render_latex.rs` — LaTeX renderer + PDF via xelatex/pdflatex
- [x] `crane doc [--format html|md|latex|pdf|all]`
- [x] `crane man [--out-dir DIR]` — man pages via clap_mangen
- [x] `crates/crane-doc/` — standalone `crane-doc` binary
- [x] `examples/doc-example/` — C, C++, Fortran sources with LaTeX math in comments

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
- [x] `DebuggerTemplate` struct — `name`, `binary`, `[launch]` separator, `[dap]` config
- [x] `detect_debuggers()` — probes `$PATH`, extracts version, finds DAP adapter binary
- [x] `toolchains/debuggers/lldb.toml` and `toolchains/debuggers/gdb.toml`
- [x] `crane toolchain list` — second table section for debuggers
- [x] `crane debug [<binary>] [--debugger <name>] [-- <args>]` — builds with debug profile, execs debugger
- [x] `crane debug --launch-json` — writes/merges `.vscode/launch.json`

### Registry (in progress — `feature/registry-lockfile`, `feature/registry-server`)
The registry spans two concerns that depend on each other: the client-side
lockfile + CLI stubs (`crane add`, `crane fetch`, …) and the server that
backs them. Server implementation unblocks the remaining client stubs.

**Client (crane-core)**
- [x] `crane.lock` read/write — deterministic dep pinning (version 1 format, sha256 checksums)
- [x] `crane.lock` auto-generated on every `crane build`
- [x] `crane tree` — dependency tree with dep type labels
- [x] `crane add` / `crane remove` — manifest mutation + lock update
- [x] `crane update [package]` — refreshes lockfile checksums for path deps
- [x] `crane fetch` — verifies path deps exist
- [x] `crane search / info / login / publish / yank` — stubs pending crane.dev
- [ ] `crane fetch` — download version deps from crane.dev
- [ ] `crane add` — resolve + lock exact version from crane.dev

**Server (crates/crane-registry/)**
- [ ] Axum-based HTTP server (`CRANE_REGISTRY_ADDR`, default `0.0.0.0:8080`)
- [ ] Filesystem layout: `registry-data/index/<name>.json` + `registry-data/packages/<name>/<version>.tar.gz`
- [ ] `GET /api/v1/packages/{name}` — versions + metadata
- [ ] `GET /api/v1/packages/{name}/{version}/download`
- [ ] `GET /api/v1/search?q=<query>`
- [ ] `POST /api/v1/publish` (bearer auth)
- [ ] `POST /api/v1/yank` (bearer auth)
- [ ] Static bearer tokens in `registry-data/tokens.toml` for v1
- [ ] `CRANE_REGISTRY_URL` env var; credentials at `~/.crane/credentials.toml`
- [ ] Wire CLI stubs to the real HTTP API
- [ ] Integration tests: spin up on an ephemeral port, publish → fetch → build

### Language Server (in progress — `feature/lsp-server`)
- [x] Crate scaffold: `crates/crane-lsp/` (lib + bin), `tower-lsp 0.20`, stdio transport
- [x] Document store backed by `DashMap<Url, String>` — full-sync updates
- [x] Diagnostics via `crane-core`'s `validate()` + `validate_dep_compat()`
- [x] Completion: section-aware (section headers, `backend`, `warnings`, `std`, `lib.type`, field snippets)
- [x] Hover docs keyed by dotted path (`compiler.backend`, `lib.type`, …)
- [x] Go-to-definition for `path = "..."` dependencies
- [x] `crane lsp` CLI subcommand
- [ ] VS Code extension that activates on `crane.toml`
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
- [x] `migrated-from-cmake/` — before/after for `crane migrate --from cmake`
- [x] `doc-example/` — C, C++, Fortran sources with LaTeX math in doc comments
