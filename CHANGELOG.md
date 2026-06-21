# Changelog

All notable changes to **freight** are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (while
`0.x`, the `freight.toml` format may still change between releases).

## [Unreleased]

### Added
- **Build plugins (`[plugin]`).** A package can declare `[plugin]`
  (`entry`/`handles`/`tools`) to ship a Rhai script. A project that depends on
  such a package and declares one of the plugin's `handles` sections (e.g.
  `[proto]`, or wildcards like `compiler.*` / `language.**`) runs the script
  during the build with that section's config in `CFG`, project paths as
  constants (`SECTION`, `PROJECT_DIR`, `SRC_DIR`, `INCLUDE_DIR`, `TARGET_DIR`,
  `OUT_DIR`, `PROFILE`), the `PKGS` map of the project's dependencies
  (`PKGS["libfoo"].dir`), the `HOST` / `TARGET` objects (`.os`, `.arch`, `.family`,
  `.pointer_width`, and `TARGET.triple`) for platform branching, the `LIB` /
  `BINS` objects describing the consuming project's library and executables
  (`LIB` an object or `()`; `BINS` a map keyed by executable name),
  build-output functions (`glob`, `run` (allow-listed `tools` only),
  `add_source`/`add_sources`, `add_include_dir`, `define`), Python-flavoured
  project-confined filesystem helpers (`read_text`, `write_text`, `append_text`,
  `copy`, `makedirs`, `listdir`, `exists`, `is_file`, `is_dir`), `capture(tool,
  [args])` (run a tool and get `#{ code, stdout, stderr }` back — for build
  stamping / version probes), pure path/string helpers (`join`, `basename`,
  `dirname`, `stem`, `ext`, `strip`, `lines`), Python `re`-flavoured regex
  (`re_test`, `re_find`, `re_find_all`, `re_replace`) for parsing tool output,
  and `add_flag(tool,
  flag)` to inject a flag into one tool's invocations — `tool` matches a compiler
  by template `name`/`alias`/`family`, the catch-all `"compiler"`, or a role
  (`"linker"`/`"archiver"`); the `TOOLS` constant enumerates valid targets.
  `link_lib(name_or_path)` / `link_dir(path)` add libraries and search dirs to
  the link (sugar over `linker` flags).
  A plugin tool's stdout/stderr now stream into the build output (and stderr is
  included in the error when it fails); `print` shows in the build output too
  (both silent under the LSP). Scripts are bounded by operation/recursion limits
  and file access is confined to the project. Activation is gated by the plugin's `goals` /
  `profiles` (default: all) and made incremental via `inputs` globs (re-run only
  when an input, the `CFG`, or the script changes). This is the general mechanism for
  code generation (protobuf, Qt moc/uic, FlatBuffers, shader compilers,
  `config.h`, build-info stamping, …) — none of it is baked into the core. The
  manifest stays declarative; the script is sandboxed and can only run the tools
  it declares.
- **Plugin distribution from `.pkgs/`.** Plugins are no longer path-deps only:
  freight discovers them from both `path` dependencies **and** packages fetched
  into `.pkgs/` (registry version, git, or archive URL). A fetched plugin runs
  automatically during the build and the LSP refresh, exactly like a path-dep
  plugin — same `tools` allow-list and project-confinement — so vet plugin
  packages before depending on them (a finer capability policy is still to come).
- **`[plugin.schema]` — editor key docs.** A plugin can document the keys its
  handled section accepts (`key = "description"`). `freight lsp` uses it to offer
  completion and hover **inside the consumer's section** (`[proto]` suggests
  `proto_path`, … labelled `plugin: proto`; hovering a key or the section header
  explains it and names the plugin). Purely advisory — never validated.
- **Reference plugins** under `plugins/`: `proto` (protoc), `flatbuffers`
  (flatc), `bison`, and `flex` — add `proto = { path = … }` + a `[proto]`
  section to a project and the codegen runs automatically. `proto` ships a
  `[plugin.schema]` documenting its `proto_path` key.
- **`freight lsp` recognizes plugins.** The server runs the (incremental) plugin
  codegen when it refreshes (on open and on `freight.toml` save), so generated
  files exist on disk, and adds each active plugin's output dir
  (`target/<profile>/plugin-gen/<section>`) to the generated
  `compile_commands.json`. So clangd resolves generated headers (e.g. `foo.pb.h`)
  and freight's undeclared-include check treats them as project-owned — with no
  manual build first. Codegen is best-effort (a missing tool won't break the
  LSP) and incremental (skipped when `inputs` are unchanged). Generated headers
  are also indexed with plugin provenance: `#include` hover, the `← generated
  (<plugin>)` inlay hint, and include completion all credit the plugin that
  produced the file instead of treating it as an ordinary project header.

### Removed
- **Built-in protobuf code generation (`[language.proto]` / `protoc`) is removed.**
  The `proto` codegen pipeline stage, the `build::proto` module, and the
  `.proto`-only source guard are gone. Protobuf is now expressed as a **build
  plugin** (see Added) rather than a hardcoded language key. A `[language.proto]`
  language section is inert. (`protoc` remains usable as a `[build-dependencies]`
  tool, now driven by the proto plugin.)

### Changed
- **Library structure: `Project` is now the central project model at
  `crate::project`** (moved out of `crate::build`, where it was incidentally
  nested). `Project` holds the project/workspace and its packages; the build
  pipeline is one consumer of it. `crate::build::{Project, PackageKind,
  source_package_dirs}` are re-exported, so existing paths keep working.
