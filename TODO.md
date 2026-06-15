# freight TODO

Sub-TODO files exist for specific areas:
- `docs/TODO.md` — VS Code extension, system lib cache, workspace improvements
- `src/bin/freight/tui/TODO.md` — TUI screens (outdated picker, tree, build panel, test runner)
- Workspace-level `AGENTS.md` — cross-crate tasks and language/toolchain gaps

This file covers items not tracked elsewhere.

---

## High priority

### DAP: additional debugger backends

**End goal:** `freight dap` can debug on every platform freight builds for.
GDB-family and LLDB-family already work; the rest are investigations gated on
tooling/hardware availability (cdb/windbg need a Windows machine).

- [ ] Keep `freight dap` focused on GDB-family (`gdb`, `cuda-gdb`) and LLDB-family
  (`lldb-dap` / `lldb-vscode`) for the current editor MVP.
- [ ] Investigate DAP support for remaining debugger templates:
  - `rr`: replay debugging through GDB DAP or an rr-aware adapter flow.
  - `cdb`: Windows Console Debugger DAP path, if available through VS Code
    debug adapters or Debugging Tools for Windows.
  - `windbg`: WinDbg DAP path, including whether this should be direct or
    VS Code adapter-mediated.
- [ ] Add fake-adapter unit tests and real smoke-test notes before exposing any
  new backend in VS Code or Neovim.

### LSP: clang-bridge to parity with clangd, then default-on

**End goal:** the in-process `clang-bridge` indexer (`src/lsp/indexers/Clang.rs`
+ `crates/clang-bridge`) replaces the clangd subprocess as the default C/C++
backend. No clangd install required; freight controls flags, modules, and the
include policy directly.

