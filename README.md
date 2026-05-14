# freight

A Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang.

Freight handles C, C++, Fortran, CUDA, HIP, OpenCL, ISPC, and assembly — with a single declarative `freight.toml`, no Makefile or CMake required.

## Features

- **One file, one command** — describe your project in `freight.toml`, run `freight build`
- **No external build system** — freight owns the entire build graph; no Ninja or Make underneath
- **Multi-language** — C, C++, Fortran, CUDA, HIP, OpenCL, ISPC, and assembly (NASM/YASM) in one project
- **C++20 modules** — scans sources for `export module` / `import`, builds a parallel-aware DAG automatically
- **Incremental builds** — mtime dirty checking via `.d` dep files tracks source + headers
- **Parallel compilation** — sources compiled in parallel with rayon
- **Profiles** — `dev` (debug, `-O0`) and `release` (`-O3`, LTO, strip) out of the box
- **Platform-conditional sources** — `[os.linux]`, `[arch.x86_64]` sections gate sources, defines, flags, includes, and deps to matching platforms
- **Dependency filters** — `os`, `arch`, and `targets` fields gate deps by host OS, CPU architecture, or cross-compilation triple
- **Cross-compilation** — `[compiler] target` and `sysroot` for toolchain-native cross builds
- **`freight watch`** — rebuild automatically on file changes (200 ms debounce)
- **ccache / sccache** — compile cache wrappers detected automatically; opt out with `FREIGHT_NO_CACHE=1`
- **Unity builds** — `[compiler] unity = true` merges all sources per language into one TU via `#include`; per-dep override with `mylib = { path = "…", unity = true }`
- **`--emit asm`** — write `.s` assembly files to `target/{profile}/asm/` alongside the normal build
- **`--time-passes`** — print a per-file compilation time table sorted slowest-first
- **Profile inheritance** — `[profile.profiling] inherits = "release"; debug = true` avoids duplicating the full flag set
- **Sanitizer CLI override** — `freight build/test --sanitize address,undefined` overrides the profile's sanitize list
- **Git dependencies** — `{ git = "url", branch = "main" }` with lock SHA enforcement and auto-fetch
- **Language server** — `freight lsp` for `freight.toml` completions, hover docs, and go-to-definition
- **Doc browser** — `freight doc` opens a terminal UI for installed local/global dependencies; `--format` extracts project doc comments as Markdown, JSON, or MessagePack

## Naming conventions

| Name | Meaning |
|---|---|
| `freight` | The CLI binary |
| `freight.toml` | Project manifest (commit this) |
| `freight.lock` | Auto-generated lockfile (commit this) |
| `build.freight` | Optional pre-build hook script |
| `~/.freight/` | Global cache: toolchain cache, user templates, credentials |
| `freight.dev` | The package registry (not yet live) |

## Installation

**Prerequisites:** Rust toolchain (stable), and at least one of gcc/clang/gfortran/nasm on `$PATH`.

```sh
git clone https://github.com/TiniTinyTerminator/freight.git
cd freight
cargo install --path crates/freight
```

## Quick start

```sh
# Scaffold a new C++ project
freight new myapp --lang cpp
cd myapp

# Build (dev profile by default)
freight build

# Build and run
freight run

# Release build
freight build --release
freight run --release

# Run tests
freight test

# Validate freight.toml
freight check
```

## freight.toml

```toml
[package]
name        = "myapp"
version     = "0.1.0"
description = "My application"
license     = "MIT"

[language.cpp]
std = "c++20"

[[bin]]
name = "myapp"
src  = "src/main.cpp"

[compiler]
opt-level = 2
warnings  = "all"
includes  = ["third-party/"]      # private -I dirs for this project
# unity = true                    # merge all sources per language into one TU

[profile.dev]
opt-level = 0
debug     = true

[profile.release]
opt-level = 3
lto       = true
strip     = true
debug     = false

[dependencies]
# Path dependency — compiles a sibling freight project and links its archive
myutils = { path = "../myutils" }
# Version dependency — resolved via pkg-config → conan → vcpkg → system-lib stub
zlib    = "1.3.1"
openssl = ">=3.0"
# Well-known OS primitives: freight ships built-in stubs, so these just work cross-platform
pthread = "0"    # → -lpthread on Linux/macOS via bundled stub; no-op on Windows
ws2_32  = "0"    # → -lws2_32 on Windows; filtered out on Linux/macOS by the stub's supports expr
# pkg-config dependency — queries pkg-config for cflags + libs
glib    = { pkg-config = "glib-2.0 >= 2.56" }
# Explicit system link — skips all resolvers, passes -l{name} directly
mylib   = { system = "mylib" }
# Pin a specific resolver
libpng  = { version = "1.6", repo = "vcpkg" }
# Architecture-filtered dependency
sse-opt = { path = "../sse-opt", arch = "x86_64" }

# OS-conditional sources, defines, and includes
[os.linux]
srcs     = ["src/os/linux/**"]
defines  = ["POSIX_BUILD"]
includes = ["platform/linux/"]

[os.windows]
srcs    = ["src/os/windows/**"]
defines = ["WIN32_LEAN_AND_MEAN"]

# Arch-conditional sources and defines
[arch.x86_64]
srcs    = ["src/arch/x86_64/**"]
defines = ["HAVE_SSE2"]

[arch.aarch64]
srcs    = ["src/arch/aarch64/**"]
defines = ["HAVE_NEON"]
```

