# freight

A Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang.

Freight handles C, C++, Fortran, CUDA, HIP, OpenCL, ISPC, and assembly — with a single declarative `freight.toml`, no Makefile or CMake required.

![freight new, build, and run](https://raw.githubusercontent.com/freight-app/freight-workspace/main/tapes/freight-new.gif)

## Features

- **One file, one command** — describe your project in `freight.toml`, run `freight build`
- **No external build system** — freight owns the entire build graph; no Ninja or Make underneath
- **Multi-language** — C, C++, Fortran, CUDA, HIP, OpenCL, ISPC, and assembly in one project
- **C++20 modules** — scans sources for `export module` / `import`, builds a DAG automatically
- **Incremental builds** — mtime dirty checking via `.d` dep files tracks source + headers; a per-package flag fingerprint recompiles when features, defines, or compile flags change
- **Cargo-style features** — `[features]` map to `-D` defines, activate optional deps (`dep:name`), or forward a define into a specific dependency's build (`<dep>/define:NAME`)
- **Parallel compilation** — sources compiled in parallel with rayon
- **Profiles** — `dev` (debug) and `release` (`-O3`, LTO, strip) out of the box
- **Platform-conditional sources** — `[os.linux]`, `[arch.x86_64]` gate sources, defines, flags, and deps
- **Cross-compilation** — `[compiler] target` and `sysroot` for toolchain-native cross builds
- **ccache / sccache** — detected automatically; opt out with `FREIGHT_NO_CACHE=1`
- **Unity builds** — `[compiler] unity = true` merges all sources per language into one TU
- **`freight watch`** — rebuild automatically on file changes (200 ms debounce)
- **Git / URL / path dependencies** — `{ url = "….git" }`, `{ url = "…", sha256 = "…" }`, `{ path = "…" }`
- **freight registry** — `freight add <name>` resolves from [freight.dev](https://freight.dev); self-hostable with `freight-registry`
- **Interactive package browser** — `freight add` (no args) opens a ratatui TUI package browser

![freight add TUI — search, add, and remove packages interactively](https://raw.githubusercontent.com/freight-app/freight-workspace/main/tapes/freight-add-tui.gif)

## Installation

**Prerequisites:** at least one C/C++/Fortran compiler (gcc/clang/gfortran/nasm/…)
on `$PATH`. Foreign-dependency builds also need the relevant tool
(cmake/meson/ninja/make). Building from source additionally needs a stable Rust
toolchain.

### Prebuilt binary

Download the archive for your platform from the
[latest release](https://github.com/freight-app/Freight/releases/latest), extract
it, and put `freight` on your `$PATH`.

### From source

```sh
cargo install --git https://github.com/freight-app/Freight.git freight
```

or from a clone:

```sh
git clone https://github.com/freight-app/Freight.git
cd Freight
cargo install --path .
```

## Quick start

![freight fetch and build with a registry dependency](https://raw.githubusercontent.com/freight-app/freight-workspace/main/tapes/freight-fetch-build.gif)

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
zlib    = "1.3"                                            # pkg-config → registry
myutils = { path = "../myutils" }                          # local freight project
imgui   = { url = "https://github.com/ocornut/imgui.git" } # git dep (a .git URL)

[os.unix]
features = ["pthread"]   # -lpthread on Unix (system libs go here, not [dependencies])

[build-dependencies]
cmake = ">=3.20, <4"   # tool needed to build deps — its bin/ is prepended to PATH

[dev-dependencies]
catch2 = "3.7"         # test framework — linked only in debug builds
```

Three dependency sections:
- `[dependencies]` — linked in all builds
- `[build-dependencies]` — executables needed during compilation; freight installs them first and prepends their `bin/` to PATH so locally-installed tools take precedence over system ones
- `[dev-dependencies]` — linked only in debug builds (test frameworks, sanitizers)

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
freight build [--release]        compile (--example/--examples, --bin via run)
freight run   [--release]        build and run (--bin <n>, --example <n>)
freight test  [<filter>]         build and run tests
freight bench [<filter>]         build and run benches/
freight watch                    rebuild on file changes
freight clean                    wipe target/
freight check                    validate freight.toml
freight add [<name|URL>]         add a dependency (no args → TUI browser)
freight remove <package>
freight update [<package>]
freight fetch                    download all git/url deps
freight tree [--depth N]         print dependency tree (all dep kinds)
freight metadata [--no-deps]     machine-readable JSON of the resolved graph
freight workspace graph          visualise inter-member dependencies
freight outdated                 show outdated registry deps
freight info [<name>]            show package metadata
freight search <query>           search the registry
freight install / package        install to a prefix / build a redistributable archive
freight fmt / lint [--fix]       format / lint sources (wraps clang-format / clang-tidy)
freight debug                    launch GDB/LLDB; generate launch.json
freight lsp                      serve freight.toml diagnostics + source intelligence
freight compile-commands         generate compile_commands.json
freight publish                  upload this package to a registry
freight doc                      browse dependency docs in a TUI
freight doc --format md|json     generate extracted API docs
freight toolchain list|use       inspect / select compiler backends
freight migrate cmake|make|autotools <path>
                                 migrate a foreign build system to freight.toml

# Build/resolution flags (build, run, test, …):
#   --offline   no network; use deps already in .pkgs/
#   --locked    require freight.lock to be up to date; never rewrite it
#   --frozen    --offline + --locked
# Command aliases: define [alias] in ~/.freight/config.toml or .freight/config.toml
```

See [docs/architecture.md](docs/architecture.md) for internals and [docs/roadmap.md](docs/roadmap.md) for planned features.

## Examples

The `examples/` directory has fully buildable projects. See [`examples/README.md`](examples/README.md) for a full tour.

| Example | What it shows |
|---|---|
| `cpp/hello/` | Multi-file C++ hello world |
| `cpp/modules/` | C++20 named modules |
| `cpp/multi-bin/` | Multiple binaries from one package |
| `cpp/static-lib/` | Static library target |
| `mixed/tri-lang/` | Fortran + C + C++ in one project |
| `deps/cmake/` | Foreign CMake dependency |
| `deps/git/` | Git dependency cloned and built |
| `deps/patch/` | `[patch]` override with a local checkout |
| `misc/workspace-inherit/` | `[workspace.dependencies]` / `[workspace.package]` inheritance |
| `misc/examples-target/` | `[[example]]` targets + `examples/` auto-discovery |
| `c/required-features/` | `required-features` gating + `default-run` |
| `misc/doc/` | C, C++, Fortran libs — run `freight doc` to demo the TUI |

## Documentation

| Document | Contents |
|---|---|
| [docs/manifest-reference.md](docs/manifest-reference.md) | Complete `freight.toml` field reference |
| [docs/compiler-templates.md](docs/compiler-templates.md) | Writing Rhai compiler scripts |
| [docs/platform-sources.md](docs/platform-sources.md) | `[os.*]` / `[arch.*]` platform-conditional sources |
| [docs/cmake-interop.md](docs/cmake-interop.md) | CMake compatibility — toolchain file, dependency provider, package export |
| [docs/architecture.md](docs/architecture.md) | Repository layout, build pipeline, architecture rules |
| [docs/cargo-vs-freight.md](docs/cargo-vs-freight.md) | Mapping from Cargo concepts to freight |
| [docs/roadmap.md](docs/roadmap.md) | Development roadmap |
| [CHANGELOG.md](CHANGELOG.md) | Release history |

## Known limitations

freight `0.1.0` is an early release. The build tool is feature-complete for the
common C/C++/Fortran workflows, but some areas are still preview or unproven:

- **Manifest format is not yet stable.** While `0.x`, `freight.toml` fields may
  change between releases.
- **Editor integration is preview.** `freight lsp` works with clangd for
  C-family files and native in-process Fortran/assembly indexers; the bundled
  VS Code and Neovim plugins and the in-process `clang-bridge` are still in
  progress.
- **Platform coverage.** Development and CI focus on Linux and macOS. Windows
  builds via MSVC but is less exercised — please report issues.
- **Foreign build systems need their tools on `PATH`.** CMake/Meson/Autotools/
  Make dependencies require the respective tool (and `ninja` where applicable).
- **Debugger backends.** GDB and LLDB are supported; `rr`, `cdb`, and `windbg`
  are not yet wired up.
- **Include-hygiene lint warns but does not block** the build yet.
- **C++20/23 named-module units are recompiled on every build** — incremental
  reuse works for ordinary C/C++ translation units but not yet for module units.
- **Self-hosted registry** (`freight-registry`) is usable but early; some admin
  features (SMTP, TOTP recovery, org roles, server-side prebuilt builds) are WIP.

See [docs/roadmap.md](docs/roadmap.md) for the full status.

## Contributing

1. Fork and create a feature branch off `master`
2. Make your changes with tests where applicable — `cargo test --workspace` must pass
3. Open a pull request with a clear description of the change
