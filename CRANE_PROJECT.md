# Crane — Build Tool & Package Manager

## What is crane?

Crane is a Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang: C, C++, Fortran, Ada, D, and others. It aims to be the single tool you need to build, test, and publish native code — no Makefile, no CMake, no Ninja required.

The project is written in Rust.

---

## Core philosophy

- **No external build system** — crane owns the entire build graph internally. No Ninja, no Make underneath.
- **C++20 modules first** — dependency scanning is based on `export module` / `import` statements and directory structure, not `#include`.
- **Declarative compiler templates** — each compiler (gcc, clang, nvcc, gfortran, icpx…) is described in a `.toml` file that maps abstract settings to real flags. Adding a new compiler = writing a TOML, not writing Rust.
- **One tool, many languages** — file extension routes to the right compiler automatically. A single project can mix `.cpp`, `.cu`, `.f90` files.
- **Incremental by default** — source + header + flag hashing, dirty checking, parallel compilation via a thread pool.

---

## Naming conventions

| Name | Meaning |
|---|---|
| `crane` | The CLI binary |
| `crane.toml` | Project manifest |
| `crane.lock` | Auto-generated lockfile (commit this) |
| `build.crane` | Optional pre-build hook script |
| `~/.crane/` | Global cache directory |
| `crane.dev` | The package registry |

---

## Repository layout (to be created)

```
crane/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── crane/                  # binary crate — CLI entry point
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs
│   └── crane-core/             # library crate — all build logic
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── manifest/       # crane.toml parsing
│           ├── compiler/       # compiler detection + templates
│           ├── graph/          # DAG, topo sort, dirty checking
│           ├── build/          # compilation + linking orchestration
│           ├── deps/           # dependency resolution
│           └── migrate/        # build system importers
├── compiler-templates/         # bundled .toml files per compiler
│   ├── gcc.toml
│   ├── clang.toml
│   ├── gfortran.toml
│   ├── gnat.toml
│   └── nvcc.toml
└── CRANE_PROJECT.md            # this file
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

[language]
name = "c++"     # c | c++ | fortran | ada | d | mixed
std  = "c++20"

[lib]
type    = "static"   # static | shared | header-only
src     = "src/"
include = "include/"

[[bin]]
name = "myproject"
src  = "src/main.cpp"

[dependencies]
libopenblas = "0.3"
openssl     = { system = "openssl", version = ">=3.0" }
myutils     = { path = "../myutils" }

[dev-dependencies]
libcheck = "0.15"

[compiler]
backend   = "auto"   # auto | gcc | clang | gfortran
opt-level = 2
debug     = false
warnings  = "all"    # none | default | all | error
defines   = ["USE_BLAS"]
flags     = []

[compiler.includes]
paths = ["include/", "third_party/include/"]

[compiler.overrides]
".cu"  = "nvcc"
".f90" = "gfortran"

[profile.dev]
opt-level = 0
debug     = true
sanitize  = ["address", "undefined"]

[profile.release]
opt-level = 3
lto       = true
strip     = true
debug     = false

[features]
default = ["blas"]
blas    = ["dep:libopenblas"]
fft     = ["dep:fftw3"]
full    = ["blas", "fft"]
```

---

## Compiler template format

Each compiler is described in a `.toml` file. Crane reads these at startup and uses them to
assemble compile commands. Adding support for a new compiler means writing a new `.toml`,
not touching Rust code.

```toml
# compiler-templates/gcc.toml

[compiler]
name          = "gcc"
binary        = "g++"
version_arg   = "--version"
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b"

[compiler.extensions]
handles = [".cpp", ".cc", ".cxx", ".c++", ".c"]

[compiler.flags]
opt.0            = "-O0"
opt.1            = "-O1"
opt.2            = "-O2"
opt.3            = "-O3"
opt.s            = "-Os"
opt.z            = "-Oz"
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

[compiler.standards]
"c11"   = "-std=c11"
"c17"   = "-std=c17"
"c23"   = "-std=c23"
"c++17" = "-std=c++17"
"c++20" = "-std=c++20"
"c++23" = "-std=c++23"

[compiler.structure]
include_dir  = "-I{path}"
define       = "-D{name}"
define_value = "-D{name}={value}"
output       = "-o {path}"
compile_only = "-c"
dep_file     = "-MF {path}"

[compiler.modules]
supported     = true
enable_flag   = "-fmodules-ts"
compile_miu   = "-fmodule-output={pcm_path}"
import_module = "-fmodule-file={name}={pcm_path}"

[compiler.passthrough]
enabled = false
prefix  = ""
```

### Compiler template — nvcc example

