# freight TODO

Sub-TODO files exist for specific areas:
- `docs/TODO.md` — VS Code extension, system lib cache, workspace improvements
- `src/bin/freight/tui/TODO.md` — TUI screens (outdated picker, tree, build panel, test runner)
- Workspace-level `AGENTS.md` — cross-crate tasks and language/toolchain gaps

This file covers items not tracked elsewhere.

---

## High priority

### ~~Compiler version gating for language standards~~
Done. `TemplateDef` now has `standard_min_versions`; `CompilerTemplate::check_standard_floor`
checks the floor; `compile_one` rejects unsupported standards with `FreightError::OptionError`
before invoking the compiler.

Floors set for GCC (g++/gcc/gfortran) and Clang (clang++/clang):
- c++20 ≥ GCC 10 / Clang 10; c++23 ≥ GCC 12 / Clang 14; c++26 ≥ GCC 14 / Clang 17
- c17 ≥ GCC 8 / Clang 6; c23 ≥ GCC 14 / Clang 17; f2018 ≥ gfortran 8

---

## Build pipeline

### `has_lang` is duplicated
`compile.rs` and `link.rs` each contain a private `has_lang` closure. Extract into a
shared free function in `build/mod.rs`.

### Linker priority list is fragile
`select_linker` in `link.rs` uses a hard-coded `&[&str]` priority slice. Adding a new
language key requires updating two separate places with no compile-time enforcement.
Define a single `LINK_PRIORITY` constant with tier comments.

### Missing `BuildEvent` for whole-program mode
When `gnatmake` runs (Ada whole-program mode) no compile-phase events are emitted.
Add `BuildEvent::Compiling` before the whole-program linker invocation.

---

## Migration tool

### Autotools: SUBDIRS not auto-detected
`migration/autotools.rs` warns when `Makefile.am` has `SUBDIRS` but does not walk
subdirectories to produce workspace members. Should recurse into each subdir.

### ~~CMake migration: platform-conditional block handling~~
Done. `cmake_lossless::eval::platform_condition` identifies `if(WIN32)`, `if(APPLE)`,
`if(UNIX)`, etc. and routes their deps to `[os.windows.dependencies]`,
`[os.macos.dependencies]`, `[os.unix.dependencies]` in the emitted `freight.toml`.
`elseif` chains each get their own scope; `else` falls through to unconditional.
Defines/includes inside platform blocks are still dropped (freight.toml has no
per-platform define syntax).

---

## Testing

- Integration test for mixed-language linking: build `examples/multi-lang` and
  `examples/tri-lang` via the `freight-core` API; assert the binary exits 0.
- Unit test for `whole_program: true` branch in `compile.rs` / `link.rs`.
- Unit test for language auto-detection via `has_lang` (extension → linker family).
- Tests for compiler version gating once implemented.

---

## Examples / language support

See `AGENTS.md` for full detail. Summary of what's missing:

| Example         | Blocker                              |
|-----------------|--------------------------------------|
| ObjC / ObjC++   | GNUstep setup; macOS native trivial  |
| HIP             | Requires ROCm hardware               |
| OpenCL          | ICD loader + any OpenCL platform     |
| ISPC            | `ispc` on `$PATH`                    |
| GDC             | `libgphobos` vs `libphobos2` check   |
| MSVC            | Windows machine needed               |
| nvfortran       | NVIDIA HPC SDK needed                |

---

## Documentation

- `docs/manifest-reference.md`: add `[language.ada]` section.
- `examples/README.md`: add rows for ObjC, HIP, OpenCL, ISPC once created.
