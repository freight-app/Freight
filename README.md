# crane

A Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang.

Crane handles C, C++, Fortran, assembly, CUDA, HIP, OpenCL, and more — with a single declarative `crane.toml`, no Makefile or CMake required.

## Features

- **One file, one command** — describe your project in `crane.toml`, run `crane build`
- **No external build system** — crane owns the entire build graph; no Ninja or Make underneath
- **Multi-language** — C, C++, Fortran, CUDA, HIP, OpenCL, Ada, D, ISPC, and assembly in one project
- **C++20 modules** — scans sources for `export module` / `import`, builds a parallel-aware DAG automatically
- **Incremental builds** — mtime dirty checking via `.d` dep files tracks source + headers
- **Parallel compilation** — sources compiled in parallel with rayon
- **Profiles** — `dev` (debug, `-O0`) and `release` (`-O3`, LTO, strip) out of the box
- **Platform overlays** — `[platform.linux]`, `[platform.windows]` for OS-specific deps and flags
- **Dependency filters** — `os`, `arch`, and `targets` fields gate deps by host OS, CPU architecture, or cross-compilation triple
- **Cross-compilation** — `[compiler] target` and `sysroot` for toolchain-native cross builds
- **`crane migrate`** — import an existing CMake, Makefile, or Meson project in one command
- **Language server** — `crane lsp` for `crane.toml` completions, hover docs, and go-to-definition
- **API docs** — `crane doc` extracts doc comments and renders HTML, Markdown, LaTeX, or PDF with full math support

## Naming conventions

| Name | Meaning |
|---|---|
| `crane` | The CLI binary |
| `crane.toml` | Project manifest (commit this) |
| `crane.lock` | Auto-generated lockfile (commit this) |
| `~/.crane/` | Global cache: toolchain cache, user templates, credentials |
| `crane.dev` | The package registry (not yet live) |
| `build.crane` | Optional pre-build hook script (planned) |

## Installation

**Prerequisites:** Rust toolchain (stable), and at least one of gcc/clang/gfortran/nasm on `$PATH`.

```sh
git clone https://github.com/TiniTinyTerminator/crane.git
cd crane
cargo install --path crates/crane
```

## Quick start

```sh
# Scaffold a new C++ project
crane new myapp --lang cpp
cd myapp

# Build (dev profile by default)
crane build

# Build and run
crane run

# Release build
crane build --release
crane run --release

# Run tests
crane test

# Validate crane.toml
crane check
```

## crane.toml

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
# Path dependency — compiles a sibling crane project and links its archive
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

Mix any combination in a single project — crane routes each file extension to the right compiler automatically.

## Migrating an existing project

```sh
cd my-cmake-project
crane migrate              # auto-detect CMake / Makefile / Meson
crane migrate --from cmake # explicit
crane migrate --dry-run    # preview without writing
```

Recognized constructs are translated to `crane.toml`. Anything that couldn't be mapped is preserved as a `# CRANE:` comment for manual review.

## Workspaces

A workspace root `crane.toml` with a `[workspace]` section builds all members:

```toml
[workspace]
members = ["app/", "libfoo/", "libbar/"]
```

`crane build`, `crane test`, and `crane clean` all operate across members from the workspace root.

## CLI reference

```
crane new <name> --lang <lang>      scaffold a new project
crane init                          init crane in current directory
crane build [--release]             build
crane run   [--release] [-- <args>] build and run
crane test  [<filter>]              build and run tests
crane clean                         wipe target/
crane check                         validate crane.toml
crane toolchain list                show detected compilers
crane add <name> [--path P] [--system] [--dev]
crane remove <package>
crane update [<package>]
crane tree                          print dependency tree
crane migrate [--from cmake|makefile|meson] [--dry-run] [--force]
crane lsp                           run language server on stdio
crane debug [<binary>] [--debugger <name>] [--launch-json] [-- <args>]
crane compile-commands [--release]  generate compile_commands.json
crane doc [--format html|md|latex|pdf|all]
crane man [--out-dir DIR]           generate man pages
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
| `migrated-from-cmake/` | Before/after for `crane migrate` |

```sh
cd examples/hello-cpp
crane build
crane run
```

## Generating API docs

`crane doc` extracts doc comments from your project's sources and renders them in one or more formats:

```sh
crane doc                        # → target/doc/index.html  (HTML with MathJax)
crane doc --format md            # → target/doc/index.md    (GFM Markdown)
crane doc --format latex         # → target/doc/docs.tex    (LaTeX source)
crane doc --format pdf           # → target/doc/docs.pdf    (requires xelatex or pdflatex)
crane doc --format all           # → target/doc/html/  md/  latex/  pdf/
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

The `crane-doc` standalone binary works without a `crane.toml`:

```sh
crane-doc src/ --format all --out docs/api
crane-doc src/ --dry-run       # list extracted items without writing
```

## Documentation

| Document | Contents |
|---|---|
| [docs/manifest-reference.md](docs/manifest-reference.md) | Complete `crane.toml` field reference |
| [docs/compiler-templates.md](docs/compiler-templates.md) | Writing Rhai compiler scripts; debugger template schema |
| [docs/architecture.md](docs/architecture.md) | Repository layout, build pipeline, architecture rules |
| [docs/roadmap.md](docs/roadmap.md) | Development roadmap and phase status |
| [docs/future-toolchains.md](docs/future-toolchains.md) | Planned compiler, assembler, and debugger additions |
| [docs/registry-plan.md](docs/registry-plan.md) | Architecture plan for the crane.dev registry server |

## Contributing

Contributions are welcome. Please read the [Code of Conduct](CODE_OF_CONDUCT.md) before participating.

1. Fork the repository and create a feature branch off `master`
2. Make your changes with tests where applicable
3. Ensure `cargo test --workspace` passes
4. Open a pull request with a clear description of the change