```toml
# compiler-templates/nvcc.toml

[compiler]
name          = "nvcc"
binary        = "nvcc"
version_arg   = "--version"
version_regex = "release (\\d+\\.\\d+)"

[compiler.extensions]
handles = [".cu", ".cuh"]

[compiler.flags]
opt.0            = "-O0"
opt.1            = "-O1"
opt.2            = "-O2"
opt.3            = "-O3"
opt.s            = "-O2"
opt.z            = "-O2"
debug.true       = "-g -G"
debug.false      = ""
warnings.none    = ""
warnings.default = "-Wall"
warnings.all     = "-Wall -Wextra"
warnings.error   = "-Wall -Wextra -Werror"
lto.true         = ""
lto.false        = ""
sanitize         = ""

[compiler.standards]
"c++17" = "-std=c++17"
"c++20" = "-std=c++20"

[compiler.structure]
include_dir  = "-I{path}"
define       = "-D{name}"
define_value = "-D{name}={value}"
output       = "-o {path}"
compile_only = "-c"
dep_file     = ""

[compiler.modules]
supported = false

[compiler.passthrough]
enabled = true
prefix  = "-Xcompiler"

[compiler.extra]
always = ["--expt-relaxed-constexpr", "--extended-lambda"]
```

---

## C++20 modules — how crane handles them

Module names map directly to directory structure:

```
src/math/vec.cpp        → module math.vec
src/engine.cpp          → module engine
src/render/pipeline.cpp → module render.pipeline
```

### Scanning rules

Crane scans each `.cpp` file for these patterns:

```
export module foo.bar;   → this file IS a Module Interface Unit (MIU)
module foo.bar;          → this file IS a Module Implementation Unit (MImplU)
import foo.bar;          → this file DEPENDS ON module foo.bar
import <vector>;         → standard library import, skip for ordering
```

### Node types in the DAG

| Type | Description | Output |
|---|---|---|
| MIU | `export module X` — declares the module | `.pcm` + `.o` |
| MImplU | `module X` (no export) — implements it | `.o` |
| TU | neither — plain translation unit | `.o` |

### Compile order