**Status:** the bridge implements every LSP method `freight lsp` needs and has
144 passing tests, but it is **off by default** — opt-in at *both* build and run
time: the `clang-bridge` cargo feature (off by default, so a normal `freight`
build doesn't compile the C++ FFI or link libclang) plus the runtime
`freight lsp --use-clang-bridge` flag (ignored with a warning if the feature
wasn't compiled in). clangd is the default C/C++ backend. Not yet differentially
verified against clangd on every method. (This section supersedes the old
libclang/`clang-sys` prototype plan — abandoned in favour of `clang-bridge`.)
To make it default-on later: finish the audit below, then add `clang-bridge` to
the crate's `default` features and flip the runtime gate.

**How to solve:**
- [ ] Finish the clangd-oracle differential audit (driver pattern:
      `/tmp/clangd_probe.py` from the 2026-06-10 session). Remaining methods:
      diagnostics (clangd publishes async — read the raw fd, not a buffered
      stream), signature-help active-parameter tracking, hover content/range,
      call/type hierarchy edges, completion item kinds/details, formatting.
- [ ] UTF-16 position encoding: LSP columns are UTF-16 code units; clang emits
      byte columns. Add a multi-byte fixture line (`// café`) and fix the
      conversion at the Rust LSP layer if (likely) broken.
- [ ] Cross-file / multi-TU: references and workspace symbols that must span
      TUs via `cb_workspace_index_add`; the current fixtures barely cross files.
- [ ] Daily-driver the bridge on the `examples/` projects; when no regressions
      vs clangd remain, flip the default and update the editor extensions.

### Include hygiene: Phases 2–3 (enforce + system libs)

**End goal:** an `#include`/`import` of a header from an undeclared package is
a hard `freight build` error under `[lints].undeclared-include = "deny"`; the
compile command only exposes declared include dirs; declared system libs
resolve their headers via pkg-config.

**Status:** Phases 1–2 shipped. Phase 1: LSP warnings, `[lints]` table, scoped
include completion, include/import inlay hints. Phase 2:
`build::validate_include_hygiene` enforces at build time (`deny` → build error,
`warn` → build warning). Plan: `docs/include-hygiene.md`; running log:
`docs/include-hygiene-audit.md` (Step 10).

**How to solve:**
- [x] Phase 2: pre-compile validation pass in `freight build` re-running the
      Phase-1 `include_policy::check_includes` classification; `deny` →
      `FreightError::UndeclaredInclude`, `warn` → `BuildEvent::Warning`. Fixture
      `examples/broken/undeclared-include/` + integration tests. (Also fixed a
      non-ASCII-comment panic in `parse_includes`.)
- [x] **Phase 3 first cut (makes `deny` safe with system deps):**
      `build::header_ownership` — Tier A (per-OS ownership table: package/slot →
      header globs, in-core seed + downloadable override, fail-open) + Tier B
      (declared dep's pkg-config dedicated dirs, default roots excluded). Wired
      into both the build pass and the LSP: owned headers suppressed, candidates
      named (`<cblas.h> provided by openblas, atlas, mkl`). BLAS/LAPACK modelled
      as slots (shared header = OR). See audit Step 11.
- [x] **POSIX/OS-header policy:** `include_policy::is_os_system_header` +
      `IncludeClass::SystemOs`. POSIX (`unistd.h`, `pthread.h`, `sys/*`, …),
      Windows SDK, Apple/Darwin, and compiler-intrinsic (`*intrin.h`, `arm_*.h`,
      `cpuid.h`) headers are system-provided and never flagged. The set is the
      cross-platform union (unresolvable headers are skipped first, so this is
      safe under cross-compilation).
- [x] **Cross-platform Tier-A seed:** the header→package map is platform-agnostic
      (`zlib.h` ⇒ `zlib` everywhere), so `seed()` is now unconditional (covers
      macOS/Windows) and expanded to common libs whose headers resolve via the
      default search (openssl, libpng, libjpeg, libcurl, zstd, lz4, libevent,
      libpcap, libffi, pcre, yaml, jansson, libzip …).
- [ ] **Per-OS Tier-A data file generation:** hook the vcpkg/registry scraper so
      registry stubs carry `provides-headers` and emit a downloadable override
      (`header-ownership-<os>.toml`, already consumed by `load()`). Cross-crate
      (lives in `vcpkg-converter` + registry), so tracked there.
- [ ] ~~Lazy `pkg-config --list-all` reverse index~~ — dropped: a header is only
      *flagged* when it resolves via the compiler's default search (bare
      `/usr/include` or a subdir of it), which is exactly what pkg-config's
      dedicated-dir lookup can't disambiguate; dedicated-subdir headers (SDL2,
      gtk) don't resolve without their `-I` so are never flagged. The Tier-A seed
      is the correct lever instead.
- [x] Quick-fix code action: on an `undeclared-include` diagnostic the LSP
      offers "Add dependency `<pkg>` to freight.toml" for each Tier-A owner of
      the header, editing `[dependencies]` via `toml_edit` (formatting preserved)
      and merging with clangd's own actions. `undeclared-module` has no owner map
      yet, so no fix is offered there.
- [ ] Phase 2 (stronger, optional): hermetic includes — stop relying on the
      compiler's default search paths so undeclared headers can't even resolve,
      rather than just being flagged after the fact.
- [x] Module→package map so named-module imports (`import foo;`) classify like
      header includes. Done: `lsp::index::ModuleIndex` scans declared packages'
      sources for `export module …;`; `import` hints resolve to `← <pkg>` /
      `← module` / `← stdlib` / `⚠ undeclared`, undeclared modules get an
      `undeclared-module` diagnostic, `import …;` completion offers declared
      modules, and goto-definition jumps to the interface unit.

### Cross-compilation: resolve deps for non-host targets

**End goal:** a cross build (`[compiler] target`/`sysroot`, `--target`,
`FREIGHT_SYSROOT`) never links host libraries — system deps come from the target
sysroot, everything else from a freight-fetched source package built for the
target. Triple and sysroot are complementary (the Yocto/Petalinux pattern):
the compiler gets `--sysroot`, pkg-config is scoped into the sysroot.

**How to solve:**
- [x] `adaptors::cross_build(manifest)` / `is_cross_triple` detect a cross build
      (target ≠ host arch/OS, or a sysroot set).
- [x] `pkg_config_query_cross` runs pkg-config with `PKG_CONFIG_SYSROOT_DIR` +
      `PKG_CONFIG_LIBDIR`/`PATH` restricted to the sysroot (no host `/usr/lib`
      leakage). Compiler `--sysroot` was already emitted by the gnu/llvm/amd/intel
      templates.
- [x] `resolve_version_dep` cross branch: never host pkg-config; sysroot
      pkg-config → libc stub (`-lpthread`, resolved by the cross linker) →
      freight-fetched source (`resolve_fetched_dep`, shared with native) → clear
      error. `fetch_package_deps` status report is sysroot-aware too.
- [x] **Hygiene/LSP under cross:** `include_policy::system_include_dirs` now takes
      the sysroot and probes the compiler with `--sysroot=<path>`, so cross system
      headers resolve against the sysroot, not the host `/usr/include`. Wired into
      the build pass (`manifest.compiler.sysroot`) and the LSP (`cached_system_dirs`,
      re-probes when the active manifest's sysroot changes). Verified e2e with a
      fake sysroot: a sysroot-only header is seen (and `deny`-flagged) only when
      `FREIGHT_SYSROOT` is set.
- [x] **Wildcard removed:** `validate_dep_versions` rejects a bare `*` (and
      empty/omitted version) for version-resolved deps — C/C++ libraries change
      their API between versions, so an unpinned dep is unsafe. The version is the
      same whether the package is already installed (resolved from the system via
      pkg-config) or downloaded from the registry; "installed" just skips the
      download. `path`/`git`/`url` deps and platform pseudo-deps are exempt.
      Generators now emit concrete versions: `freight add` falls back to
      pkg-config `--modversion`; migration pins `--modversion` (or `*` as a draft
      placeholder that build then flags); the LSP quick-fix pins the installed
      version and dep completion inserts a `"${1:version}"` snippet.

---

### LSP: workspace/package recognition

- [x] Treat `[workspace]` manifests as first-class in `freight lsp` diagnostics instead of parsing
  them as package manifests.
- [x] Generate the hidden backend `compile_commands.json` for every workspace member, then point
  clangd at the merged hidden DB instead of requiring a visible workspace-root file.
- [x] Track `[[bin]]` and `[lib]` targets across workspace members so IDEs can offer target/package
  choices for run, debug, build, and source navigation.
- [x] Build the doc hover index across explicit path dependencies, not just workspace members
  and the nearest manifest's `src/` tree. Workspace member indexing is done.
- [x] Refresh the workspace compile DB and doc index when any member `freight.toml` changes.

### LSP ↔ build/: remove duplicated package/source discovery

**End goal:** the LSP derives "what packages and headers exist" from the same
`build/` primitives the compile path uses, with no parallel re-implementation.

**How to solve:**
- [x] Shared package enumerator: `build::source_package_dirs` (project +
      workspace members + path deps, read-only, tolerant of unfetched deps),
      consumed by `lsp::refresh_header_index` (replaced the old
      `build_header_specs`/`collect_path_dep_specs`).
- [x] Manifest-load cache: `manifest::load_manifest_cached` (mtime-validated)
      for read-heavy LSP callers; build/compile path stays on uncached
      `load_manifest`.
- [x] Single `src/` walk per package shared by the header and module indexes:
      `lsp::index::build_source_indexes` walks each package's `src/` once,
      classifying headers and `export module` declarations in the same pass
      (`HeaderIndex::build`/`ModuleIndex::build` removed; the LSP refresh calls
      the combined builder). `build::discover` stays separate — it is the
      compile-path walk (single project, template-keyed languages, conditional
      `[os.*]`/`[arch.*]` globs), a genuinely different scope.
- [x] `ServerState` holds an owned project model (`active_manifest` +
      `package_dirs`), recomputed once per manifest-set change in
      `refresh_project_model` (driven from `refresh_compile_commands`). The
      header/module refresh and the per-keystroke manifest read sites
      (`undeclared_include_level`, `declared_dep_names`, sysroot) consume it
      instead of re-deriving. (The build's `Project` itself isn't held — it
      implies fetch/resolve, which the LSP must never do.)

### LSP: native Fortran support via `fortran-lsp`

**End goal:** Fortran files are served by the workspace's `crates/fortran-lsp`
embedded as a `LanguageIndexer` (like ClangIndexer), scoped by freight's
manifest source graph. `fortls` remains a reference implementation and
flag-gated fallback until removal.

**Status:** `fortran-lsp` already covers parsing (free/fixed form, preprocessor
evaluation, recursive includes), indexing, hover, definition, completion,
signature help, references, and a broad diagnostic set (48 tests) — but
**`freight lsp` does not call it yet**; Fortran traffic still goes to fortls.

**How to solve:**
- [ ] `FortranIndexer` in `src/lsp/indexers/` wrapping `fortran_lsp::Workspace`:
      feed it manifest source roots + include dirs; route Fortran URIs to it
      behind a `--use-native-fortran` flag (mirror the clang-bridge gating).
- [ ] Map `fortran-lsp` model types to LSP responses for supported methods;
      forward unsupported methods to fortls while gaps remain.
- [ ] Differential-test against fortls (same oracle technique as clang-bridge
      vs clangd), close gaps, then flip the default.
- [ ] See `crates/fortran-lsp/TODO.md` for crate-side gaps.

### LSP: native assembly support (`AsmIndexer`)

**End goal:** `.s`/`.S`/`.asm`/`.nasm` files are served by a native
`AsmIndexer`, so the external `asm-lsp` binary is not required. (asm-lsp is pure
Rust, so "native" here means an in-process indexer — same self-contained goal as
clang-bridge/fortran-lsp.)

**Status:** implemented and default-on — `src/lsp/indexers/Asm.rs` (GAS + NASM).
`--no-native-asm` falls back to the external `asm-lsp` passthrough; otherwise
that passthrough is not started and asm requests route to `AsmIndexer`.
Comment/string-aware tokenizer; `%`-registers and `$`/`@` sigils handled. 16 unit
tests + end-to-end verified through `freight lsp`.

Implemented:
- **Symbols** — labels, constants (`.equ`/`.set`/`.equiv`, GAS `name = …`, NASM
  `name equ …`/`%define`/`%assign`), macros (`.macro`/`%macro`); each with
  documentSymbol (kinded), goto, references (honours `includeDeclaration`),
  hover, completion.
- **Numeric local labels** — `1:` with directional `1f`/`1b` goto.
- **Cross-file resolution** — goto / references / hover / completion follow the
  transitive `.include`/`%include` closure from the queried file (BFS,
  cycle-safe; the current file ranks before its includes). `.include "file"`
  navigation also opens the target.
- **Macro awareness** — `.macro foo a, b` parameters and `\arg` references are
  treated as macro-locals (not global symbols); labels defined inside a macro
  body are excluded from duplicate-symbol diagnostics (templated per expansion
  via `\@`).
- **Hover** — symbol provenance (with the defining file for cross-file symbols) +
  curated per-arch **instruction** / **register** tables (x86-64, AArch64,
  RISC-V) + **directive** help, dispatched by cursor context (mnemonic vs
  operand). Arch comes from the manifest `[target] arch` (or the host) via
  `refresh_flags`; an unknown arch tries every table.
- **Folding** — `.macro`/`.rept`/conditional blocks and per-label regions.
- **Diagnostics** — duplicate symbol definition.

**Remaining / how to grow it:**
- [ ] **Fuller instruction/register DB** — the curated x86-64/AArch64/RISC-V
      tables cover common mnemonics/registers; fuller coverage could embed the
      upstream `asm-lsp` crate's data tables rather than hand-rolling.
- [ ] **Semantic tokens** — only once freight owns the global legend (see the
      clang-bridge legend note); otherwise leave to TextMate.
- [ ] Consider extracting the parser into a `crates/asm-lsp`-style crate if it
      grows (kept inline in `Asm.rs` for now).

### ~~Compiler version gating for language standards~~
Done. `TemplateDef` now has `standard_min_versions`; `CompilerTemplate::check_standard_floor`
checks the floor; `compile_one` rejects unsupported standards with `FreightError::OptionError`
before invoking the compiler.

Floors set for GCC (g++/gcc/gfortran) and Clang (clang++/clang):
- c++20 ≥ GCC 10 / Clang 10; c++23 ≥ GCC 12 / Clang 14; c++26 ≥ GCC 14 / Clang 17
- c17 ≥ GCC 8 / Clang 6; c23 ≥ GCC 14 / Clang 17; f2018 ≥ gfortran 8

---

## Build pipeline

### ~~`has_lang` is duplicated~~
Done. Extracted `pub(super) fn has_lang` to `build/mod.rs`; both `compile.rs` and `link.rs` call `super::has_lang`.

### ~~Linker priority list is fragile~~
Done. `LINK_PRIORITY: &[&str]` constant defined in `link.rs`; `select_linker` references it.

### ~~Missing `BuildEvent` for whole-program mode~~
Done. `BuildEvent::Compiling` emitted before `gnatmake` invocation in `compile.rs`.

---

## Migration tool

### ~~Autotools: SUBDIRS not auto-detected~~
Done. `migration/autotools.rs` now parses `SUBDIRS = ...`, recurses into each
listed directory that has a `Makefile.am`, writes a `freight.toml` there, and
adds any library targets as `{ path = "subdir" }` deps in the root manifest.
Subdirs with only `bin_PROGRAMS` are migrated independently but not added as
deps. Missing subdirs are skipped with a warning.

### ~~CMake migration: platform-conditional block handling~~
Done. `cmake_lossless::eval::platform_condition` identifies `if(WIN32)`, `if(APPLE)`,
`if(UNIX)`, etc. and routes their deps to `[os.windows.dependencies]`,
`[os.macos.dependencies]`, `[os.unix.dependencies]` in the emitted `freight.toml`.
`elseif` chains each get their own scope; `else` falls through to unconditional.
Defines/includes inside platform blocks are still dropped (freight.toml has no
per-platform define syntax).

---

## Testing

- Integration test for mixed-language linking: build `examples/mixed/c-cpp` and
  `examples/mixed/tri-lang` via the `freight` API; assert the binary exits 0.
- Unit test for `whole_program: true` branch in `compile.rs` / `link.rs`.
- Unit test for language auto-detection via `has_lang` (extension → linker family).
- Tests for compiler version gating once implemented.

---

## Examples / language support

See `AGENTS.md` for full detail. Summary of what's missing:

| Example         | Status / Blocker                                      |
|-----------------|-------------------------------------------------------|
| OpenCL          | ✓ Done — `examples/opencl-hello/`                    |
| CUDA            | ✓ Done — `examples/cuda-hello/`                      |
| D               | ✓ Done — `examples/d-hello/` (ldc2 + dmd)            |
| ObjC / ObjC++   | ✓ Done — `examples/objc-hello/`, `examples/objcpp-hello/` |
| HIP             | ✓ Done — `examples/hip-hello/` (requires ROCm hardware) |
| ISPC            | ✓ Done — `examples/ispc-hello/`                       |
| GDC             | ✓ Available (`gdc` 16.1.1); `d-hello` already works  |
| MSVC            | Windows machine needed                                |
| nvfortran       | NVIDIA HPC SDK needed                                 |

---

## Documentation

- `docs/manifest-reference.md`: add `[language.ada]` section.
- `examples/README.md`: keep prerequisite notes current as new toolchain examples land.
