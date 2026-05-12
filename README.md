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
# System dependency — links against a system-installed library
openssl = { system = "openssl" }
# pkg-config dependency — queries pkg-config for cflags + libs
zlib    = { pkg-config = "zlib" }
# OS-filtered dependency — only linked on matching host OS
pthread = { system = "pthread", os = "linux" }
ws2_32  = { system = "ws2_32",  os = "windows" }
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

# Validate that assembly sources are only built for x86_64.
[language.asm]
arch = "x86_64"
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
| C++ | `cpp` | g++ / clang++ / msvc / icpx | `.cpp` `.cc` `.cxx` `.c++` `.cppm` |
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
freight build [--release]             build
freight run   [--release] [-- <args>] build and run
freight test  [<filter>]              build and run tests
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
freight doc                          browse installed dependency docs in a TUI
freight doc --format md|json|msgpack|all  generate extracted API docs
freight man [--out-dir DIR]           generate man pages
```

## Examples

The `examples/` directory contains fully buildable projects:

| Example | What it shows |
|---|---|
| `hello-cpp/` | Multi-file C++ with tests |
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
| `doc-example/` | C, C++, Fortran with LaTeX math in comments |

```sh
cd examples/hello-cpp
freight build
freight run
```

## Browsing and generating docs

`freight doc` opens an interactive terminal browser for installed dependencies. The browser lists local project dependencies from `freight.toml` / `.deps/` and global cached dependencies from `~/.freight`, with arrow-key and `j`/`k` scrolling plus details for any README or generated docs found for the selected dependency.

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
