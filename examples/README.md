# Freight examples

This directory is a tour of Freight's day-to-day usability. Each example is a
small, buildable project with its own `freight.toml`; together they exercise
single-language projects, mixed-language builds, tests, benchmarks, features,
platform filters, path dependencies, foreign build systems, docs, packaging, and
workspace orchestration.

> Tip: from any example directory, run `freight check` first to validate the
> manifest, then `freight build` or `freight run` to compile it.

## Quick command tour

```sh
# Inspect detected compilers and generated build metadata.
freight toolchain list
freight check
freight compile-commands

# Build, run, test, and benchmark with the dev profile.
freight build
freight run
freight test
freight bench

# Use release settings, sanitizer overrides, verbose compiler output, and jobs.
freight build --release
freight run --release -- --program-arg
freight test --sanitize address,undefined
freight -v -j 4 build

# Feature selection works like Cargo-style opt-in compilation switches.
freight run --features tls,json
freight run --no-default-features

# Dependency and graph commands are useful before and after adding deps.
freight tree
freight graph --format mermaid --output deps.mmd
freight fetch
freight update

# Developer tooling and distribution helpers.
freight fmt --check
freight lint
freight debug --launch-json
freight doc --format all
freight doc --man --out-dir target/man
freight install --destdir target/stage --prefix /usr/local
freight package
```

Some commands require optional tools (`gfortran`, `nasm`, `cmake`, `make`,
`pkg-config`, formatters, linters, or a debugger). The examples below call out
those prerequisites when they matter.

## Example matrix

| Example | What it demonstrates | Useful commands |
|---|---|---|
| [`c-simple/`](c-simple/) | Small pure-C binary, profiles, compiler warnings | `freight run`, `freight build --release` |
| [`hello-cpp/`](hello-cpp/) | Multi-file C++ project with `inc/`, tests, and benchmarks | `freight test`, `freight bench bench_stats` |
| [`multi-lang/`](multi-lang/) | C implementation linked into a C++ executable | `freight run`, `freight test` |
| [`static-lib/`](static-lib/) | C static library (`libstatic-lib.a`) consumed by a C++ binary in the same manifest | `freight run` |
| [`tri-lang/`](tri-lang/) | Fortran kernel, C timer, C++ orchestration | `freight run` |
| [`fortran-hello/`](fortran-hello/) | Pure Fortran binary: derived types, array intrinsics, and elemental functions | `freight run` |
| [`asm-hello/`](asm-hello/) | C entry point calling NASM assembly | `freight run` |
| [`d-hello/`](d-hello/) | Pure D binary: ranges, UFCS, operator overloading, C interop via `extern (C)` | `freight run` |
| [`cpp-modules/`](cpp-modules/) | C++20 named module discovery and dependency ordering | `freight run` |
| [`zig-hello/`](zig-hello/) | Native Zig binary: comptime generics, tagged unions, error handling | `freight run` |
| [`zig-cc-hello/`](zig-cc-hello/) | C compiled via `zig cc` — drop-in clang with Zig's cross-compilation support | `freight run` |
| [`zig-cpp-hello/`](zig-cpp-hello/) | Zig binary calling a C++ library; documents the Zig 0.16 SysV ABI rules for >16-byte structs | `freight run` |
| [`zig-asm-hello/`](zig-asm-hello/) | Zig binary calling hand-written x86-64 NASM assembly (POPCNT, BSWAP, GCD, BSR) | `freight run` |
| [`ada-hello/`](ada-hello/) | Pure Ada binary: `Vec2` record, insertion sort, subtype constraints, exception handling | `freight run` |
| [`cuda-hello/`](cuda-hello/) | CUDA `vec_add` and `vec_scale` kernels — requires a CUDA-capable GPU to run | `freight run` |
| [`opencl-hello/`](opencl-hello/) | OpenCL `vec_add` and `vec_scale` — requires an OpenCL platform (NVIDIA / Intel / AMD / POCL) | `freight run` |
| [`multi-bin/`](multi-bin/) | Multiple `[[bin]]` targets in one manifest | `freight run --bin encode`, `freight run --bin decode` |
| [`features-demo/`](features-demo/) | `[features]`, defaults, transitive feature activation | `freight run --features tls`, `freight run --no-default-features` |
| [`platform-deps/`](platform-deps/) | `[os.*]` / `[arch.*]` defines and platform dependencies | `freight run`, `freight graph` |
| [`with-deps/`](with-deps/) | Version-style dependency declaration | `freight tree`, `freight build` |
| [`with-cmake-dep/`](with-cmake-dep/) | Local path dependency that Freight builds with CMake | `freight run` |
| [`with-make-dep/`](with-make-dep/) | Local path dependency that Freight builds with Make | `freight run` |
| [`with-git-dep/`](with-git-dep/) | Git dependency fetch, include override, CMake args | `freight fetch`, `freight build` |
| [`with-external-deps/`](with-external-deps/) | URL archive deps plus `pkg-config` / system fallback | `freight fetch`, `freight run` |
| [`prebuilt-demo/`](prebuilt-demo/) | Optional `pkg-config` dep checked by `build.freight` | `freight run` |
| [`with-build-script/`](with-build-script/) | Full pre-build script: generated header, optional package probe, rerun rules | `freight run`, `freight compile-commands` |
| [`doc-example/`](doc-example/) | C, C++, and Fortran doc comments rendered by `freight doc` | `freight doc --format all` |
| [`migrated-from-cmake/`](migrated-from-cmake/) | Same project expressed as CMake and as `freight.toml` | `freight run` |
| [`workspace-demo/`](workspace-demo/) | Workspace root with two members and a path-linked library | `freight build`, `freight run` from `app/` |
| [`with-registry-dep/`](with-registry-dep/) | Pull C++ deps (`fmt`, `nlohmann-json`) from a self-hosted freight registry | `freight fetch`, `freight run` |
| [`with-registry-versions/`](with-registry-versions/) | Version constraints (`=`, `>=`) and `repo` pinning for registry deps | `freight info`, `freight fetch`, `freight run` |

