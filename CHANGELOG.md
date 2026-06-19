# Changelog

All notable changes to **freight** are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (while
`0.x`, the `freight.toml` format may still change between releases).

## [Unreleased]

### Changed
- The default build profile is renamed **`dev` → `debug`** (so output lands in
  `target/debug/`, matching the `Debug` CMake config name). Old manifests keep
  working: `[profile.dev]` is accepted as an alias for `[profile.debug]`, and the
  `dev` profile name still resolves.

### Added
- **`include/` auto-detected as an include directory** (alongside `inc/`) — the
  dominant C/C++ convention, and what most migrated CMake libraries use, so they
  build natively without extra configuration.
- **`[lib].srcs` is now optional** — a library with no `srcs` is compiled from
  the auto-discovered `src/` tree (like a target-less project). Validation no
  longer requires `srcs`; a genuinely source-less build is still caught at build
  time. Lets a migrated CMake library (whose sources were a `file(GLOB …)`) build
  from its `src/` directory.
- **Foreign packages build standalone** — `freight build` on a package whose
  `[package]` declares `url`/`build` and has no native targets (a vcpkg-scraper
  port) now fetches + foreign-builds it and places the produced library in
  `target/<profile>/`. Validation and source discovery allow such target-less
  foreign packages. With workspace `[patch]` wiring, `freight build -p <port>`
  builds a port and its (transitive, foreign) dependencies — e.g. `libpng` pulls
  and builds `zlib` and links against it.
- **Transitive foreign dependencies** — a foreign package's own foreign deps are
  now discovered and built (the foreign-member sub-graph), in topological order,
  and each build receives its already-built dependencies' install prefixes via
  `CMAKE_PREFIX_PATH` (so a member's `find_package` finds them). Built libraries
  are returned dependent-first for correct static link order. A full
  `app → mid → base` cmake chain now builds + links + runs offline.
- **Foreign packages** — a package whose `[package]` declares `url` + `build`
  (no local `[lib]`/sources) is itself fetched from `url` and built with the
  named foreign build system (`cmake`/`make`/…), then exposed to dependents. This
  is the shape `vcpkg-scraper` emits, so a vendored upstream can be a workspace
  member or a `[patch]` target and build offline. `[package].patches` are applied
  to the fetched source.
- `build_foreign_deps` now honors `[patch]`: a dependency patched to a local path
  resolves to (and links) that member instead of falling through to
  pkg-config/the registry (previously it built the member but then failed
  resolution with "dep not found").

### Added (earlier)
- Source discovery now also compiles the files listed in `[lib].srcs` /
  `[[bin]].src` (in addition to the `src/**` walk, de-duplicated), and adds the
  parent dirs of `[lib].hdrs` to the include path. This lets projects whose
  sources live at the repo root (or in a shared tree referenced via `../`) build
  without relocating files — including migrated and workspace-member packages.
- `freight migrate cmake` now emits a **workspace** when a project defines more
  than one library (a freight package has at most one `[lib]`): one member
  package per library plus one per executable, each referencing the shared
  sources by relative path.

### Fixed
- `freight migrate` emitted `[[lib]]` (array of tables) which does not match the
  manifest's single `lib` field — the output failed to parse. Now emits a single
  `[lib]` table (cmake/make/autotools); make/autotools warn and keep the first
  library when several are present.
- `freight migrate cmake` no longer emits CMake `-U<name>` undefines (or other
  non-`-D` flags) as defines, which produced an invalid `-D-U…` and broke the
  build.

### Changed
- `freight migrate cmake|make|autotools` now folds in a sibling `vcpkg.json`'s
  declared dependencies — with override versions, features, `default-features`,
  and platform conditions (→ `[os.*.dependencies]`) — on top of the targets and
  standards reconstructed from the build system. Versions resolve from
  `vcpkg.json` overrides, then `pkg-config`, then the `"*"` draft placeholder.
  This turns a CMake/Make project that declares its deps in `vcpkg.json` into a
  complete, buildable `freight.toml` in one step.

## [0.1.1] — 2026-06-16

First public release. (The `0.1.0` tag was reserved by a failed CI run under
GitHub's immutable-releases feature, so the initial release ships as `0.1.1`.)

A Cargo-inspired build tool and package manager for
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

[Unreleased]: https://github.com/freight-app/Freight/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/freight-app/Freight/releases/tag/v0.1.1
