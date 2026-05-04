# Freight — Build Tool & Package Manager

## What is freight?

Freight is a Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang: C, C++, Fortran, assembly, CUDA, HIP, OpenCL, and others. It aims to be the single tool you need to build, test, and publish native code — no Makefile, no CMake, no Ninja required.

The project is written in Rust.

---

## Core philosophy

- **No external build system** — freight owns the entire build graph internally. No Ninja, no Make underneath.
- **Declarative compiler templates** — each compiler (gcc, clang, nvcc, gfortran, nasm…) is described in a `.rhai` file that maps abstract settings to real flags. Adding a new compiler = writing a Rhai script, not writing Rust.
- **One tool, many languages** — file extension routes to the right compiler automatically. A single project can mix `.cpp`, `.c`, `.f90`, `.asm`, `.cu` files.
- **Incremental by default** — mtime dirty checking via Makefile `.d` dep files (source + all included headers), parallel compilation via rayon.
- **C++20 modules supported** — scanner detects `export module` / `import` declarations, builds a dependency DAG, compiles MIUs in topological order (parallel within each level), then compiles the rest in parallel with `-fmodule-file=` flags injected per import.

---

## Naming conventions

| Name | Meaning |
|---|---|
| `freight` | The CLI binary |
| `freight.toml` | Project manifest |
| `freight.lock` | Auto-generated lockfile (commit this) |
| `build.freight` | Optional pre-build hook script |
| `~/.freight/` | Global cache directory |
| `freight.dev` | The package registry — not yet implemented |

---

## Repository layout

```
crane/                              # repo root (git)
├── Cargo.toml                      # workspace root
├── CLAUDE.md                       # this file
├── vendors/                        # runtime arch/os/compiler token database
│   ├── x86_64.toml                 # kind = "arch", aliases = ["amd64", ...]
│   ├── linux.toml                  # kind = "os"
│   ├── gnu.toml                    # kind = "compiler", aliases = ["gnueabi", ...]
│   └── ...                         # one .toml per arch/os/compiler family
├── toolchains/                     # bundled .rhai compiler templates
│   ├── gcc.rhai
│   ├── clang.rhai
│   ├── gfortran.rhai
│   ├── nasm.rhai
│   ├── nvcc.rhai
│   ├── msvc.rhai
│   └── ...                         # one per compiler
├── crates/
│   ├── freight/                    # binary crate — CLI shells + clap dispatch
│   │   └── src/
│   │       ├── main.rs             # clap parse → commands::* dispatch
│   │       ├── output.rs           # coloured print helpers (CLI-only)
│   │       └── commands/           # one cmd_* shell per command, calls into freight-core
│   │           ├── mod.rs
│   │           ├── build.rs        # cmd_build, cmd_run, cmd_test, cmd_clean, cmd_watch
│   │           ├── check.rs        # cmd_check + manifest summary printer
│   │           ├── compile_commands.rs  # cmd_compile_commands
│   │           ├── debug.rs        # cmd_debug
│   │           ├── deps.rs         # cmd_add, remove, update, fetch, tree, search, info, login, publish, yank
│   │           ├── doc.rs          # cmd_doc, cmd_man
│   │           ├── install.rs      # cmd_install, cmd_package
│   │           ├── migrate.rs      # cmd_migrate
│   │           ├── new.rs          # cmd_new, cmd_init
│   │           └── toolchain.rs    # cmd_toolchain_list, cmd_toolchain_add
│   ├── freight-core/               # library crate — all build logic, no CLI / no printing of results
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs
│   │       ├── new.rs              # scaffold_project / init_project (returns ScaffoldOutcome)
│   │       ├── dep_cmds.rs         # manifest_add_dep, manifest_remove_dep, regen_lock, locate_project
│   │       ├── install.rs          # install_project, package_project
│   │       ├── lock.rs             # freight.lock read/write
│   │       ├── vendor.rs           # VendorDb, parse_triple, global_db()
│   │       ├── manifest/           # freight.toml parsing + validation
│   │       │   ├── mod.rs
│   │       │   ├── types.rs
│   │       │   ├── find.rs
│   │       │   └── validate.rs
│   │       ├── toolchain/          # compiler detection + templates
│   │       │   ├── mod.rs
│   │       │   ├── template.rs
│   │       │   ├── detect.rs
│   │       │   └── cache.rs
│   │       └── build/              # compilation + linking orchestration
│   │           ├── mod.rs          # build_project, clean_project, test_project (pub functions)
│   │           ├── compile.rs      # source → object, parallel via rayon
│   │           ├── deps.rs         # dep graph resolution + topo sort + slot conflict check
│   │           ├── discover.rs     # walkdir source discovery
│   │           ├── features.rs     # feature resolution, dep: activation, to_defines()
│   │           ├── foreign.rs      # CMake/Make/Meson/SCons/Autotools foreign dep builds
│   │           ├── header_units.rs # C++20 header unit precompilation
│   │           ├── http.rs         # URL dep download + sha256 verify
│   │           ├── link.rs         # object → binary / .a / .so
│   │           ├── modules.rs      # C++20 module scanner, DAG, phased compilation
│   │           └── script.rs       # build.freight script runner
│   ├── freight-doc/                # doc extraction and site generation
│   ├── freight-migrator/           # library crate — freight migrate (CMake/Makefile/Meson → freight.toml)
│   │   └── src/
│   │       ├── lib.rs              # run_migrate → MigrateOutcome, ImportedProject IR
│   │       ├── detect.rs           # pick format from files present
│   │       ├── emit.rs             # ImportedProject → freight.toml string
│   │       ├── cmake.rs            # CMakeLists.txt parser
│   │       ├── makefile.rs         # Makefile parser
│   │       └── meson.rs            # meson.build parser
│   └── freight-lsp/                # Language Server for freight.toml
│       └── src/
│           ├── lib.rs
│           ├── completion.rs
│           ├── docs.rs
│           └── position.rs
└── examples/                       # every example is buildable — `cd <dir> && freight build`
    ├── hello-cpp/
    ├── multi-lang/
    ├── with-deps/
    ├── c-simple/
    ├── multi-bin/
    ├── cpp-modules/
    ├── tri-lang/
    ├── with-build-script/
    ├── with-cmake-dep/
    ├── with-git-dep/
    └── migrated-from-cmake/
```

