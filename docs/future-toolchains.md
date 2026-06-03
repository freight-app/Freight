# Future Toolchain & Language Support

This document lists compilers, assemblers, debuggers, and language extensions worth adding
templates for. Each entry includes what makes it interesting, what a template would require,
and any known technical challenges.

All templates are now hardcoded Rust in `crates/freight/src/toolchain/builtin/`.
Items marked **[needs Rust]** require changes beyond a new template file.

---

## C / C++ Compilers

### `zig cc` / `zig c++` ✓ template exists
- `builtin/zig/mod.rs` — `zig-c` and `zig-c++` templates.
- Uses the new `subcommand` field on `CompilerTemplate`: binary=`zig`, subcommand=`cc`/`c++`.
  The subcommand is inserted as the first argument after the binary in both compile and link steps.
- Cross-compilation via `--target={triple}`.

### TCC (Tiny C Compiler) ✓ template exists
- `builtin/misc/mod.rs` — C-only; no `-MMD` dep file support (mtime-only dirty checking).

### MSVC (`cl.exe`) ✓ template exists
- `builtin/windows/mod.rs` — full `/O`/`/W`/`/Zi` flag translation, `/showIncludes` dep tracking, `link.exe` linker.

### `clang-cl` ✓ template exists
- `builtin/windows/mod.rs` — Clang with MSVC-compatible flags; `llvm` family, `lld-link`/`llvm-lib` toolset.

### Intel oneAPI C++ (`icpx`) ✓ template exists
- `builtin/intel/mod.rs` — includes SYCL support (`-fsycl` always-flag, `.sycl` extension).

### PGI / NVHPC (`nvc++`) ✓ template exists
- `builtin/nvidia/mod.rs` — `nvc`, `nvc++`, `nvfortran`; family `"nvidia"`.

### Circle (`circle`) ✓ template exists
- `builtin/misc/mod.rs` — Clang-compatible flags, version parsed from `circle --version` build number.

### Emscripten (`emcc`) ✓ template exists
- `builtin/emscripten/mod.rs` — `emcc` (C) and `em++` (C++) templates. GCC-compatible flags;
  `emar` as archiver. Output extension limitations (`.wasm` vs `.js`) not yet handled — freight
  treats output like a native binary.

### wasi-sdk ✓ template exists
- `builtin/emscripten/mod.rs` — `wasi-clang` template with `always_flags = ["--target=wasm32-wasi"]`.
  Alias responds to `wasi-clang++`.

---

## Fortran Compilers

### Intel Fortran (`ifx`) ✓ template exists
- `builtin/intel/mod.rs` — `-warn all/errors`, `-ipo` LTO, `-cpp -MMD -MF` dep tracking.

### Flang (LLVM Fortran) ✓ template exists
- `builtin/llvm/mod.rs` — tracks `flang` / `flang-new` binary names via version regex.

### NAG Fortran (`nagfor`) ✓ template exists
- `builtin/misc/mod.rs` — `-gnatw`-style warning flags, `-halt=error` for warnings-as-errors,
  standard flags `-f95`/`-f2003`/`-f2008`/`-f2018`.

---

## Assembly

### YASM ✓ template exists
- Already present in `toolchains/yasm.rhai`. `requires_toolchain = ["c"]` makes it a guest extension.

### FASM (Flat Assembler)
- **What**: Self-hosted assembler with its own syntax. Used in OS development and low-level work.
  Produces flat binaries, ELF, PE, COFF.
- **Template**: Different syntax for output format selection. Simpler flag set.
- **Challenge**: FASM's output format is specified inside the source file (`format ELF64`), not
  via a command-line flag — the `[arch_flags]` approach doesn't map cleanly.

### MASM (`ml` / `ml64`) ✓ template exists
- `builtin/windows/mod.rs` — `masm` template, Windows-only, `.asm`/`.masm` extensions,
  `/I{path}` includes, `/Fo {path}` output, guest compiler (`requires_toolchain = ["cpp"]`).

### GNU ARM Assembler (via `arm-none-eabi-gcc`)
- **What**: GAS targeting bare-metal ARM Cortex-M / Cortex-A. Used in embedded development.
- **Template**: Extend the GAS entries in `gcc.rhai` to add `[arch_flags]` for `"arm"`,
  `"armv7"`, `"cortex-m4"` etc.

### RISC-V GAS (via `riscv64-linux-gnu-gcc`)
- **What**: GNU assembler for RISC-V targets.
- **Template**: Extend the GAS entries in `gcc.rhai` with RISC-V arch flags.

---

## GPU / Parallel / Special

### CUDA via HIP (`hipcc`) ✓ template exists
- Already present in `toolchains/amd/hipcc.rhai`. `family = ""`, `requires_toolchain = ["cpp"]` — guest extension.

### NVCC (`nvcc`) ✓ template exists
- Already present in `toolchains/nvidia/nvcc.rhai`. `family = ""`, `requires_toolchain = ["cpp"]` — guest extension.
  May benefit from `arch_flags` for `-gencode` per SM architecture.

