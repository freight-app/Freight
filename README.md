# freight

A Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang.

Freight handles C, C++, Fortran, CUDA, HIP, OpenCL, ISPC, and assembly — with a single declarative `freight.toml`, no Makefile or CMake required.

![freight new, build, and run](https://raw.githubusercontent.com/TiniTinyTerminator/freight-workspace/main/tapes/freight-new.gif)

## Features

- **One file, one command** — describe your project in `freight.toml`, run `freight build`
- **No external build system** — freight owns the entire build graph; no Ninja or Make underneath
- **Multi-language** — C, C++, Fortran, CUDA, HIP, OpenCL, ISPC, and assembly in one project
- **C++20 modules** — scans sources for `export module` / `import`, builds a DAG automatically
- **Incremental builds** — mtime dirty checking via `.d` dep files tracks source + headers
- **Parallel compilation** — sources compiled in parallel with rayon
- **Profiles** — `dev` (debug) and `release` (`-O3`, LTO, strip) out of the box
- **Platform-conditional sources** — `[os.linux]`, `[arch.x86_64]` gate sources, defines, flags, and deps
- **Cross-compilation** — `[compiler] target` and `sysroot` for toolchain-native cross builds
- **ccache / sccache** — detected automatically; opt out with `FREIGHT_NO_CACHE=1`
- **Unity builds** — `[compiler] unity = true` merges all sources per language into one TU
- **`freight watch`** — rebuild automatically on file changes (200 ms debounce)
- **Git / URL / path dependencies** — `{ git = "…" }`, `{ url = "…", sha256 = "…" }`, `{ path = "…" }`
- **freight registry** — `freight add <name>` resolves from [freight.dev](https://freight.dev); self-hostable with `freight-registry`
- **Interactive package browser** — `freight add` (no args) opens a ratatui TUI package browser

![freight add TUI — search, add, and remove packages interactively](https://raw.githubusercontent.com/TiniTinyTerminator/freight-workspace/main/tapes/freight-add-tui.gif)

## Installation

**Prerequisites:** Rust toolchain (stable), and at least one of gcc/clang/gfortran/nasm on `$PATH`.

```sh
git clone https://github.com/TiniTinyTerminator/freight.git
cd freight
cargo install --path crates/freight
```

## Quick start

![freight fetch and build with a registry dependency](https://raw.githubusercontent.com/TiniTinyTerminator/freight-workspace/main/tapes/freight-fetch-build.gif)

```sh
freight new myapp --lang cpp
cd myapp
freight build
freight run
```

## freight.toml at a glance

```toml
[package]
name    = "myapp"
version = "0.1.0"

[language.cpp]
std = "c++20"

[[bin]]
name = "myapp"
src  = "src/main.cpp"

[dependencies]
zlib    = "1.3"                                         # pkg-config → registry
myutils = { path = "../myutils" }                       # local freight project
imgui   = { git = "https://github.com/ocornut/imgui" } # git dep
pthread = { version = "0", os = "unix" }                # unix only
```

See [docs/manifest-reference.md](docs/manifest-reference.md) for the full field reference.

## Package browser

```sh
freight add          # interactive TUI browser
freight add zlib     # add by name
freight remove zlib  # remove
freight update       # bump all deps to latest
```

## CLI reference

```
freight new <name>               scaffold a new project
freight build [--release]        compile
freight run   [--release]        build and run
freight test  [<filter>]         build and run tests
freight watch                    rebuild on file changes
freight clean                    wipe target/
freight check                    validate freight.toml
freight add [<name|URL>]         add a dependency (no args → TUI browser)
freight remove <package>
freight update [<package>]
freight fetch                    download all git/url deps
freight tree                     print dependency tree
freight search <query>           search the registry
freight publish                  upload this package to a registry
freight doc                      browse dependency docs in a TUI
freight doc --format md|json     generate extracted API docs
freight migrate cmake|make|autotools <path>
                                 migrate a foreign build system to freight.toml
```

See [docs/architecture.md](docs/architecture.md) for internals and [docs/roadmap.md](docs/roadmap.md) for planned features.

## Examples

The `examples/` directory has fully buildable projects. See [`examples/README.md`](examples/README.md) for a full tour.

| Example | What it shows |
|---|---|
| `hello-cpp/` | Multi-file C++ with tests and a benchmark |
| `with-deps/` | Path dependency (static lib) |
| `cpp-modules/` | C++20 named modules, ASCII ray tracer |
| `tri-lang/` | Fortran + C + C++ N-body gravity |
| `with-cmake-dep/` | Foreign CMake dependency |
| `with-git-dep/` | Git dependency cloned and built |
| `doc-example/` | C, C++, Fortran libs — run `freight doc` to demo the TUI |

## Documentation

| Document | Contents |
|---|---|
| [docs/manifest-reference.md](docs/manifest-reference.md) | Complete `freight.toml` field reference |
| [docs/compiler-templates.md](docs/compiler-templates.md) | Writing Rhai compiler scripts |
| [docs/platform-sources.md](docs/platform-sources.md) | `[os.*]` / `[arch.*]` platform-conditional sources |
| [docs/architecture.md](docs/architecture.md) | Repository layout, build pipeline, architecture rules |
| [docs/roadmap.md](docs/roadmap.md) | Development roadmap |

## Contributing

1. Fork and create a feature branch off `master`
2. Make your changes with tests where applicable — `cargo test --workspace` must pass
3. Open a pull request with a clear description of the change