---

## freight.toml — manifest format

```toml
[package]
name        = "myproject"
version     = "0.1.0"
authors     = ["You <you@example.com>"]
description = "A short description"
license     = "MIT"
# Virtual slots this package fills — used for conflict detection.
# If two active deps declare the same slot, freight errors before compilation.
provides    = ["blas"]   # e.g. openblas and mkl both provide "blas"

# Per-language settings; key matches the template's language identifier
[language.cpp]
std = "c++20"

[language.c]
std = "c17"

[language.fortran]
# std is optional for Fortran

[lib]
type    = "static"   # static | shared | header-only
src     = "src/"
inc     = "include/" # public include directory (alias: include)

[[bin]]
name = "myproject"
src  = "src/main.cpp"

[dependencies]
# Version deps are fetched from freight.dev (not yet implemented)
libopenblas = "0.3"
# System deps link against a system-installed library
openssl     = { system = "openssl", version = ">=3.0" }
# Path deps compile a sibling freight project and link its archive
myutils     = { path = "../myutils" }
# Optional deps — only compiled when activated via a feature
openblas    = { path = "deps/openblas", optional = true }
mkl         = { path = "deps/mkl",      optional = true }
# Target-filtered dep — only linked when cross-compiling to that triple
arm-hal     = { path = "../arm-hal", targets = ["aarch64-linux-gnu"] }
# OS-filtered deps — only linked on matching host OS (accepts string or array)
pthread     = { system = "pthread", os = "linux" }
ws2_32      = { system = "ws2_32",  os = "windows" }
libm        = { system = "m",       os = ["linux", "macos"] }
# Arch-filtered dep
sse-util    = { path = "../sse-util", arch = "x86_64" }
# Combined: OS + arch filter (both must match)
avx-opt     = { system = "avx-opt", os = "linux", arch = ["x86_64", "aarch64"] }

[dev-dependencies]
libcheck = "0.15"

# Features: each key maps to a list of other features it implies.
# Active features produce -D<NAME_UPPER> compiler flags for all sources.
# "default" is a special list of features active when none are requested explicitly.
# dep:name entries activate optional deps instead of producing a define.
[features]
default  = ["openblas"]        # active by default; does NOT produce -DDEFAULT
logging  = []                  # → -DLOGGING when active
tls      = ["net"]             # → -DTLS + activates "net"
net      = []                  # → -DNET
openblas = ["dep:openblas"]    # activates the optional openblas dep
mkl      = ["dep:mkl"]         # activates the optional mkl dep

[compiler]
backend   = "auto"   # auto | gcc | clang | gfortran | nasm | …
opt-level = 2
debug     = false
warnings  = "all"    # none | default | all | error
defines   = ["USE_BLAS"]
flags     = []
target    = "aarch64-linux-gnu"   # optional cross-compilation target triple
sysroot   = "/opt/sysroot"        # optional sysroot path

[compiler.includes]
paths = ["include/", "third_party/include/"]

[profile.dev]
opt-level = 0
debug     = true
sanitize  = ["address", "undefined"]

[profile.release]
opt-level = 3
lto       = true
strip     = true
debug     = false
features  = ["mkl"]   # profile-level features: release builds use MKL

# Per-platform overlays — keyed by host OS or family alias.
# Recognized keys: linux, windows, macos, freebsd, openbsd, netbsd, dragonfly,
# android, ios, solaris, illumos, plus the family aliases `unix` (everything
# except windows) and `bsd` (the BSDs). Family overlays are applied first,
# then the specific OS — so a Linux build picks up [platform.unix] then
# [platform.linux].
[platform.linux.dependencies]
dl      = { system = "dl" }
pthread = { system = "pthread" }

[platform.windows.dependencies]
ws2_32  = { system = "ws2_32" }

[platform.windows.compiler]
defines = ["WIN32_LEAN_AND_MEAN"]

[platform.unix.compiler]
defines = ["POSIX_BUILD"]
```