### Intel DPC++ (`dpcpp` / `icpx -fsycl`) ✓ template exists
- Covered by `builtin/intel/mod.rs` `icpx` template — `always_flags = ["-fsycl"]`,
  `.sycl` extension, `sycl` language key with compatible `["c++", "c", "fortran"]`.

### OpenCL kernel compiler (`clang -x cl`) ✓ template exists
- Already present in `toolchains/opencl.rhai`. `requires_toolchain = ["cpp"]` — guest extension.

### Metal shader compiler (`xcrun metal`)
- **What**: Apple's GPU shading language compiler for iOS/macOS. Compiles `.metal` files to
  `.air` (intermediate) then `metallib` archives.
- **Template [needs Rust]**: Two-step compilation (`.metal` → `.air` → `.metallib`) resembles
  C++20 module compilation. Would need a dedicated pipeline step.

### GLSL / HLSL / SPIR-V (`glslangValidator`, `dxc`, `spirv-cross`)
- **What**: GPU shader compilation for Vulkan, DirectX, OpenGL.
- **Template**: Each tool has its own flag scheme. Output formats are non-standard (`.spv`, `.dxil`).
- **Challenge [needs Rust]**: Output file extensions and linking semantics differ completely from
  native object files. Shader "linking" means bundling into a binary or embedding in a C header.

### ISPC (`ispc`) ✓ template exists
- Already present in `toolchains/intel/ispc.rhai`. `requires_toolchain = ["cpp"]` — guest extension.

---

## Other Languages

### D (`dmd` / `ldc2` / `gdc`) ✓ templates exist
- `toolchains/dmd.rhai` covers the reference compiler.
- `toolchains/llvm/ldc2.rhai` covers the LLVM-based D compiler.
- `toolchains/gnu/gdc.rhai` covers the GCC-based D compiler.
- **ABI compatibility**: D's ABI is compatible with C (`extern(C)`) but not C++ by default.
  The `linking["d"]` ABI key handles this.

### Ada (`gnat`) ✓ template exists
- `builtin/misc/mod.rs` — `.adb`/`.ads` extensions, `-gnatwa`/`-gnatwe` warning flags,
  `-gnat83` through `-gnat2022` standards. Single-file compilation only; `gprbuild`-style
  multi-unit projects are not yet handled.

### Objective-C / Objective-C++ (via Clang) ✓ template support exists
- **What**: Clang compiles `.m` and `.mm` files natively.
- **Template**: `clang.rhai` claims `.m` as `objc`; `clang++.rhai` claims `.mm` as `objcpp`.
  Platform frameworks such as `-framework Foundation` can be supplied through manifest linker flags or a build script.

### Swift (`swiftc`) ✓ template exists
- `builtin/misc/mod.rs` — `-Onone`/`-O`/`-Osize` opt levels, `-suppress-warnings`/`-warnings-as-errors`,
  `-lto=llvm-full`. Produces object files linkable with C.
- **Limitation**: Swift module inter-dependencies (`.swiftmodule`) are not tracked; only
  single-translation-unit or pre-built module use cases work today.

### Zig (`zig build-lib` / `zig build-exe`)
- **What**: Zig's native compiler. Output is linkable with C. Zig has its own build system but
  could be used as a library producer.
- **Challenge [needs Rust]**: Zig doesn't use a separate compile-then-link flow in the same way.
  Closest analogy would be generating a static lib via `zig build-lib`.

### Rust (`rustc`)
- **What**: Compile Rust libraries as `.rlib` or `cdylib` for FFI use from C/C++.
- **Challenge [needs Rust]**: Rust has its own dependency resolution (Cargo). `rustc` directly
  is feasible for single-file libs but impractical for anything with Cargo dependencies.

---

## Debuggers

### `rr` (Mozilla Record & Replay) ✓ template exists
- `debugger.rs` — `separator = "replay"` (rr's subcommand), `chaos` and `no_syscall_buffer`
  settings. No DAP support. `freight debug --debugger rr` selects it; a dedicated
  `--record`/`--replay` CLI split is still pending.

### WinDbg / CDB (Windows) ✓ template exists
- `debugger.rs` — `cdb` (console, `-nologo` default arg) and `windbg` templates.
  Neither has DAP support; WinDbg Preview DAP can be added when it stabilises.

### OpenOCD + GDB (Embedded)
- **What**: Open On-Chip Debugger connects to embedded targets (JTAG/SWD). Works with GDB as
  the front-end.
- **Template addition**: A `toolchains/debuggers/openocd-gdb.rhai` that launches `openocd` as
  a background process then connects `gdb` to its GDB server port.
- **Challenge [needs Rust]**: Two-process launch doesn't fit the current single-binary
  `launch_command()` model.

### LLDB-DAP standalone
- Already detected via `dap.binaries = ["lldb-dap", "lldb-vscode"]` in `lldb.rhai`.
  No additional template needed; it surfaces automatically when installed.

