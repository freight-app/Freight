# freight TODO

Sub-TODO files exist for specific areas:
- `docs/TODO.md` — VS Code extension, system lib cache, workspace improvements
- `src/bin/freight/tui/TODO.md` — TUI screens (outdated picker, tree, build panel, test runner)
- Workspace-level `AGENTS.md` — cross-crate tasks and language/toolchain gaps

This file covers items not tracked elsewhere.

---

## High priority

### Compiler version gating for language standards
`std = "c++26"` silently passes `-std=c++26` even on GCC 11, producing a confusing
compiler error instead of a clear freight message.

- Extend `TemplateDef::standards` entries to carry `min_compiler_version`.
- Check detected version against the floor in `assemble_compile_flags`.
- Emit `FreightError` (or `BuildEvent::Warning`) when the compiler is too old.
- Known floors: `c++20` ≥ GCC 10 / Clang 10; `c++23` ≥ GCC 12 / Clang 14;  
  `c++26` ≥ GCC 14 / Clang 17; `c17` ≥ GCC 8 / Clang 6; `f2018` ≥ gfortran 8.

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

### CMake migration: platform-conditional block handling
The cmake migrator currently drops all `if` block contents. Once cmake-lossless adds
an `if` condition evaluator, platform blocks (`if(WIN32)`, `if(APPLE)`) can be mapped
to `[os.windows.dependencies]` / `[os.macos.dependencies]` instead of being lost.

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