---

## Build engine — internal pipeline

```
freight build
  │
  ├── 1. Parse + validate freight.toml
  ├── 2. Detect toolchain (probe $PATH, load compiler templates, version cache)
  ├── 3. Resolve features (dep: entries activate optional deps, profile features merged)
  ├── 4. Resolve dependency graph (topo sort, flat .deps/ pool, slot conflict check)
  │       ├── compile each dep → archive (.a)
  │       └── collect dep include dirs
  ├── 5. Walk src/ — discover sources by file extension → language key
  ├── 6. Scan C++ sources for `export module` / `import` declarations
  │       ├── [no modules] → flat parallel compile (step 7a)
  │       └── [modules found] → module-aware pipeline (step 7b)
  ├── 7a. Flat: dirty-check + compile all sources in parallel (rayon)
  ├── 7b. Module-aware:
  │       ├── topo-sort MIUs into batches (Kahn's algorithm)
  │       ├── for each batch: compile MIUs in parallel → produce .pcm + .o
  │       │     GCC: one pass with -fmodule-output=
  │       │     Clang: --precompile → .pcm, then -c → .o
  │       └── compile MImplUs + regular TUs in parallel with -fmodule-file= per import
  └── 8. Link all .o + dep .a files → binary / .a / .so
          (each [[bin]] only links its own entry-point .o, not other bins')
```

---

## Dependency model

| Kind | freight.toml syntax | How it works |
|---|---|---|
| Path | `{ path = "../mylib" }` | Compiles the dep project, links its `.a` archive |
| System | `{ system = "openssl" }` | Passes `-l{name}` to the linker |
| Version | `"0.3"` | Fetched from freight.dev (not yet implemented) |
| Git | `{ git = "..." }` | Cloned to `.deps/{name}/` by `freight fetch` |
| URL | `{ url = "https://..." }` | Downloaded + extracted to `.deps/{name}/` |
| Foreign | `{ path = "...", build_system = "cmake" }` | Delegates to CMake/Make/Meson/SCons/Autotools |
| Optional | `{ path = "...", optional = true }` | Only compiled when activated via a `dep:name` feature |

