# AGENTS.md

Outstanding tasks, known gaps, and work items for Freight. Update this file when tasks are started, completed, or reprioritised.

---

## High priority

### Per-standard compiler version gating
**Status:** Not implemented  
**Why it matters:** `std = "c++26"` silently passes `-std=c++26` even on GCC 11, which errors at compile time with a confusing message instead of a clear freight error.  
**What to build:**
- Extend `TemplateDef::standards` from `&[(&str, &str)]` to `&[(&str, &str, Option<&str>)]` — `(name, flag, min_compiler_version)`.
- Propagate through `TemplateDef::build()` and `CompilerTemplate`.
- In `assemble_compile_flags`, when the user requests a standard, check the detected compiler version against the entry's `min_compiler_version`. Emit a `FreightError` (or `BuildEvent::Warning`) if the version is too old.
- Fill in version floors for existing templates (GCC, Clang, gfortran, MSVC, icpx, ldc2…).
- Known floors to encode: `c++20` ≥ GCC 10 / Clang 10, `c++23` ≥ GCC 12 / Clang 14, `c++26` ≥ GCC 14 / Clang 17; `f2018` ≥ gfortran 8; `c17` ≥ GCC 8 / Clang 6.

### ObjC / ObjC++ examples
**Status:** Not done  
**What to build:** `examples/objc-hello/` and `examples/objcpp-hello/` — minimal programs that compile with `clang`/`clang++` on Linux (with GNUstep) and macOS (native frameworks). Add rows to `examples/README.md` and `README.md`.

---

## Language / toolchain gaps

### HIP example
**Status:** Not done  
**Blocker:** Requires ROCm hardware or a ROCm Docker image to verify.  
**What to build:** `examples/hip-hello/` — simple vector-add kernel with `hipMalloc`/`hipMemcpy`. Mirror the structure of `cuda-hello`. Note in the README that ROCm hardware is required.

### OpenCL example
**Status:** Not done  
**What to build:** `examples/opencl-hello/` — host-side C with a `.cl` kernel. Should work on any system with an ICD loader and at least one OpenCL platform.

### ISPC example
**Status:** Not done  
**What to build:** `examples/ispc-hello/` — simple SPMD reduction or mandelbrot kernel. Requires `ispc` on `$PATH`.

### GDC (GCC D compiler) support verification
**Status:** Unknown  
**What to check:** `gdc` is listed as a supported D compiler but has not been tested end-to-end. Verify `examples/d-hello` builds with `gdc` and that the linker selection correctly handles `libgphobos` vs `libphobos2`.

### MSVC / Windows support
**Status:** Partially implemented (templates exist)  
**What to verify:** Build `examples/c-simple` and `examples/hello-cpp` on Windows with MSVC (`cl.exe`). Cross-compilation from Linux via Wine is not expected to work.

### nvfortran (NVIDIA Fortran)
**Status:** Template exists, untested  
**What to verify:** Build `examples/fortran-hello` with `nvfortran`. Check that Fortran standard flags map correctly.

---

## Code quality / pipeline

### Linker priority list in `link.rs` is a raw string slice
**Status:** Fragile  
**Why it matters:** Adding a new lang key requires knowing to update two separate priority slices (`select_linker`) and there is no compile-time enforcement.  
**Proposal:** Define a single `LINK_PRIORITY` constant with comments explaining the tiers (GPU accelerator > C++ ABI > C ABI > specialised > assembly). The current ad-hoc slice works but is easy to get wrong.

### `has_lang` is duplicated between `compile.rs` and `link.rs`
**Status:** Known duplication  
**Fix:** Extract into a free function in `build/mod.rs` or a small `build/util.rs`, import in both files.

### `REQUIRES_DECLARATION` comment is thin
**Status:** Minor  
**Fix:** Add an inline comment per entry explaining *why* that lang key must be declared (which extension it shares with C/C++).

### Whole-program build mode needs a `BuildEvent`
**Status:** Missing observability  
**Why:** When gnatmake is invoked, freight currently emits no compile-phase events (because compilation is skipped). The user sees silence until the link step. Emit a `BuildEvent::Compiling` or similar before the whole-program linker invocation so the TUI/CLI shows progress.

---

## Testing gaps

### No integration tests for mixed-language linking
**Status:** Missing  
**What to add:** A test that builds `examples/multi-lang` and `examples/tri-lang` programmatically (via `freight-core` API, not shell) and asserts the binary exists and exits 0.

### No test for `whole_program` mode
**Status:** Missing  
**What to add:** Unit test in `compile.rs` or `link.rs` that exercises the `whole_program: true` branch with a mock template.

### No test for language auto-detection via `has_lang`
**Status:** Missing  
**What to add:** Test cases in `compile.rs` confirming that a manifest with no `[language.X]` section but a matching source extension is correctly routed to the right linker family.

### Compiler version gating (once implemented)
**Status:** Blocked on implementation  
**What to add:** Tests that a requested standard with a too-old compiler version produces a `FreightError::UnsupportedStandard` (or equivalent), not a silent wrong flag.

---

## Documentation

### `docs/manifest-reference.md` — `[language.ada]` section missing
**Status:** Not done  
**What to add:** Document the `ada` language key, supported options (opt-level, debug, warnings), and note that `[language.ada]` is optional when no configuration is needed.

### `examples/README.md` — ObjC, HIP, OpenCL, ISPC rows missing
**Status:** Blocked on example creation above  
**Fix:** Add rows once the examples exist.
