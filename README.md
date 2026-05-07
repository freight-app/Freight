# freight

A Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang.

Freight handles C, C++, Fortran, assembly, CUDA, HIP, OpenCL, and more — with a single declarative `freight.toml`, no Makefile or CMake required.

## Features

- **One file, one command** — describe your project in `freight.toml`, run `freight build`
- **No external build system** — freight owns the entire build graph; no Ninja or Make underneath
- **Multi-language** — C, C++, Fortran, Swift, Zig, Objective-C, Pascal, CUDA, HIP, OpenCL, Ada, D, ISPC, Odin, V, Mojo, and assembly in one project
- **C++20 modules** — scans sources for `export module` / `import`, builds a parallel-aware DAG automatically
- **Incremental builds** — mtime dirty checking via `.d` dep files tracks source + headers
- **Parallel compilation** — sources compiled in parallel with rayon
- **Profiles** — `dev` (debug, `-O0`) and `release` (`-O3`, LTO, strip) out of the box
- **Platform-conditional sources** — `[os.linux]`, `[arch.x86_64]` sections include source files and defines only on matching platforms; non-matching files are excluded from the build entirely
- **Platform overlays** — `[platform.linux]`, `[platform.windows]` for OS-specific deps and compiler flags
- **Dependency filters** — `os`, `arch`, and `targets` fields gate deps by host OS, CPU architecture, or cross-compilation triple
- **Cross-compilation** — `[compiler] target` and `sysroot` for toolchain-native cross builds
- **`freight watch`** — rebuild automatically on file changes (200 ms debounce)
- **ccache / sccache** — compile cache wrappers detected automatically; opt out with `FREIGHT_NO_CACHE=1`
- **Git dependencies** — `{ git = "url", branch = "main" }` with lock SHA enforcement and auto-fetch
- **`freight migrate`** — import an existing CMake, Makefile, or Meson project in one command
- **Language server** — `freight lsp` for `freight.toml` completions, hover docs, and go-to-definition
- **API docs** — `freight doc` extracts doc comments and renders HTML, Markdown, LaTeX, or PDF with full math support

## Naming conventions

| Name | Meaning |
|---|---|
| `freight` | The CLI binary |
| `freight.toml` | Project manifest (commit this) |
| `freight.lock` | Auto-generated lockfile (commit this) |
| `~/.freight/` | Global cache: toolchain cache, user templates, credentials |
| `freight.dev` | The package registry (not yet live) |
| `build.freight` | Optional pre-build hook script (planned) |

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
# OS-filtered dependency — only linked on matching host OS
pthread = { system = "pthread", os = "linux" }
ws2_32  = { system = "ws2_32",  os = "windows" }
# Architecture-filtered dependency
sse-opt = { path = "../sse-opt", arch = "x86_64" }

# OS-conditional sources and defines
[os.linux]
sources = ["src/os/linux/**"]
defines = ["POSIX_BUILD"]

[os.windows]
sources = ["src/os/windows/**"]
defines = ["WIN32_LEAN_AND_MEAN"]

# Arch-conditional sources and defines
[arch.x86_64]
sources = ["src/arch/x86_64/**"]
defines = ["HAVE_SSE2"]

