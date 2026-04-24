# Crane — Build Tool & Package Manager

## What is crane?

Crane is a Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang: C, C++, Fortran, assembly, CUDA, HIP, OpenCL, and others. It aims to be the single tool you need to build, test, and publish native code — no Makefile, no CMake, no Ninja required.

The project is written in Rust.

---

## Core philosophy

- **No external build system** — crane owns the entire build graph internally. No Ninja, no Make underneath.
- **Declarative compiler templates** — each compiler (gcc, clang, nvcc, gfortran, nasm…) is described in a `.toml` file that maps abstract settings to real flags. Adding a new compiler = writing a TOML, not writing Rust.
- **One tool, many languages** — file extension routes to the right compiler automatically. A single project can mix `.cpp`, `.c`, `.f90`, `.asm`, `.cu` files.
- **Incremental by default** — mtime dirty checking via Makefile `.d` dep files (source + all included headers), parallel compilation via rayon.
- **C++20 modules supported** — scanner detects `export module` / `import` declarations, builds a dependency DAG, compiles MIUs in topological order (parallel within each level), then compiles the rest in parallel with `-fmodule-file=` flags injected per import.

---

## Naming conventions

| Name | Meaning |
|---|---|
| `crane` | The CLI binary |
| `crane.toml` | Project manifest |
| `crane.lock` | Auto-generated lockfile (commit this) — not yet implemented |
| `build.crane` | Optional pre-build hook script — not yet implemented |
| `~/.crane/` | Global cache directory |
| `crane.dev` | The package registry — not yet implemented |

---

## Repository layout