All deps — including transitive ones — live in the **root project's flat `.deps/` pool**.
Version/git deps always resolve to `{root}/.deps/{name}/`. Path deps are relative to the
manifest that declares them. The topo sort ensures deps are compiled in the right order.

### Slot conflict detection

A package can declare `provides = ["blas"]` in its `[package]`. If two active deps fill the
same slot, freight errors before compilation:

```
error: slot conflict — 'openblas' and 'mkl' both provide 'blas'
       only one provider per slot may be active
```

Use optional deps + features to select one provider at a time.

---

## CLI commands

```
freight new <name> [--lang <lang>]         scaffold a new project              ✓ implemented
freight init [--lang <lang>]               init freight in current directory   ✓ implemented
freight build [--release] [--features F]   build the project                   ✓ implemented
freight run [--release] [-- <args>]        build and run default binary        ✓ implemented
freight test [<name>] [--release]          build and run tests                 ✓ implemented
freight clean                              wipe target/                        ✓ implemented
freight check                              validate freight.toml               ✓ implemented
freight watch [--release]                  rebuild on file changes             ✓ implemented
freight debug [<binary>] [--debugger D]    launch interactive debugger         ✓ implemented
freight compile-commands [--release]       generate compile_commands.json      ✓ implemented
freight doc [--format html|md|latex|pdf]   generate documentation site         ✓ implemented
freight man [--out-dir DIR]                generate man pages                  ✓ implemented

freight add <name>[@ver] [--path P] [--system] [--dev]  add a dependency      ✓ implemented
freight remove <package>                   remove a dependency                 ✓ implemented
freight update [<package>]                 refresh lockfile for path deps      ✓ implemented (registry pending)
freight fetch                              verify/download deps                ✓ implemented (registry pending)
freight tree                               print dependency tree               ✓ implemented
freight info <package>                     show package metadata               ✗ Phase 12 (registry server)
freight search <query>                     search freight.dev                  ✗ Phase 12 (registry server)
freight migrate [--from <format>] [--dry-run] [--force]  import existing build system  ✓ implemented
freight install [--prefix P] [--destdir D] [--target T]  install to system    ✓ implemented
freight package [--target TRIPLES]         build redistributable tar.gz        ✓ implemented
freight login                              authenticate with freight.dev       ✗ Phase 12 (registry server)
freight publish                            upload package to registry          ✗ Phase 12 (registry server)
freight yank <version>                     yank a published version            ✗ Phase 12 (registry server)
freight toolchain list                     show detected compilers             ✓ implemented
freight toolchain add <path>               install a compiler template         ✓ implemented
freight toolchain use <name>               set default compiler backend        ✗ deferred
freight lsp                                run language server on stdio        ✓ implemented
```

---

## Development roadmap

### Phase 1 — CLI skeleton ✓ COMPLETE
- [x] Cargo workspace: `freight` (bin) + `freight-core` (lib)
- [x] `clap` wiring — all subcommands stubbed
- [x] `FreightError` enum with `thiserror`
- [x] Coloured output helpers: success `✓`, warning `⚠`, error `✗`
- [x] `freight new <name> --lang <lang>` — scaffold directory + freight.toml + hello-world src
- [x] `freight init [--lang <lang>]` — init in current dir, auto-detects language from existing files

### Phase 2 — Manifest ✓ COMPLETE
- [x] Serde structs for every freight.toml section (`manifest/types.rs`)
- [x] Parse + validate with `toml_edit`
- [x] `freight check` — validate manifest, print clear errors or a summary
- [x] `find_manifest_dir` — walk up the directory tree to locate `freight.toml`
- [x] `Manifest::build_settings_for(profile)` — convert manifest + profile into `BuildSettings`
- [x] ABI compatibility validation for path dependencies
- [x] C/C++ standard consistency validation