## Workflows by capability

### 1. Validate and build a project

```sh
cd examples/hello-cpp
freight check
freight build
freight run
```

`hello-cpp` is the best starting point because it shows a C++ executable,
private include discovery through `inc/`, reusable implementation files under
`src/`, and a test binary under `tests/`.

### 2. Run tests and benchmarks

```sh
cd examples/hello-cpp
freight test
freight test test_stats
freight bench
freight bench bench_stats
```

Tests and benchmarks are discovered from the `tests/` and `benches/`
directories. Each source file there is compiled as a standalone executable and
linked with the non-`main()` objects from the project.

### 3. Select features at build time

```sh
cd examples/features-demo
freight run
freight run --features tls
freight run --features json,net
freight run --no-default-features
freight run --no-default-features --features tls
```

The manifest documents the feature graph: `default` enables `logging`, while
`tls` enables `net`. Active features become preprocessor defines such as
`LOGGING`, `TLS`, and `NET`.

### 4. Work with multiple binaries

```sh
cd examples/multi-bin
freight run --bin encode -- hello
freight run --bin decode -- aGVsbG8=
freight build --release
```

The `multi-bin` manifest repeats `[[bin]]` for two executables that share the
same implementation sources. Freight avoids linking the wrong `main()` object
into each binary.

### 5. Use mixed languages

```sh
cd examples/multi-lang
freight test
freight run

cd ../tri-lang
freight run

cd ../asm-hello
freight run

cd ../d-hello
freight run
```

Freight classifies sources by extension and routes them to the matching compiler
template. These examples show C + C++, Fortran + C + C++, C + assembly, and D
with C interop via `extern (C)` / libc `qsort`.

### 6. Exercise dependency resolution

```sh
cd examples/with-cmake-dep
freight tree
freight run

cd ../with-make-dep
freight tree
freight run

cd ../with-external-deps
freight fetch
freight run
```

Path dependencies can be ordinary Freight projects or foreign source trees.
Freight auto-detects common foreign build systems such as CMake and Make, then
links the produced libraries into the root project.

### 7. Inspect platform-conditional settings

```sh
cd examples/platform-deps
freight check
freight run
freight graph --format dot
```

This example uses `[os.linux]`, `[os.macos]`, `[os.windows]`, and `[os.unix]` to
merge defines and dependencies only when they match the current platform.

### 8. Generate API docs

```sh
cd examples/doc-example
freight doc --format md
freight doc --format json
freight doc --format msgpack
freight doc --format all
```

The `doc-example` project includes documented C, C++, and Fortran libraries as
path dependencies. Use it to verify Markdown, JSON, MessagePack, and TUI doc
browser behavior.

### 9. Run a pre-build script

```sh
cd examples/with-build-script
freight run
freight compile-commands
```

`build.freight` generates `version.h`, probes optional `zlib`, and demonstrates
incremental rerun rules. The source includes the generated header from the build
output directory.

### 10. Build a workspace

```sh
cd examples/workspace-demo
freight build
freight test

cd app
freight run
```

A workspace root contains only a `[workspace]` table. Its members are normal
projects; the app member also demonstrates a path dependency on the core library
member.

### 11. Stage and package build outputs

```sh
cd examples/hello-cpp
freight install --destdir target/stage --prefix /usr/local
freight package
```

Use `--destdir` for packaging or CI dry runs so files are staged under the
project's `target/` directory instead of written directly to a system prefix.

### 12. Resolve dependencies from a freight registry

These examples require a running freight registry.  The included
`.freight/config.toml` points to `http://localhost:7878`; change the `url`
to match your registry or copy the block to `~/.freight/config.toml` to apply
it globally.

```sh
# Start a local registry (see freight-registry repo for setup).
# Then search and inspect packages before adding them:
freight search zlib --repo local
freight info fmt --repo local
freight info sqlite3 --repo local

# with-registry-dep: C++17, fmt + nlohmann-json from the registry.
cd examples/with-registry-dep
freight fetch          # resolves versions, downloads source archives
freight build
freight run

# with-registry-versions: C11, exact + minimum version constraints, repo pin.
cd examples/with-registry-versions
freight fetch
freight tree           # shows resolved versions
freight run
```

Registry deps behave exactly like any other version dep once fetched — freight
caches the source archive, builds the library, and links it automatically.