```
crane/
├── Cargo.toml                  # workspace root
├── CLAUDE.md                   # this file
├── crates/
│   ├── crane/                  # binary crate — CLI shells + clap dispatch
│   │   └── src/
│   │       ├── main.rs         # clap parse → commands::* dispatch
│   │       ├── output.rs       # coloured print helpers (CLI-only)
│   │       └── commands/       # one cmd_* shell per command, calls into crane-core
│   │           ├── mod.rs
│   │           ├── build.rs    # cmd_build, cmd_run, cmd_test, cmd_clean
│   │           ├── check.rs    # cmd_check + manifest summary printer
│   │           ├── deps.rs     # cmd_add, remove, update, fetch, tree, search, info, login, publish, yank
│   │           ├── migrate.rs  # cmd_migrate
│   │           ├── new.rs      # cmd_new, cmd_init
│   │           └── toolchain.rs # cmd_toolchain_list
│   ├── crane-core/             # library crate — all build logic, no CLI / no printing of results
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs
│   │       ├── new.rs          # scaffold_project / init_project (returns ScaffoldOutcome)
│   │       ├── dep_cmds.rs     # manifest_add_dep, manifest_remove_dep, regen_lock, locate_project
│   │       ├── lock.rs         # crane.lock read/write
│   │       ├── manifest/       # crane.toml parsing + validation
│   │       │   ├── mod.rs
│   │       │   ├── types.rs
│   │       │   ├── find.rs
│   │       │   └── validate.rs
│   │       ├── toolchain/      # compiler detection + templates
│   │       │   ├── mod.rs
│   │       │   ├── template.rs
│   │       │   ├── detect.rs
│   │       │   └── cache.rs
│   │       ├── build/          # compilation + linking orchestration
│   │       │   ├── mod.rs      # build_project, clean_project, test_project (pub functions)
│   │       │   ├── compile.rs  # source → object, parallel via rayon
│   │       │   ├── link.rs     # object → binary / .a / .so
│   │       │   ├── discover.rs # walkdir source discovery
│   │       │   ├── deps.rs     # dep graph resolution + topo sort
│   │       │   └── modules.rs  # C++20 module scanner, DAG, phased compilation
│   ├── crane-importer/         # library crate — crane migrate (CMake/Makefile/Meson → crane.toml)
│   │   └── src/
│   │       ├── lib.rs          # run_migrate → MigrateOutcome, ImportedProject IR
│   │       ├── detect.rs       # pick format from files present
│   │       ├── emit.rs         # ImportedProject → crane.toml string
│   │       ├── cmake.rs        # CMakeLists.txt parser
│   │       ├── makefile.rs     # Makefile parser
│   │       └── meson.rs        # meson.build parser
│   └── crane-lsp/              # Language Server for crane.toml
│       └── src/
│           ├── lib.rs
│           ├── error.rs
│           ├── new.rs          # crane new / crane init
│           ├── manifest/       # crane.toml parsing + validation
│           │   ├── mod.rs
│           │   ├── types.rs
│           │   ├── find.rs
│           │   └── validate.rs
│           ├── toolchain/      # compiler detection + templates
│           │   ├── mod.rs
│           │   ├── template.rs
│           │   ├── detect.rs
│           │   └── cache.rs
│           └── build/          # compilation + linking orchestration
│               ├── mod.rs      # cmd_build, cmd_run, cmd_test, cmd_clean
│               ├── compile.rs  # source → object, parallel via rayon
│               ├── link.rs     # object → binary / .a / .so
│               ├── discover.rs # walkdir source discovery
│               ├── deps.rs     # dep graph resolution + topo sort
│               └── modules.rs  # C++20 module scanner, DAG, phased compilation
├── compiler-templates/         # bundled .toml files per compiler
│   ├── gcc.toml                # g++ (C++ linker), gcc (C compiler override)
│   ├── clang.toml              # clang++ (C++ linker), clang (C compiler override)
│   ├── gfortran.toml
│   ├── gnat.toml               # GNU Ada compiler
│   ├── dmd.toml                # D language compiler
│   ├── nvcc.toml
│   ├── hipcc.toml
│   ├── icpx.toml               # Intel oneAPI C++
│   ├── opencl.toml
│   └── ispc.toml               # Intel SPMD
└── examples/                   # every example is buildable — `cd <dir> && crane build`
    ├── hello-cpp/              # multi-file C++ with tests
    ├── multi-lang/             # C + C++ mixed, tests
    ├── with-deps/              # path dependency (static lib)
    ├── c-simple/               # pure C (Collatz benchmark)
    ├── multi-bin/              # two binaries from one source tree (base64 encode/decode)
    ├── cpp-modules/            # C++20 named modules (ASCII ray tracer)
    ├── tri-lang/               # Fortran + C + C++ in one project (requires gfortran)
    └── migrated-from-cmake/    # before/after for `crane migrate --from cmake`
```

---

## crane.toml — manifest format

```toml
[package]
name        = "myproject"
version     = "0.1.0"
authors     = ["You <you@example.com>"]
description = "A short description"
license     = "MIT"

# Per-language settings; key matches the template's [linking.<key>] name
[language.cpp]
std = "c++20"

[language.c]
std = "c17"

[language.fortran]
# std is optional for Fortran

[lib]
type    = "static"   # static | shared | header-only
src     = "src/"
include = "include/"

[[bin]]
name = "myproject"
src  = "src/main.cpp"

[dependencies]
# Version deps are fetched from crane.dev (not yet implemented)
libopenblas = "0.3"
# System deps link against a system-installed library
openssl     = { system = "openssl", version = ">=3.0" }
# Path deps compile a sibling crane project and link its archive
myutils     = { path = "../myutils" }
# Target-filtered dep — only linked when cross-compiling to that triple
arm-hal     = { path = "../arm-hal", targets = ["aarch64-linux-gnu"] }
# OS-filtered deps — only linked on matching host OS (accepts string or array)
# Supported keys: unix, linux, windows, macos, freebsd, bsd, and other platform keys
pthread     = { system = "pthread", os = "linux" }
ws2_32      = { system = "ws2_32",  os = "windows" }
# Multiple OS values accepted via array
libm        = { system = "m",       os = ["linux", "macos"] }
# Arch-filtered dep — only linked on matching CPU architecture
sse-util    = { path = "../sse-util", arch = "x86_64" }
# Combined: OS + arch filter (both must match)
avx-opt     = { system = "avx-opt", os = "linux", arch = ["x86_64", "aarch64"] }

[dev-dependencies]
libcheck = "0.15"

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

## Compiler template format

Each compiler is described by a flat `.toml` file — no `[compiler]` nesting. Crane loads all `.toml` files from `compiler-templates/` at startup. Adding a new compiler = writing a new TOML, not touching Rust.

```toml
# compiler-templates/gcc.toml