- New **`crate::environment::Environment`** — a resolved view of *where/how* a
  build runs (host OS/arch, optional cross target triple + parsed target
  OS/arch, sysroot, default backend/debugger, CPU-tuning, jobs), the counterpart
  to `Project`'s *what*. Consolidates facts previously read ad hoc from
  `std::env::consts`, `GlobalConfig`, and triple parsing. `Environment::detect()`
  / `from_config(..)`.
- **Internals now route through `Project`/`Environment`, removing duplication.**
  Target OS/arch resolution (config target → triple → host fallback), which was
  copied in `install` (×3), `build::link`, and `dap`, now goes through one
  `vendor::resolve_target` that `Environment` also uses; the build core
  (`load_project_at`) resolves the env via `Environment::from_config`. The
  default job count lives once in `environment::default_jobs` (the CLI's
  `--jobs` handling and `Environment` share it). The free `install_project` /
  `package_project` / `build_project_at` / `test_project_at` / `bench_project_at`
  now delegate to the matching `Project` methods (the `PipelineOutput` dispatch
  lives only on `Project`). This also fixed `Project::install` silently dropping
  `features` / `default-features`.
- **Environment configuration is centralized on `Environment`.**
  `Environment::for_project()` resolves the merged config layers + `FREIGHT_SYSROOT`
  once, and `Environment::apply_to_manifest()` is the single setter for the
  machine-local `compiler.target`/`sysroot`/`auto-cpu-tuning` — replacing the
  copies in the build core, dependency resolution (`fetch_package_deps`), and the
  LSP (which now also honors per-project `.freight/config.toml`, matching the
  build). The build-session flags moved behind `Environment::verbose()` /
  `offline()` / `locked()` + `set_session_flags()`, so the `FREIGHT_VERBOSE` /
  `FREIGHT_OFFLINE` / `FREIGHT_LOCKED` variable names live in one place instead of
  being read/written ad hoc in `pipeline`, `compile`, `link`, and the CLI.
- **`#include` inlay hints distinguish local from dependency headers** — a
  header from the current project now shows its **directory** (relative to the
  package root, e.g. `← include/geometry`) instead of repeating the project's
  name, while a dependency header still shows the **dependency** it comes from
  (e.g. `← zlib`). System headers remain `← stdlib`.

### Fixed
- **`#include` hints/tooltips now show the version of `.pkgs/` dependencies.**
  Fetched packages live in `.pkgs/<name>` (no version in the directory name), so
  the version parse came up empty and hints rendered `**zlib**/zlib.h` instead of
  `**zlib@1.3.2**/zlib.h`. The LSP header index now reads the version (and name)
  from the fetched package's own `freight.toml`. (The old `name-version`
  dir-name split was removed — `.pkgs/` dirs are always named just `<name>`, so
  the split never helped and could mis-parse a foreign dep name like `foo-2bar`.)
  This fixes the version shown in the inline include hint, the include-completion
  detail, and the C++20 module hints alike (all share the indexed version).
- **C/C++ semantic highlighting through `freight lsp`** — when the native
  Fortran/asm indexers were active (the default), the server advertised its own
  9-type semantic-token legend and stopped forwarding clangd's tokens, so C/C++
  files lost all semantic colouring (only TextMate base highlighting remained).
  The server now advertises clangd's legend when the in-process clang bridge is
  off and forwards clangd's `semanticTokens/full` responses verbatim, while
  remapping the native indexers' tokens into that legend. C/C++ files get
  clangd's semantic colours again; Fortran/asm keep theirs.

### Added
- **Consume Freight packages from other build systems** — the mirror of
  `freight migrate`. `freight install` now also emits a pkg-config descriptor
  (`<prefix>/lib/pkgconfig/<name>.pc`) alongside the installed headers and
  library, so a Freight library is usable from any build system that speaks
  pkg-config: CMake (`pkg_check_modules`), Meson (`dependency()`), autotools
  (`PKG_CHECK_MODULES`), and plain Makefiles. The `.pc` carries
  `Name`/`Description`/`Version`/`Cflags`/`Libs`, and plain version
  dependencies are listed under `Requires.private` (static linking only, so a
  missing module never breaks dynamic-link consumers). A bundled
  `cmake/Freight.cmake` adds an idiomatic CMake front-end: `freight_dependency(
  <name> [SOURCE_DIR …] [PREFIX …] [FEATURES …] [NO_DEFAULT_FEATURES] [REQUIRED]
  …)` builds/installs a Freight project on the fly (or imports an installed one)
  and exposes a `freight::<name>` imported target. See `cmake/README.md`.
- **`freight install` honors `--features` / `--no-default-features`** — install
  now builds with the requested feature set (previously it always built with
  defaults), which is what `freight_dependency(... FEATURES …)` forwards.

- **Native assembly LSP — completed feature set.** The in-process `AsmIndexer`
  (GAS + NASM) now also serves `documentHighlight`, `workspaceSymbol`,
  `selectionRange`, `semanticTokens` (labels/constants/macros under freight's
  token legend), and `rename` (a symbol + all references across the `.include`
  closure), on top of the existing symbols/definition/references/hover/
  completion/folding/diagnostics. No external `asm-lsp` binary needed.

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