### Library targets

```toml
[lib]
type = "static"
srcs = ["src/mathlib.cpp", "src/vec2.cpp"]   # list or single string
hdrs = ["include/mathlib.h", "include/vec2.h"] # public API — exposed to dependents
```

When a project depends on this library, freight infers the public include directories from the parent paths of `hdrs` and injects them automatically.

## Compiler-specific options

Beyond the standard settings (`opt-level`, `warnings`, `lto`, `std`, etc.),
individual compiler templates can expose their own options via
`compiler_option` and `language_option` callbacks registered in `.rhai` files.
Those registrations can include a default value, so handlers can still run when
the corresponding manifest option is omitted; omit the default when no fallback
is useful, and omit a return value when the handler succeeds.

**`[compiler.<name>]`** — options dispatched to the named compiler regardless
of which language is being compiled.

**`[language.<key>]`** — options dispatched to the compiler handling that
language. Unknown keys are silently ignored, so these sections are forwards-compatible.

```toml
# Enforce a minimum clang++ version across the project.
[compiler.clang++]
min_version = "14.0"

# Set the GPU compute architecture for CUDA sources.
[compiler.nvcc]
sm_arch     = "sm_89"
min_version = "11.8"
```

Callbacks in the template receive a `ctx` object with `ctx.value`,
`ctx.version`, `ctx.arch`, `ctx.os`, and `ctx.name`. They return `""` on
success or a non-empty error string to abort the build. Extra flags are
injected by calling the global `add_flag(s)` function. See
[docs/compiler-templates.md](docs/compiler-templates.md) for the full reference.

## Supported languages

| Language | Key | Compiler | Extensions |
|---|---|---|---|
| C | `c` | gcc / clang / tcc / msvc / icpx | `.c` |
| C++ | `cpp` | g++ / clang++ / msvc / icpx | `.cpp` `.cc` `.cxx` `.c++` `.cppm` `.ixx` `.mpp` |
| Fortran | `fortran` | gfortran / flang / ifx / nvfortran | `.f90` `.f95` `.f03` `.f08` `.f` |
| CUDA | `cuda` | nvcc | `.cu` `.cuh` |
| HIP | `hip` | hipcc | `.hip` |
| OpenCL | `opencl` | clang | `.cl` |
| Intel SPMD | `ispc` | ispc | `.ispc` |
| Assembly (GAS) | `gas` | as (binutils) | `.s` `.S` |
| Assembly (NASM) | `nasm` | nasm | `.asm` `.nasm` |
| Assembly (YASM) | `yasm` | yasm | `.asm` `.yasm` |
| D | `d` | dmd / ldc2 / gdc | `.d` |
| Objective-C | `objc` | clang | `.m` |
| Objective-C++ | `objcpp` | clang++ | `.mm` |

Mix any combination in a single project — freight routes each file extension to the right compiler automatically.

## Workspaces

A workspace root `freight.toml` with a `[workspace]` section builds all members:

```toml
[workspace]
members = ["app/", "libfoo/", "libbar/"]
```

`freight build`, `freight test`, and `freight clean` all operate across members from the workspace root.

## CLI reference

```
freight new <name> --lang <lang>      scaffold a new project
freight init                          init freight in current directory
freight build [--release] [-p <pkg>] [--emit asm] [--time-passes]
                                      build (or single workspace member)
freight run   [--release] [-p <pkg>] [-- <args>] build and run
freight test  [<filter>]  [-p <pkg>]  build and run tests
freight bench [<filter>]  [-p <pkg>]  build and run benchmarks in benches/
freight watch [--release]             watch for changes and rebuild
freight clean                         wipe target/
freight check                         validate freight.toml
freight toolchain list                show detected compilers and their supported CPU extensions
freight add <name> [--path P] [--git URL [--branch B] [--rev R]] [--system] [--dev]
freight remove <package>
freight update [<package>]
freight tree                          print dependency tree
freight lsp                           run language server on stdio
freight debug [<binary>] [--debugger <name>] [-- <args>]
freight compile-commands [--release]  generate compile_commands.json
freight doc                           browse installed dependency docs in a TUI
freight doc --format md|json|msgpack|all  generate extracted API docs
freight doc --man [--out-dir DIR]     generate man pages
```

## Examples

The `examples/` directory contains fully buildable projects. See [`examples/README.md`](examples/README.md) for a command-by-command tour that covers building, running, testing, benchmarking, features, dependency graphing, docs, install/package flows, and workspaces.