name          = "gcc"
binary        = "g++"          # binary used for linking
version_arg   = "--version"
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b"

[extensions]
handles = [".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"]

[flags]
opt.0            = "-O0"
opt.1            = "-O1"
opt.2            = "-O2"
opt.3            = "-O3"
debug.true       = "-g"
debug.false      = ""
warnings.none    = ""
warnings.default = "-Wall"
warnings.all     = "-Wall -Wextra -Wpedantic"
warnings.error   = "-Wall -Wextra -Wpedantic -Werror"
lto.true         = "-flto"
lto.false        = ""
strip.true       = "-s"
strip.false      = ""
sanitize         = "-fsanitize={values}"

[standards]
"c11"   = "-std=c11"
"c17"   = "-std=c17"
"c23"   = "-std=c23"
"c++17" = "-std=c++17"
"c++20" = "-std=c++20"
"c++23" = "-std=c++23"

[structure]
include_dir  = "-I{path}"
define       = "-D{name}"
define_value = "-D{name}={value}"
output       = "-o {path}"
compile_only = "-c"
dep_file     = "-MMD -MF {path}"   # generates Makefile dep file for header tracking
target       = "--target={triple}" # empty string = unsupported (e.g. GCC uses dedicated cross binary)
sysroot      = "--sysroot={path}"  # empty string = unsupported

[modules]
supported     = true
enable_flag   = "-fmodules-ts"
compile_miu   = "-fmodule-output={pcm_path}"   # GCC one-step: produces both .o and .pcm
import_module = "-fmodule-file={name}={pcm_path}"

[passthrough]
enabled = false
prefix  = ""

# A template can claim multiple language keys.
# [linking.<key>] declares ABI + linker compatibility for that language.
# compile_binary overrides the top-level binary for *compilation* only.
[linking.c]
abi            = "c"
compile_binary = "gcc"   # C files compiled with gcc, not g++
compatible     = ["fortran"]
linker         = ""
extensions     = [".c"]