[arch.aarch64]
sources = ["src/arch/aarch64/**"]
defines = ["HAVE_NEON"]
```

## Compiler-specific options

Beyond the standard settings (`opt-level`, `warnings`, `lto`, `std`, etc.),
individual compiler templates can expose their own options via
`compiler_option` and `language_option` callbacks registered in `.rhai` files.

**`[compiler.<name>]`** — options dispatched to the named compiler regardless
of which language is being compiled. If the compiler is detected but not the
active backend, the callbacks still run for validation (e.g. version checks)
but any injected flags are discarded.

**`[language.<key>]`** — options dispatched to the compiler handling that
language. Unknown keys (not registered by the template) are silently ignored,
so these sections are forwards-compatible.

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
[docs/requirements_handling.md](docs/requirements_handling.md) for the full
reference and [docs/compiler-templates.md](docs/compiler-templates.md) for
the template authoring guide.

## Supported languages

| Language | Key | Compiler | Extensions |
|---|---|---|---|
| C | `c` | gcc / clang / zig-cc | `.c` |
| C++ | `cpp` | g++ / clang++ / zig-cc | `.cpp` `.cc` `.cxx` `.c++` `.cppm` |
| Fortran | `fortran` | gfortran / flang / ifx / nvhpc | `.f90` `.f95` `.f03` `.f08` `.f` |
| Swift | `swift` | swiftc | `.swift` |
| Zig | `zig` | zig | `.zig` |
| Objective-C | `objc` | clang | `.m` |
| Objective-C++ | `objcpp` | clang++ | `.mm` |
| Pascal | `pascal` | fpc | `.pas` `.pp` `.lpr` |
| CUDA | `cuda` | nvcc / nvhpc | `.cu` |
| HIP | `hip` | hipcc | `.hip` |
| OpenCL | `opencl` | clang | `.cl` |
| Ada | `ada` | gnat | `.adb` `.ads` |
| D | `d` | dmd / ldc2 | `.d` |
| Odin | `odin` | odin | `.odin` |
| V | `v` | v | `.v` |
| Mojo | `mojo` | mojo | `.mojo` |
| Intel SPMD | `ispc` | ispc | `.ispc` |
| Assembly (NASM) | `nasm` | nasm | `.asm` `.nasm` |
| Assembly (GAS) | `gas` | as | `.s` `.S` |
| Assembly (YASM) | `yasm` | yasm | `.asm` `.yasm` |

Mix any combination in a single project — freight routes each file extension to the right compiler automatically.

> **zig-cc** (`backend = "zig-cc"`) is a drop-in GCC-compatible C/C++ compiler that enables zero-setup cross-compilation — `zig cc -target aarch64-linux-musl` just works without installing a sysroot.

## Migrating an existing project

```sh
cd my-cmake-project
freight migrate              # auto-detect CMake / Makefile / Meson
freight migrate --from cmake # explicit
freight migrate --dry-run    # preview without writing
```

Recognized constructs are translated to `freight.toml`. Anything that couldn't be mapped is preserved as a `# FREIGHT:` comment for manual review.

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
freight migrate [--from cmake|makefile|meson] [--dry-run] [--force]
freight lsp                           run language server on stdio
freight debug [<binary>] [--debugger <name>] [--launch-json] [-- <args>]
freight compile-commands [--release]  generate compile_commands.json
freight doc [--format html|md|latex|pdf|all]
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
| `doc-example/` | C, C++, Fortran with LaTeX math in comments |
| `migrated-from-cmake/` | Before/after for `freight migrate` |

```sh
cd examples/hello-cpp
freight build
freight run
```

## Generating API docs

`freight doc` extracts doc comments from your project's sources and renders them in one or more formats:

```sh
freight doc                        # → target/doc/index.html  (HTML with MathJax)
freight doc --format md            # → target/doc/index.md    (GFM Markdown)
freight doc --format latex         # → target/doc/docs.tex    (LaTeX source)
freight doc --format pdf           # → target/doc/docs.pdf    (requires xelatex or pdflatex)
freight doc --format all           # → target/doc/html/  md/  latex/  pdf/
```

Recognised doc comment styles:

| Language | Styles |
|---|---|
| C / C++ | `/** */`, `/*! */`, `///` — Doxygen `@param`/`@return`/`@brief`/… |
| Rust | `///`, `/** */` |
| Fortran | `!>` block opener, `!!` continuation — FORD conventions |
| D | `/++ +/`, `/**`, `///` — DDoc |
| Ada | `--!`, `---` |

Doc comment bodies are processed as Markdown (bold, italic, code spans, tables, lists).
LaTeX math — `$...$`, `$$...$$`, `\(...\)`, `\[...\]` — is preserved verbatim so MathJax
(HTML/Markdown) and LaTeX itself can render it.

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
| [docs/requirements_handling.md](docs/requirements_handling.md) | `compiler_option` / `language_option` callback system |
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

