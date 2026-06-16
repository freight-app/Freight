# Changelog

All notable changes to **freight** are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (while
`0.x`, the `freight.toml` format may still change between releases).

## [Unreleased]

## [0.1.0] — 2026-06-16

First public release: a Cargo-inspired build tool and package manager for
compiled languages that target GCC or Clang. A single `freight.toml` replaces
Makefiles and CMake for C, C++, Fortran, CUDA, HIP, OpenCL, ISPC, D, Ada,
Objective-C, and assembly projects.

### Build engine
- Parallel, incremental compilation (mtime + `.d`/`/showIncludes` header tracking).
- Binary, static-library, and shared-library targets; multi-language projects.
- C++20 named modules (DAG-ordered batch build) and precompiled headers.
- Profiles (`dev`/`release`/custom) with opt-level, LTO, strip, sanitizers.
- Protobuf codegen (`[language.proto]`), CPU-feature flags (`[arch.*]`),
  platform-conditional sources (`[os.*]`/`[arch.*]`).
- Multiple compiler toolchains via Rhai templates (gcc, clang, MSVC, Intel,
  NVHPC, …) with guest compilers (nvcc, hipcc, nasm, …).

### Dependencies
- `[dependencies]` / `[build-dependencies]` / `[dev-dependencies]`.
- Path, git (branch/tag/rev), URL archive (`.tar.gz`/`.tar.xz`/`.tar.bz2`/`.zip`,
  with optional SHA-256), and version deps.
- Resolution chain for version deps: pkg-config → bundled system-lib stubs →
  registry. A concrete version or range is required (bare `*` rejected).
- Foreign build systems: CMake, Meson, Autotools, Make, SCons, Bazel — fetched,
  built, and linked automatically; header-only auto-detection.
- `[features]` (Cargo-style: `dep:name`, `define:NAME`, `<dep>/define:NAME`),
  `provides`/slot conflict resolution, and a `freight.lock` lockfile.

### Cargo-parity
- `[patch]` source overrides (path, across the whole graph including transitive).
- Workspaces with `[workspace.dependencies]` / `[workspace.package]` inheritance
  (`{ workspace = true }`).
- `[[bin]] required-features` and `[package] default-run`.
- `[[example]]` targets + `examples/` auto-discovery (`--example`/`--examples`).
- `--offline` / `--locked` / `--frozen`; `[alias]` command aliases.
- `freight metadata` (JSON graph); `freight tree` shows all dep kinds + `--depth`.

### Commands
`new`, `init`, `build`, `run`, `test`, `bench`, `debug`, `watch`, `add`,
`remove`, `update`, `fetch`, `tree`, `metadata`, `workspace`, `outdated`, `info`,
`search`, `check`, `clean`, `compile-commands`, `doc`, `install`, `package`,
`fmt`, `lint`, `migrate`, `lsp`, `toolchain`, `publish`/`login`/`logout`/
`register`/`yank`, `completions`.

### Tooling
- `freight lsp` — `freight.toml` diagnostics + clangd/fortls passthrough.
- `freight doc` — Markdown/JSON API docs via docify, plus a TUI browser.
- `freight migrate cmake|make|autotools` — convert foreign build systems.

[Unreleased]: https://github.com/freight-app/Freight/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/freight-app/Freight/releases/tag/v0.1.0