1. MIUs compiled first (produce `.pcm` files)
2. MImplUs compiled next (depend on their MIU's `.pcm`)
3. Plain TUs last
4. All `.o` files linked into final binary

### GCC vs Clang module flags

```bash
# GCC — one step, produces .pcm and .o together
g++ -std=c++20 -fmodules-ts \
    -fmodule-output=target/dev/pcm/math/vec.pcm \
    -c src/math/vec.cpp \
    -o target/dev/objs/math.vec.o

# Clang — two steps for MIUs
clang++ -std=c++20 --precompile src/math/vec.cpp -o target/dev/pcm/math/vec.pcm
clang++ -std=c++20 -fmodule-file=target/dev/pcm/math/vec.pcm \
    -c src/math/vec.cpp -o target/dev/objs/math.vec.o
```

The `CompilerTemplate` handles this difference — Clang MIUs emit two job steps, GCC one.

### Target layout

```
target/
└── dev/
    ├── objs/
    │   ├── math.vec.o
    │   ├── engine.o
    │   └── main.o
    ├── pcm/
    │   ├── math/
    │   │   └── vec.pcm
    │   └── engine.pcm
    └── myproject
```

---

## Build engine — internal pipeline

```
crane build
  │
  ├── 1. Parse crane.toml
  ├── 2. Detect toolchain (probe $PATH, load compiler templates)
  ├── 3. Walk src/ — collect all source files
  ├── 4. Scan each file — classify as MIU / MImplU / TU
  ├── 5. Build DAG (nodes = files, edges = import dependencies)
  ├── 6. Topological sort → compilation batches (Kahn's algorithm)
  ├── 7. Dirty check (hash source + flags → skip clean files)
  ├── 8. Compile each batch in parallel (rayon thread pool)
  │       ├── MIUs     → -fmodule-output=... → .pcm + .o
  │       ├── MImplUs  → -fmodule-file=...   → .o
  │       └── TUs      → -c                  → .o
  └── 9. Link all .o files → binary / .a / .so
```

---

## Dependency kinds

```rust
enum DepKind {
    Source    { srcs: Vec<PathBuf>, includes: Vec<PathBuf> },
    Static    { lib_path: PathBuf,  includes: Vec<PathBuf> },
    Shared    { lib_path: PathBuf,  includes: Vec<PathBuf>, rpath: PathBuf },
    System    { link_flags: Vec<String>, cflags: Vec<String> },
    HeaderOnly { includes: Vec<PathBuf> },
}
```

- **Source** — compile alongside your own code with full flag consistency
- **Static** — pre-compiled `.a`, link with `-L` and `-l`
- **Shared** — pre-compiled `.so`, link with `-L`, `-l`, `-Wl,-rpath`
- **System** — resolved via `pkg-config --libs --cflags`
- **Header-only** — add the include path, nothing to link

---

## CLI commands

```
crane new <name> --lang <lang>    scaffold a new project
crane init                        init crane in current directory
crane build [--release]           build the project
crane run [-- <args>]             build and run default binary
crane test [<name>]               build and run tests
crane add <package>[@version]     add a dependency
crane remove <package>            remove a dependency
crane update [<package>]          update deps within semver ranges
crane fetch                       download deps without building
crane tree                        print dependency tree
crane info <package>              show package metadata
crane search <query>              search crane.dev
crane check                       validate crane.toml
crane clean                       wipe target/
crane migrate [--from <format>]   import existing build system
crane login                       authenticate with crane.dev
crane publish                     upload package to registry
crane yank <version>              yank a published version
crane toolchain list              show detected compilers
crane toolchain add <name>        install a compiler template
crane toolchain use <name>        set default compiler backend
```

---

## Supported build system importers (crane migrate)

| Format | Detected by | Parser used |
|---|---|---|
| CMake | `CMakeLists.txt` | `cmake-parser` crate (all 127 commands) |
| Makefile | `Makefile` / `GNUmakefile` | `makefile-lossless` crate (lossless CST) |
| Meson | `meson.build` | hand-written regex |
| Autoconf | `configure.ac` + `Makefile.am` | hand-written regex |

Crane never executes imported files — static parsing only.
Anything it cannot parse is left as `# CRANE: could not import — review manually` in the generated `crane.toml`.

---

## Key Rust dependencies

```toml
[dependencies]
# CLI
clap          = { version = "4", features = ["derive"] }
owo-colors    = "4"
indicatif     = "0.17"

# Manifest
toml_edit     = "0.22"
serde         = { version = "1", features = ["derive"] }
serde_json    = "1"

# Build engine
rayon         = "1"
walkdir       = "2"
regex         = "1"
sha2          = "0.10"

# Dependency resolution
semver        = "1"

# Importers
cmake-parser      = "0.1"
makefile-lossless = "0.2"

# Error handling
anyhow        = "1"
thiserror     = "1"
```

---

## Development roadmap

### Phase 1 — CLI skeleton ✓ COMPLETE
- [x] Cargo workspace: `crane` (bin) + `crane-core` (lib)
- [x] `clap` wiring — all subcommands stubbed, print "not implemented"
- [x] `CraneError` enum with `thiserror`
- [x] Coloured output helpers: success `✓`, warning `⚠`, error `✗`
- [x] `crane new <name> --lang <lang>` — scaffold directory + crane.toml + hello-world src
- [x] `crane init [--lang <lang>]` — init in current dir, auto-detects language from existing files

### Phase 2 — Manifest ✓ COMPLETE
- [x] Serde structs for every crane.toml section (`manifest/types.rs`)
- [x] Parse + validate with `toml_edit` (`load_manifest_str`, `load_manifest`)
- [x] `crane check` — validate manifest, print clear errors or a summary
- [x] `find_manifest_dir` — walk up the directory tree to locate `crane.toml`
- [x] `Manifest::build_settings_for(profile)` — convert manifest + profile into `BuildSettings` (needed for Phase 4)

### Phase 3 — Compiler detection ✓ COMPLETE
- [x] Probe `$PATH` for known compiler binaries
- [x] Load + deserialize compiler template `.toml` files
- [x] `CompilerTemplate` struct + `assemble_flags()` method (pure, unit-tested)
- [x] `crane toolchain list`
- [x] Toolchain version cache — `~/.crane/toolchain-cache.json`, mtime-validated, avoids re-running `--version` on every invocation

### Phase 4 — Build engine (first working build)
- [ ] Source discovery with `walkdir`
- [ ] Module scanner — classify MIU / MImplU / TU via regex
- [ ] DAG construction + Kahn's topological sort
- [ ] Cycle detection with full cycle path in error
- [ ] Single-threaded compile loop
- [ ] Linker invocation
- [ ] `crane build` + `crane run` end-to-end ← first milestone

### Phase 5 — Incremental + parallel
- [ ] Dirty checking: hash source + transitive pcm deps + flags
- [ ] Persist hashes to `target/.crane-cache`
- [ ] Parallel compilation with `rayon`
- [ ] Live progress: `Compiling [3/12] math.vec`

### Phase 6 — Dependencies + ecosystem
- [ ] System deps via `pkg-config`
- [ ] Local path deps
- [ ] `crane.lock` read/write
- [ ] Build system importers (`crane migrate`)
- [ ] Crane registry fetch + publish

---

## Architecture rules for Claude Code

1. **`crane` crate is thin** — only `main.rs`, CLI parsing, delegates everything to `crane-core`
2. **All logic in `crane-core`** — testable without the CLI
3. **Compiler templates are runtime data** — loaded from `compiler-templates/` directory, not hardcoded
4. **Module name = file path** — `src/foo/bar.cpp` → module `foo.bar`. Enforce as a convention, not a guess.
5. **DAG cycles = hard error** — report the full cycle path: `a.cpp → b.cpp → c.cpp → a.cpp`
6. **Dirty check hashes**: file contents + all transitively imported `.pcm` paths + assembled compiler flags
7. **`CompilerTemplate::assemble_flags()` is pure** — no side effects, easy to unit test
8. **Never shell out to Make / Ninja / CMake during a build** — crane owns the build graph entirely
9. **Errors use `thiserror` in crane-core, `anyhow` at the CLI boundary**
10. **Imports of standard library modules (`import <vector>`) are silently ignored** for DAG ordering — they are never project nodes