### Phase 3 — Compiler detection ✓ COMPLETE
- [x] Probe `$PATH` for known compiler binaries
- [x] Load + deserialize compiler template `.rhai` files at runtime from `toolchains/`
- [x] `CompilerTemplate` struct + `assemble_flags()` method (pure, unit-tested)
- [x] `freight toolchain list`
- [x] Toolchain version cache (`~/.freight/toolchain-cache.json`, mtime-validated)
- [x] Templates: gcc, clang, gfortran, gnat, dmd, nvcc, hipcc, icpx, opencl, ispc, nasm, msvc, zig, swift, odin, and more

### Phase 4 — Build engine ✓ COMPLETE
- [x] Source discovery with `walkdir` — extension → language key routing
- [x] Parallel compilation via `rayon`
- [x] Mtime dirty checking — source vs object, headers via `.d` dep files
- [x] Linker invocation — binary, static lib (`.a`), shared lib (`.so`)
- [x] `freight build` + `freight run` end-to-end
- [x] `freight test` — compiles test files, links against project objects (excluding `main()`), runs each test binary
- [x] `freight clean` — wipes `target/`
- [x] Multi-language builds — C + C++ in one project, each compiled with the right binary
- [x] Multi-bin fix — each `[[bin]]` links only its own entry-point object
- [x] Compiler warnings/notes always forwarded to stderr; compiler command only shown with `--verbose`