[linking.cpp]
abi        = "c++"
compatible = ["c", "fortran"]
linker     = ""
extensions = [".cpp", ".cppm", ".cc", ".cxx", ".c++"]
```

### Clang module strategy (two-step)

Clang differs from GCC in that `--precompile` produces only the BMI (.pcm), then a separate
`-c` pass produces the object file. The template encodes this difference:

```toml
[modules]
supported     = true
enable_flag   = ""
precompile    = "--precompile"           # step 1: src → .pcm (no object)
import_module = "-fmodule-file={name}={pcm_path}"  # flag passed to consumers
```

---

## Build engine — internal pipeline

```
crane build
  │
  ├── 1. Parse + validate crane.toml
  ├── 2. Detect toolchain (probe $PATH, load compiler templates, version cache)
  ├── 3. Resolve dependency graph (topo sort, compile path deps in order)
  │       ├── compile each dep → archive (.a)
  │       └── collect dep include dirs
  ├── 4. Walk src/ — discover sources by file extension → language key
  ├── 5. Scan C++ sources for `export module` / `import` declarations
  │       ├── [no modules] → flat parallel compile (step 6a)
  │       └── [modules found] → module-aware pipeline (step 6b)
  ├── 6a. Flat: dirty-check + compile all sources in parallel (rayon)
  ├── 6b. Module-aware:
  │       ├── topo-sort MIUs into batches (Kahn's algorithm)
  │       ├── for each batch: compile MIUs in parallel → produce .pcm + .o
  │       │     GCC: one pass with -fmodule-output=
  │       │     Clang: --precompile → .pcm, then -c → .o
  │       └── compile MImplUs + regular TUs in parallel with -fmodule-file= per import
  └── 7. Link all .o + dep .a files → binary / .a / .so
          (each [[bin]] only links its own entry-point .o, not other bins')
```

---

## Dependency kinds

| Kind | crane.toml syntax | How it works |
|---|---|---|
| Path | `{ path = "../mylib" }` | Compiles the dep project, links its `.a` archive |
| System | `{ system = "openssl" }` | Passes `-l{name}` to the linker |
| Version | `"0.3"` | Fetched from crane.dev (not yet implemented) |
| Git | `{ git = "..." }` | Not yet implemented |

Path dependencies are non-recursive: crane checks that a dep's own deps are already present in `.deps/` but does not download them. The topo sort ensures deps are compiled in the right order.

---

## CLI commands

```
crane new <name> --lang <lang>    scaffold a new project              ✓ implemented
crane init                        init crane in current directory     ✓ implemented
crane build [--release]           build the project                   ✓ implemented
crane run [--release] [-- <args>] build and run default binary        ✓ implemented
crane test [<name>]               build and run tests                 ✓ implemented
crane clean                       wipe target/                        ✓ implemented
crane check                       validate crane.toml                 ✓ implemented
crane toolchain list              show detected compilers             ✓ implemented

crane add <name>[@ver] [--path P] [--system] [--dev]   add a dependency        ✓ implemented
crane remove <package>            remove a dependency                 ✓ implemented
crane update [<package>]          refresh lockfile for path deps      ✓ implemented (registry pending)
crane fetch                       verify/download deps                ✓ implemented (registry pending)
crane tree                        print dependency tree               ✓ implemented
crane info <package>              show package metadata               ✗ Phase 12 (registry server)
crane search <query>              search crane.dev                    ✗ Phase 12 (registry server)
crane migrate [--from <format>] [--dry-run] [--force]  import existing build system  ✓ implemented
crane login                       authenticate with crane.dev         ✗ Phase 12 (registry server)
crane publish                     upload package to registry          ✗ Phase 12 (registry server)
crane yank <version>              yank a published version            ✗ Phase 12 (registry server)
crane toolchain add <path>        install a compiler template         ✓ implemented
crane toolchain use <name>        set default compiler backend        ✗ deferred
crane lsp                         run language server on stdio        ✓ implemented
```

---

## Development roadmap

### Phase 1 — CLI skeleton ✓ COMPLETE
- [x] Cargo workspace: `crane` (bin) + `crane-core` (lib)
- [x] `clap` wiring — all subcommands stubbed
- [x] `CraneError` enum with `thiserror`
- [x] Coloured output helpers: success `✓`, warning `⚠`, error `✗`
- [x] `crane new <name> --lang <lang>` — scaffold directory + crane.toml + hello-world src
- [x] `crane init [--lang <lang>]` — init in current dir, auto-detects language from existing files

### Phase 2 — Manifest ✓ COMPLETE
- [x] Serde structs for every crane.toml section (`manifest/types.rs`)
- [x] Parse + validate with `toml_edit`
- [x] `crane check` — validate manifest, print clear errors or a summary
- [x] `find_manifest_dir` — walk up the directory tree to locate `crane.toml`
- [x] `Manifest::build_settings_for(profile)` — convert manifest + profile into `BuildSettings`
- [x] ABI compatibility validation for path dependencies
- [x] C/C++ standard consistency validation (c23 must not be newer than c++17, etc.)

### Phase 3 — Compiler detection ✓ COMPLETE
- [x] Probe `$PATH` for known compiler binaries
- [x] Load + deserialize compiler template `.toml` files at runtime
- [x] `CompilerTemplate` struct + `assemble_flags()` method (pure, unit-tested)
- [x] `crane toolchain list`
- [x] Toolchain version cache (`~/.crane/toolchain-cache.json`, mtime-validated)
- [x] Template system supports: gcc, clang, gfortran, gnat, dmd, nvcc, hipcc, icpx, opencl, ispc

### Phase 4 — Build engine ✓ COMPLETE
- [x] Source discovery with `walkdir` — extension → language key routing
- [x] Parallel compilation via `rayon`
- [x] Mtime dirty checking — source vs object, headers via `.d` dep files
- [x] `.d` dep file generation (`-MMD -MF`) for transitive header tracking
- [x] Linker invocation — binary, static lib (`.a`), shared lib (`.so`)
- [x] `crane build` + `crane run` end-to-end
- [x] `crane test` — compiles test files, links against project objects (excluding `main()`), runs each test binary
- [x] `crane clean` — wipes `target/`
- [x] Multi-language builds — C + C++ in one project, each compiled with the right binary
- [x] Multi-bin fix — each `[[bin]]` links only its own entry-point object, not all bins' mains

### Phase 5 — Dependencies ✓ COMPLETE
- [x] Path dependency resolution — compile dep, archive to `.a`, link into project
- [x] System dependency linking — `{ system = "..." }` → `-l{name}`
- [x] Dependency graph with topological sort (Kahn's algorithm)
- [x] Cycle detection with error
- [x] `.deps/<name>/` folder convention for version-pinned deps
- [x] Transitive dep checks — errors if a dep's dep is not present, does not fetch recursively
- [x] Dep include dirs accumulated in topo order for multi-level dep builds

### Phase 6 — Assembly + target config (in progress — `feature/assembly-support`)
- [x] NASM template (`nasm.toml`) — `.asm`/`.nasm`, x86/x86_64 arch flags
- [x] GAS template (`gas.toml`) — `.s`, x86/x86_64/aarch64 arch flags
- [x] `[target]` section in crane.toml — `arch` and `cpu_extensions`
- [x] `arch` drives `[arch_flags]` lookups in templates (e.g. `-f elf64` for NASM)
- [x] `cpu_extensions` produces per-extension flags (e.g. `-mavx2`, `-mfma` via `cpu_extension = "-m{name}"`)
- [x] Unified C/C++ templates — `compile_binary = "gcc"` override in `[linking.c]` so C files are not compiled with `g++`

### Phase 7 — Examples ✓ COMPLETE
- [x] `examples/hello-cpp/` — multi-file C++ with tests
- [x] `examples/multi-lang/` — C + C++ mixed project with tests
- [x] `examples/with-deps/` — path dependency (static lib)
- [x] `examples/c-simple/` — pure C, Collatz benchmark
- [x] `examples/multi-bin/` — two binaries (base64 encode/decode) from one source tree
- [x] `examples/cpp-modules/` — C++20 named modules, ASCII ray tracer
- [x] `examples/tri-lang/` — Fortran + C + C++ N-body gravity (requires gfortran)

### Phase 8 — C++20 modules ✓ COMPLETE
- [x] Scan source files for `export module` / `import` statements (`build/modules.rs`)
- [x] Classify files as MIU (`export module foo;`) / MImplU (`module foo;`) / Regular TU
- [x] Global module fragment support (`module;` + `#include` before `export module`)
- [x] Header unit imports (`import <foo>`, `import "foo"`) skipped cleanly without breaking scan
- [x] Build module DAG — Kahn's topo sort into parallel batches
- [x] Cycle detection with `DependencyCycle` error
- [x] GCC one-step MIU compilation: `-fmodule-output={pcm_path}` produces both `.o` and `.pcm`
- [x] Clang two-step MIU compilation: `--precompile` → `.pcm`, then `-c` → `.o`
- [x] `-fmodule-file={name}={pcm_path}` injected for every import with a known BMI
- [x] BMI stored at `target/{profile}/modules/{name}.pcm`
- [x] Transparent activation — auto-detected from source content; projects without `export module` use the unchanged flat parallel pipeline
- [x] Incremental: MIUs skipped when both `.o` and `.pcm` are up-to-date
- [x] `.cppm` added to gcc and clang template extension lists

### Phase 9 — Registry + lockfile (in progress — `feature/registry-lockfile`)
- [x] `crane.lock` read/write — deterministic dep pinning (version 1 format, sha256 checksums)
- [x] `crane.lock` auto-generated on every `crane build` from the resolved dep graph
- [x] `crane tree` — prints the dependency tree with dep type labels (path/registry/system/git)
- [x] `crane add <name> [--path <rel>] [--system] [--dev]` — manifest mutation + lock update
- [x] `crane remove <name>` — removes dep from crane.toml (drops empty section) + lock update
- [x] `crane update [package]` — refreshes lockfile checksums for path deps; warns on registry/git
- [x] `crane fetch` — verifies path deps exist; warns registry/git deps need crane.dev
- [x] `crane search / info` — stubs with clear "registry not yet available" message
- [x] `crane login / publish / yank` — stubs with clear "registry not yet available" message
- [ ] `crane fetch` — actually download version deps from crane.dev (needs registry server)
- [ ] `crane add` — resolve + lock exact version from crane.dev (needs registry server)

### Phase 10 — Cross-compilation ✓ COMPLETE

- [x] `[compiler] target = "aarch64-linux-gnu"` → `--target={triple}` flag via template `[structure].target`; empty template field = unsupported (GCC requires dedicated cross binary, Clang is natively multi-target)
- [x] `[compiler] sysroot = "/opt/sysroot"` → `--sysroot={path}` via template `[structure].sysroot`; supported by clang, gcc, gfortran, hipcc, icpx
- [x] `targets = ["aarch64-linux-gnu"]` on any dep — filtered in/out by `effective_dependencies()` based on `compiler.target`; absent = always include, present + native build = exclude
- [x] `os = "linux"` / `os = ["linux", "macos"]` on any dep — filtered by host OS at native build time; accepts crane platform keys and family aliases (`unix`, `bsd`); validated by `crane check`
- [x] `arch = "x86_64"` / `arch = ["x86_64", "aarch64"]` on any dep — filtered by `std::env::consts::ARCH`; validated against known arch set
- [x] `crane toolchain add <path>` — validates a local `.toml` as a `CompilerTemplate`, installs to `~/.crane/templates/<name>.toml`; `load_all_templates()` merges system + user templates (user overrides same-named system)

### Phase 11 — Importer (in progress — `feature/importer`)
Priority phase: frictionless migration off existing build systems is the single
biggest unblocker for new users. Lives in `crates/crane-importer/` (a standalone library crate that depends on `crane-core` for its error types).
All three importers parse into a shared [`ImportedProject`] IR, which
[`emit::to_toml`] serializes into `crane.toml` with stable output ordering.

- [x] `crane migrate [--from cmake|makefile|meson] [--dry-run] [--force]` — auto-detects source build system when `--from` is omitted
- [x] Auto-detection via presence of `CMakeLists.txt`, `Makefile` / `GNUmakefile`, or `meson.build` (CMake wins on ties)
- [x] CMake importer — uses `cmake-parser 0.1.0-beta.1` (`parse_cmakelists` → `Doc` → `to_commands_iter()`); inline `#` comments pre-stripped before parsing (parser limitation); `set(VAR …)` variable references expanded at assignment time so chained expansions work — extracts `project()`, `add_executable`, `add_library`, `target_link_libraries`, `target_include_directories` / `include_directories`, `set(CMAKE_CXX_STANDARD …)` / `CMAKE_C_STANDARD`, `add_definitions` / `add_compile_definitions`, `add_compile_options` / `target_compile_options`, `find_package(...)`
- [x] CMake v1 scope: flat projects only; `add_subdirectory(...)` emits a `# CRANE: subdirectory not imported` comment
- [x] CMake `set(VAR …)` tracked and `${VAR}` references expanded in subsequent calls; multi-token expansions split on whitespace so `set(SRCS a.cpp b.cpp); add_executable(app ${SRCS})` works
- [x] Multiple `add_executable(...)` calls produce multiple `[[bin]]` entries
- [x] Unrecognized `if(...) … endif()` blocks emit a review note; recognized platform guards (`if(WIN32)`, `if(LINUX)`, `if(APPLE)`, `if(UNIX)`, `if(MSVC)`, `if(MINGW)`, `if(CMAKE_SYSTEM_NAME STREQUAL "X")`) route their contents into `[platform.X]` overlays in the emitted manifest
- [x] `find_package(Foo)` → `{ system = "foo" }` dep with a review comment
- [x] Makefile importer — uses `makefile-lossless 0.3` (`Makefile::from_str_relaxed`); backslash continuations pre-joined before parsing (crate only returns first physical line of multi-line variables); extracts `CC`/`CXX`/`FC`, `CFLAGS`/`CXXFLAGS`/`FFLAGS`/`CPPFLAGS`, `LDLIBS`/`LDFLAGS`, `SRCS`/`SRC`/`SOURCES`/`OBJS`, `TARGET`/`PROGRAM`/`BIN`/`EXE`; expands `$(VAR)` / `${VAR}` references
- [x] Meson importer (regex-based over `meson.build`) — `project()`, `executable()`, `library()` / `shared_library()` / `static_library()`, `dependency()`, `include_directories()`, `add_project_arguments()` / `add_global_arguments()`; `default_options` carries `cpp_std` / `c_std` through
- [x] Unrecognised constructs → `# CRANE: could not import — review manually` preserved in the emitted TOML
- [x] `--dry-run` prints generated `crane.toml` to stdout without writing
- [x] Leaves original build files in place; errors if `crane.toml` already exists unless `--force`
- [x] Fixture tests under `crates/crane-importer/tests/importer_fixtures/{cmake,make,meson}/` with expected outputs
- [x] One worked example: `examples/migrated-from-cmake/` showing before/after
- [x] `[platform.<os>]` manifest section: per-platform overlays for `dependencies`, `compiler.defines`, `compiler.flags`, `compiler.includes.paths`. Build engine merges matching overlays at build time using `std::env::consts::OS`; family aliases (`unix`, `bsd`) apply before the specific OS. Validated by `validate_platforms`; round-tripped by the CMake importer's `if(...)` recogniser. Per-platform `[language]`, `[[bin]]`, profiles and sanitizers are deliberately not overlay-able in v1.

### Phase 12 — Registry server (planned — `feature/registry-server`, after Phase 11)
New workspace crate `crates/crane-registry/` implementing crane.dev. Filesystem-backed
for v1 so it can run self-hosted with zero external services; storage backend is
swappable later. Unblocks the outstanding Phase 9 items (`crane fetch` / `add` against
a real registry).

- [ ] Axum-based HTTP server bound to `CRANE_REGISTRY_ADDR` (default `0.0.0.0:8080`)
- [ ] Filesystem layout: `registry-data/index/<name>.json` (versions + checksums) + `registry-data/packages/<name>/<version>.tar.gz`
- [ ] `GET /api/v1/packages/{name}` — return versions + metadata
- [ ] `GET /api/v1/packages/{name}/{version}/download` — stream the `.tar.gz`
- [ ] `GET /api/v1/search?q=<query>` — prefix + substring match across package names/descriptions
- [ ] `POST /api/v1/publish` (bearer auth) — accept tarball + manifest, reject on name/version collision
- [ ] `POST /api/v1/yank` (bearer auth) — mark a version yanked; still downloadable by lock, not resolvable by `add`
- [ ] Static bearer tokens in `registry-data/tokens.toml` for v1; JWT/OAuth deferred
- [ ] Client side: `CRANE_REGISTRY_URL` env var (default `https://crane.dev`); credentials at `~/.crane/credentials.toml`
- [ ] Wire Phase 9 stubs — `crane fetch` / `add` / `search` / `info` / `publish` / `login` / `yank` — to the real HTTP API
- [ ] Integration tests spin up the server on an ephemeral port and exercise publish → fetch → build

### Phase 13 — Language server (in progress — `feature/lsp-server`)
A dedicated LSP for `crane.toml`, built on `tower-lsp` + `tokio`. Lives in
`crates/crane-lsp/` and is invokable either as a standalone `crane-lsp` binary
or via `crane lsp` (the CLI spins up a tokio runtime and hands off to the same
`run()` entry point).

- [x] Crate scaffold: `crates/crane-lsp/` (lib + bin), `tower-lsp 0.20`, stdio transport
- [x] Document store backed by `DashMap<Url, String>` — full-sync updates
- [x] Diagnostics via `crane-core`'s `validate()` + `validate_dep_compat()`
- [x] Text-based position mapping in `position.rs` — validation errors carry a free-form context (`[package]`, `[dependencies.foo]`) with no spans, so we search the buffer for those strings and fall back to the first line when nothing matches
- [x] Parse-error diagnostics extract `line N, column M` from the serde message
- [x] Completion in `completion.rs` — detects the current `[section]` via prefix scan:
    - top-level: section headers
    - `[compiler]`: `backend` (loaded template names), `warnings`, `opt-level`, field snippets
    - `[language.X]`: `std` values pulled from loaded templates' `[standards]` tables
    - `[lib]`: `type` (`static` | `shared` | `header-only`)
    - `[[bin]]`, `[profile.*]`, `[target]`: field snippets
- [x] Hover docs in `docs.rs` — Markdown descriptions keyed by dotted path (`compiler.backend`, `lib.type`, etc.)
- [x] Go-to-definition for `path = "..."` dependencies — resolves relative to the document and opens the target `crane.toml`
- [x] `crane lsp` CLI subcommand
- [ ] Publish a VS Code extension that activates on `crane.toml`
- [ ] Inlay hints showing resolved compiler flags per profile
- [ ] Code actions: "add `[[bin]]` target", "convert simple version dep → detailed table"

---

## Architecture rules

1. **`crane` crate owns the CLI** — clap parsing, `commands/` shells, and `output.rs` colour helpers. Each `cmd_*` reads cwd, calls a pure function in `crane-core`, prints the outcome.
2. **`crane-core` is a library, no CLI knowledge** — pure functions return `Result<T, CraneError>` (e.g. `build_project`, `scaffold_project → ScaffoldOutcome`). It must not depend on `output.rs` or call `print_*`. Inline `println!` for build-engine progress (`Compiling foo.cpp`, `Linking …`) is the one exception, pending a future progress-callback abstraction.
2a. **`crane-importer` is a separate library** — depends on `crane-core` for `CraneError`, exposes `run_migrate → MigrateOutcome`. Keeping it separate lets external tools use the importer without pulling in the build engine.
3. **Compiler templates are runtime data** — loaded from `compiler-templates/` directory, not hardcoded
4. **One template per toolchain, not per language** — `gcc.toml` handles both C and C++; `compile_binary` in `[linking.c]` overrides which binary compiles that language
5. **DAG cycles = hard error** — report the full cycle path (both dep cycles and module cycles)
6. **`CompilerTemplate::assemble_flags()` is pure** — no side effects, unit-tested
7. **Never shell out to Make / Ninja / CMake during a build** — crane owns the build graph entirely
8. **Errors use `thiserror` in crane-core, surface at the CLI boundary**
9. **Feature branches** — each new feature gets its own `feature/<name>` branch off `master`
10. **Module detection is transparent** — `build_sources()` scans automatically; projects without `export module` take the unchanged fast path

---

## Key Rust dependencies

```toml
[dependencies]
clap          = { version = "4", features = ["derive"] }
owo-colors    = "4"
toml_edit     = "0.22"
serde         = { version = "1", features = ["derive"] }
rayon         = "1"
walkdir       = "2"
regex         = "1"
semver        = "1"
tempfile      = "3"    # test helpers
thiserror     = "1"
```