| Example | What it shows |
|---|---|
| `hello-cpp/` | Multi-file C++ with tests and a benchmark |
| `c-simple/` | Pure C, Collatz benchmark |
| `multi-lang/` | C + C++ mixed project |
| `with-deps/` | Path dependency (static lib) |
| `multi-bin/` | Two binaries from one source tree |
| `cpp-modules/` | C++20 named modules, ASCII ray tracer |
| `tri-lang/` | Fortran + C + C++ N-body gravity |
| `asm-hello/` | C + NASM assembly |
| `platform-deps/` | `[os.*]` / `[arch.*]` conditional sources, defines, and deps |
| `features-demo/` | `[features]` conditional compilation |
| `with-cmake-dep/` | Foreign CMake dependency (auto-detected) |
| `with-make-dep/` | Foreign Make dependency (auto-detected) |
| `with-git-dep/` | Git dependency cloned and built automatically |
| `with-external-deps/` | URL archive and pkg-config deps |
| `prebuilt-demo/` | `build.freight` pre-build script |
| `doc-example/` | C, C++, Fortran libs as path deps; run `freight doc` here to demo the TUI |
| `with-build-script/` | `build.freight` pre-build script with generated header and optional package probing |
| `migrated-from-cmake/` | Side-by-side CMake and Freight manifests for the same C++ project |
| `workspace-demo/` | Workspace root with app and library members, plus a path dependency |

```sh
cd examples/hello-cpp
freight check
freight build
freight run
freight test
freight bench
```

## Browsing and generating docs

`freight doc` opens a two-mode terminal browser:

**List mode** — left panel lists every dependency (local, local-dev, global), colour-coded by scope. The right panel shows the selected dep's name, kind, version, source, on-disk path, and any doc files found.

**DocView mode** — press `Enter` or click a row to open the dependency's API docs. Freight first tries to extract doc comments from the dep's source tree; if none are found it falls back to `README.md` or `target/doc/index.md`. The rendered view includes:
- **Signature** (green) below the item header, then **brief** description
- **Parameter table** with box-drawing borders, a separator row between each parameter, and word-wrapped descriptions — parameter names are highlighted in cyan
- **Returns**, **Note**, **Warning**, and other tags displayed with labelled sections
- LaTeX math (`$...$`, `$$...$$`) converted to Unicode symbols (Greek letters, ∑ ∫ √ ≤ ≥ ×, super/subscripts)

| Key | Action |
|---|---|
| `↑`/`↓`, `j`/`k` | move / scroll |
| `PgUp`/`PgDn`, `Space` | jump a page |
| `g` / `G` | top / bottom |
| `Enter` / click | open docs for selected dep |
| `Esc`, `Backspace` | return to dep list |
| `q`, `Ctrl-C` | quit |

Use `--format` when you want to extract doc comments from your project's sources and render them to `target/doc/`:

```sh
freight doc                         # browse installed local/global dependencies
freight doc --format md             # → target/doc/index.md    (GFM Markdown)
freight doc --format json           # → target/doc/docs.json   (structured JSON)
freight doc --format msgpack        # → target/doc/docs.msgpack (binary MessagePack)
freight doc --format all            # → target/doc/md/  json/  msgpack/
```

Recognised doc comment styles:

| Language | Styles |
|---|---|
| C / C++ | `/** */`, `/*! */`, `///` — Doxygen `@param`/`@return`/`@brief`/… |
| Rust | `///`, `/** */` |
| Fortran | `!>` block opener, `!!` continuation — FORD conventions |
| D | `/++ +/`, `/**`, `///` — DDoc |
| Ada | `--!`, `---` |

Doc comment bodies are processed as Markdown. LaTeX math — `$...$`, `$$...$$`, `\(...\)`, `\[...\]` — is preserved verbatim in the generated Markdown and structured outputs.

The `freight-doc` standalone binary works without a `freight.toml`:

```sh
freight-doc src/ --format all --out docs/api
freight-doc src/ --dry-run       # list extracted items without writing
```

## Documentation

| Document | Contents |
|---|---|
| [docs/manifest-reference.md](docs/manifest-reference.md) | Complete `freight.toml` field reference |
| [docs/compiler-templates.md](docs/compiler-templates.md) | Writing Rhai compiler scripts; debugger template schema |
| [docs/platform-sources.md](docs/platform-sources.md) | `[os.*]` / `[arch.*]` platform-conditional sources |
| [docs/architecture.md](docs/architecture.md) | Repository layout, build pipeline, architecture rules |
| [docs/roadmap.md](docs/roadmap.md) | Development roadmap and phase status |
| [docs/future-toolchains.md](docs/future-toolchains.md) | Planned compiler, assembler, and debugger additions |
| [docs/registry-plan.md](docs/registry-plan.md) | Architecture plan for the freight.dev registry server |

## Contributing

Contributions are welcome. Please read the [Code of Conduct](CODE_OF_CONDUCT.md) before participating.

1. Fork the repository and create a feature branch off `master`
2. Make your changes with tests where applicable
3. Ensure `cargo test --workspace` passes
4. Open a pull request with a clear description of the change
