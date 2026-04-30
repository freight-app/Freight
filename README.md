# freight

A Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang.

Freight handles C, C++, Fortran, assembly, CUDA, HIP, OpenCL, and more — with a single declarative `freight.toml`, no Makefile or CMake required.

## Features

- **One file, one command** — describe your project in `freight.toml`, run `freight build`
- **No external build system** — freight owns the entire build graph; no Ninja or Make underneath
- **Multi-language** — C, C++, Fortran, CUDA, HIP, OpenCL, Ada, D, ISPC, and assembly in one project
- **C++20 modules** — scans sources for `export module` / `import`, builds a parallel-aware DAG automatically
- **Incremental builds** — mtime dirty checking via `.d` dep files tracks source + headers
- **Parallel compilation** — sources compiled in parallel with rayon
- **Profiles** — `dev` (debug, `-O0`) and `release` (`-O3`, LTO, strip) out of the box
- **Platform overlays** — `[platform.linux]`, `[platform.windows]` for OS-specific deps and flags
- **Dependency filters** — `os`, `arch`, and `targets` fields gate deps by host OS, CPU architecture, or cross-compilation triple
- **Cross-compilation** — `[compiler] target` and `sysroot` for toolchain-native cross builds
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

# Per-platform compiler settings
[platform.linux.compiler]
defines = ["POSIX_BUILD"]

[platform.windows.compiler]
defines = ["WIN32_LEAN_AND_MEAN"]
```

## Supported languages

| Language | Key | Default compiler |
|---|---|---|
| C | `c` | gcc / clang |
| C++ | `cpp` | g++ / clang++ |
| Fortran | `fortran` | gfortran |
| CUDA | `cuda` | nvcc |
| HIP | `hip` | hipcc |
| Ada | `ada` | gnat |
| D | `d` | dmd |
| Intel SPMD | `ispc` | ispc |
| Assembly (NASM) | `nasm` | nasm |
| Assembly (GAS) | `gas` | as |

Mix any combination in a single project — freight routes each file extension to the right compiler automatically.

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
freight clean                         wipe target/
freight check                         validate freight.toml
freight toolchain list                show detected compilers
freight add <name> [--path P] [--system] [--dev]
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

## Relation to xmake

After starting this project I discovered [xmake](https://xmake.io), which covers similar ground — build tool for native languages, Lua-scripted toolchain definitions, package management. The surface resemblance is real and unintentional; I wasn't aware of it when freight was started.

The underlying approach is different enough that I want to keep going:

- **freight.toml is declarative, not a build script.** xmake's `xmake.lua` is executable Lua — the project description and the build logic are the same file. freight separates them: `freight.toml` is pure data (like `Cargo.toml`), and only toolchain definitions use scripting (Rhai, planned).
- **freight is Cargo-flavoured.** The workflow — `freight add`, `freight.lock`, a central registry, `freight test` conventions — follows Cargo's model. The goal is that a Rust developer picking up a C++ project feels at home immediately.
- **freight owns the build graph.** No Ninja or Make underneath. The DAG, dirty checking, parallel compilation, and C++20 module ordering all happen in freight itself.

If xmake already does what you need, use it. Freight is a different bet on how the UX should feel.

## Documentation

| Document | Contents |
|---|---|
| [docs/manifest-reference.md](docs/manifest-reference.md) | Complete `freight.toml` field reference |
| [docs/compiler-templates.md](docs/compiler-templates.md) | Writing Rhai compiler scripts; debugger template schema |
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