### Phase 5 — Dependencies ✓ COMPLETE
- [x] Path dependency resolution — compile dep, archive to `.a`, link into project
- [x] System dependency linking — `{ system = "..." }` → `-l{name}`
- [x] Dependency graph with topological sort (Kahn's algorithm)
- [x] Cycle detection with error
- [x] Flat `.deps/` pool — all deps (including transitive) resolve from the root project's `.deps/`
- [x] Dep include dirs accumulated in topo order for multi-level dep builds

### Phase 6 — Assembly + target config ✓ COMPLETE
- [x] NASM template — `.asm`/`.nasm`, x86/x86_64 arch flags
- [x] GAS template — `.s`, x86/x86_64/aarch64 arch flags
- [x] `[target]` section in freight.toml — `arch` and `cpu_extensions`
- [x] `arch` drives `[arch_flags]` lookups in templates
- [x] `cpu_extensions` produces per-extension flags (e.g. `-mavx2`, `-mfma`)

### Phase 7 — Examples ✓ COMPLETE
- [x] `examples/hello-cpp/` — multi-file C++ with tests
- [x] `examples/multi-lang/` — C + C++ mixed project with tests
- [x] `examples/with-deps/` — path dependency (static lib)
- [x] `examples/c-simple/` — pure C, Collatz benchmark
- [x] `examples/multi-bin/` — two binaries from one source tree
- [x] `examples/cpp-modules/` — C++20 named modules, ASCII ray tracer
- [x] `examples/tri-lang/` — Fortran + C + C++ N-body gravity
- [x] `examples/with-build-script/` — build.freight pre-build hook
- [x] `examples/with-cmake-dep/` — foreign CMake dependency
- [x] `examples/with-git-dep/` — git dependency

### Phase 8 — C++20 modules ✓ COMPLETE
- [x] Scan source files for `export module` / `import` statements
- [x] Classify files as MIU / MImplU / Regular TU
- [x] Build module DAG — Kahn's topo sort into parallel batches
- [x] Cycle detection with `DependencyCycle` error
- [x] GCC one-step MIU compilation: `-fmodule-output={pcm_path}`
- [x] Clang two-step MIU compilation: `--precompile` → `.pcm`, then `-c` → `.o`
- [x] Incremental: MIUs skipped when both `.o` and `.pcm` are up-to-date
- [x] Transparent activation — auto-detected from source content

### Phase 9 — Registry + lockfile (in progress)
- [x] `freight.lock` read/write — deterministic dep pinning (version 1 format, sha256 checksums)
- [x] `freight.lock` auto-generated on every `freight build`
- [x] `freight tree` — prints the dependency tree with dep type labels
- [x] `freight add` — manifest mutation + lock update
- [x] `freight remove` — removes dep from freight.toml + lock update
- [x] `freight update` — refreshes lockfile checksums for path deps
- [x] `freight fetch` — verifies path deps exist; warns for registry/git deps
- [x] `freight search / info` — stubs with "registry not yet available" message
- [x] `freight login / publish / yank` — stubs with "registry not yet available" message
- [ ] `freight fetch` — actually download version deps from freight.dev (needs Phase 12)
- [ ] `freight add` — resolve + lock exact version from freight.dev (needs Phase 12)

### Phase 10 — Cross-compilation ✓ COMPLETE
- [x] `[compiler] target` → `--target={triple}` via template; empty field = unsupported
- [x] `[compiler] sysroot` → `--sysroot={path}`
- [x] `targets = [...]` on any dep — filtered by active cross-compilation triple
- [x] `os = ...` / `arch = ...` dep filters — host OS and CPU architecture
- [x] `freight toolchain add <path>` — install custom compiler template
- [x] `freight install [--target TRIPLE]` — install to system prefix
- [x] `freight package [--target TRIPLES]` — produce redistributable tar.gz, multi-target
- [x] `vendors/` runtime database — arch/os/compiler tokens with aliases, loaded via `VendorDb`
- [x] `parse_triple` — derives `(arch, os)` from partial or full target triples, host fallback

### Phase 11 — Importer ✓ COMPLETE
- [x] `freight migrate [--from cmake|makefile|meson] [--dry-run] [--force]`
- [x] Auto-detection via presence of `CMakeLists.txt`, `Makefile`, or `meson.build`
- [x] CMake, Makefile, and Meson importers — parse into shared `ImportedProject` IR
- [x] `[platform.<os>]` manifest overlays round-tripped through the CMake `if(WIN32/LINUX/APPLE/UNIX)` recogniser
- [x] Foreign build system deps: `build_system = "cmake|make|meson|scons|autotools"`
- [x] URL deps with sha256 verification: `{ url = "https://...", sha256 = "..." }`
- [x] `--dry-run` prints generated `freight.toml` without writing

### Phase 12 — Features + optional deps ✓ COMPLETE
- [x] `[features]` — Cargo-style feature definitions with transitive expansion
- [x] `dep:name` in feature lists — activates optional deps, does not produce a define
- [x] `optional = true` on deps — excluded from build unless activated via `dep:name`
- [x] `default-features = false` on a dep — opt out of the dep's default feature set
- [x] `features = [...]` on a dep — activate specific features of a dep
- [x] `features = [...]` in `[profile.dev]` / `[profile.release]` — profile-level feature selection
- [x] `provides = [...]` in `[package]` — slot declaration for conflict detection
- [x] Slot conflict error before compilation when two active deps fill the same slot

### Phase 13 — Registry server (planned)
New workspace crate `crates/freight-registry/` implementing freight.dev. Filesystem-backed
for v1; storage backend is swappable later. Unblocks the outstanding Phase 9 items.

- [ ] Axum-based HTTP server bound to `FREIGHT_REGISTRY_ADDR` (default `0.0.0.0:8080`)
- [ ] Filesystem layout: `registry-data/index/<name>.json` + `registry-data/packages/<name>/<version>.tar.gz`
- [ ] `GET /api/v1/packages/{name}` — versions + metadata
- [ ] `GET /api/v1/packages/{name}/{version}/download` — stream the `.tar.gz`
- [ ] `GET /api/v1/search?q=<query>` — prefix + substring match
- [ ] `POST /api/v1/publish` (bearer auth) — accept tarball + manifest
- [ ] `POST /api/v1/yank` (bearer auth) — mark a version yanked
- [ ] Static bearer tokens in `registry-data/tokens.toml` for v1; JWT/OAuth deferred
- [ ] Client: `FREIGHT_REGISTRY_URL` env var; credentials at `~/.freight/credentials.toml`
- [ ] Wire Phase 9 stubs to the real HTTP API
- [ ] Integration tests: spin up server on ephemeral port, exercise publish → fetch → build

### Phase 14 — Language server (in progress)
LSP for `freight.toml`, built on `tower-lsp` + `tokio`. Invokable as `freight lsp`.

- [x] Crate scaffold: `crates/freight-lsp/` (lib + bin), stdio transport
- [x] Document store backed by `DashMap<Url, String>`
- [x] Diagnostics via `freight-core`'s `validate()` + `validate_dep_compat()`
- [x] Completion — section headers, `backend`, `warnings`, `std` values, field snippets
- [x] Hover docs — Markdown descriptions keyed by dotted path
- [x] Go-to-definition for `path = "..."` dependencies
- [x] `freight lsp` CLI subcommand
- [ ] VS Code extension that activates on `freight.toml`
- [ ] Inlay hints showing resolved compiler flags per profile
- [ ] Code actions: "add `[[bin]]` target", "convert simple version dep → detailed table"

---

## Backburner (deferred, not forgotten)

- **`freight toolchain use <name>`** — set default compiler backend globally; deferred, low demand
- **Slot-based substitution** — `provides` currently only detects conflicts; auto-routing a dep request to a compatible provider (e.g. root has `mkl`, sub-dep requests `openblas`, both provide `blas`) is complex and deferred to Phase 13+
- **Progress callbacks** — build output currently goes to stdout via `println!`; routing through a callback for GUI/TUI frontends is future work
- **Per-language `[platform]` overlays** — `[platform.linux.language.cpp]` deliberately excluded from v1
- **JWT/OAuth for registry** — v1 uses static bearer tokens only
- **Git dep recursive fetch** — freight intentionally does not fetch transitively; user manages `.deps/` manually

---

## Architecture rules

1. **`freight` crate owns the CLI** — clap parsing, `commands/` shells, and `output.rs` colour helpers. Each `cmd_*` reads cwd, calls a pure function in `freight-core`, prints the outcome.
2. **`freight-core` is a library, no CLI knowledge** — pure functions return `Result<T, FreightError>`. It must not depend on `output.rs` or call `print_*`. Inline `println!` for build-engine progress is the one exception, pending a future progress-callback abstraction.
3. **`freight-migrator` is a separate library** — depends on `freight-core` for `FreightError`, exposes `run_migrate → MigrateOutcome`.
4. **Compiler templates are runtime data** — loaded from `toolchains/` directory as `.rhai` files, not hardcoded in Rust.
5. **Vendor database is runtime data** — arch/os/compiler tokens loaded from `vendors/*.toml` at startup via `VendorDb`; adding a new target = writing a TOML.
6. **One template per toolchain, not per language** — `gcc.rhai` handles both C and C++; `compile_binary` in `[linking.c]` overrides which binary compiles that language.
7. **DAG cycles = hard error** — report the full cycle path (both dep cycles and module cycles).
8. **Flat dep pool** — all deps resolve from the root project's `.deps/`; no nested `.deps/` inside deps.
9. **`CompilerTemplate::assemble_flags()` is pure** — no side effects, unit-tested.
10. **Never shell out to Make / Ninja / CMake during a build** — freight owns the build graph entirely (foreign deps are the explicit exception).
11. **Errors use `thiserror` in freight-core, surface at the CLI boundary.**
12. **Feature branches** — each new feature gets its own `feature/<name>` branch off `master`.
13. **Module detection is transparent** — `build_sources()` scans automatically; projects without `export module` take the unchanged fast path.

---

## Key Rust dependencies

```toml
[dependencies]
clap          = { version = "4", features = ["derive"] }
clap_mangen   = "0.2"
owo-colors    = "4"
indicatif     = "0.17"
toml_edit     = { version = "0.22", features = ["serde"] }
serde         = { version = "1", features = ["derive"] }
serde_json    = "1"
rmp-serde     = "1"           # MessagePack serialisation (freight doc --format msgpack)
pulldown-cmark = "0.12"       # Markdown parsing (freight doc)
rayon         = "1"
walkdir       = "2"
regex         = "1"
sha2          = "0.10"
git2          = "0.19"
semver        = "1"
anyhow        = "1"
thiserror     = "2"
tower-lsp     = "0.20"
tokio         = { version = "1", features = ["rt-multi-thread", "io-std", "macros"] }
dashmap       = "6"
rhai          = "1"
notify        = "6"
```
