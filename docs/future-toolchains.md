# Future Toolchain & Language Support

This document lists compilers, assemblers, debuggers, and language extensions worth adding
templates for. Each entry includes what makes it interesting, what a template would require,
and any known technical challenges.

Freight's Rhai-driven template system means most of these require zero Rust changes — just a new
`.rhai` file in `toolchains/`. Items marked **[needs Rust]** require changes to `freight-core`.
See `docs/compiler-templates.md` for the full script API including `family` and `requires_toolchain`.

---

## C / C++ Compilers

### `zig cc` / `zig c++`
- **What**: Zig ships a bundled Clang that acts as a drop-in `cc`/`c++` replacement. Excellent
  for cross-compilation — a single `zig` binary can target any supported triple.
- **Template**: straightforward Clang-compatible flags; `target = "-target {triple}"` for cross.
- **Challenge**: `binary = "zig"` but invocations need `zig cc` / `zig c++` subcommands.
  `compile_binary` would be `"zig cc"` for C and `"zig c++"` for C++. The linker invocation also
  needs `zig c++` rather than `zig` alone. May require a multi-word `compile_binary` in the template.

### TCC (Tiny C Compiler)
- **What**: Extremely fast single-pass compiler. Good for rapid iteration and scripting-style
  C. Targets x86, x86-64, ARM.
- **Template**: `-DFOO` defines, `-I` includes, `-o`, `-c` are standard. No `-MMD` dep files —
  would need `dep_file = ""` and fall back to mtime-only dirty checking.
- **Challenge**: No C++ support, limited standard library. Primarily useful for C-only projects.

### MSVC (`cl.exe` / `clang-cl`)
- **What**: Microsoft's compiler, required for Windows SDK integration and COM/ATL/MFC. `clang-cl`
  is a Clang frontend with MSVC-compatible flags.
- **Template**: completely different flag scheme (`/O2`, `/W4`, `/WX`, `/Zi`, `/MT`, `/MD`, etc.).
  Include dirs use `/I{path}`, defines use `/D{name}`.
- **Challenge [needs Rust]**: object files are `.obj` not `.o`; output archive format is `.lib`
  not `.a`. Link command uses `link.exe` or `lld-link`. The build engine currently assumes Unix
  conventions throughout. Windows support would be the largest cross-cutting change.

### Intel oneAPI C++ (`icpx`) ✓ template exists
- Already present in `toolchains/intel/icpx.rhai`. May need updates as oneAPI releases progress.

### PGI / NVHPC (`nvc++`) ✓ template exists
- Already present in `toolchains/nvidia/nvhpc.rhai`. Family `"nvidia"`, handles C, C++, Fortran, CUDA.

### Circle (`circle`)
- **What**: An experimental C++20+ compiler with metaprogramming extensions (compile-time
  Python-like introspection). Drop-in Clang-compatible.
- **Template**: Clang-compatible flags; reuse most of `clang.rhai` with `binary = "circle"`.

### Emscripten (`emcc`)
- **What**: LLVM/Clang-based C/C++ to WebAssembly/JavaScript compiler. Required for building
  WASM modules from C/C++ source.
- **Template**: mostly GCC-compatible flags; output is `.wasm` + `.js`. `target` is implicit
  (always wasm32).
- **Challenge [needs Rust]**: output file extensions differ (`.wasm`); linking produces multiple
  files. The linker stage would need an extension hook.

### wasi-sdk
- **What**: WASI (WebAssembly System Interface) cross-compilation toolchain. Produces
  standalone WASM binaries that run in WASI runtimes (Wasmtime, WasmEdge).
- **Template**: Clang-based with `--target=wasm32-wasi`. Very similar to Emscripten template.

---

## Fortran Compilers

### Intel Fortran (`ifort` / `ifx`)
- **What**: Intel's classic (`ifort`, now deprecated) and next-gen LLVM-based (`ifx`) Fortran
  compilers. Widely used in HPC and scientific computing.
- **Template**: GFortran-compatible for most flags. `-standard-semantics` for strict conformance.
  `ifx` is the preferred target going forward.
- **Notes**: Often paired with MKL. `ifx` accepts `-std=f2018` like gfortran.

### Flang (LLVM Fortran)
- **What**: The LLVM project's Fortran frontend. New-flang (`flang-new` or `flang`) is the
  actively developed version aiming for full Fortran 2018.
- **Template**: GFortran-compatible flag set with some differences. Module files use `.mod`.
- **Challenge**: Still maturing; standard support varies significantly by release.

### NAG Fortran (`nagfor`)
- **What**: Numerical Algorithms Group compiler, the strictest Fortran standard checker available.
  Popular in academic environments for validation.
- **Template**: Different flag scheme (`-O2`, `-g`, `-I` are the same but standard flags differ).

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

### MASM (`ml` / `ml64`) — Windows only
- **What**: Microsoft Macro Assembler. Required for Windows kernel and driver development.
- **Template**: `/c`, `/Fo {path}`, `/I{path}` flag conventions.
- **Challenge**: Windows-only; depends on MSVC platform support.

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

### Intel DPC++ (`dpcpp` / `icpx -fsycl`)
- **What**: Intel's SYCL compiler for heterogeneous CPU/GPU/FPGA programming. Part of oneAPI.
- **Template**: Extend `icpx.rhai` with a `sycl` language key and `-fsycl` flag.

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

### Ada (`gnat`)
- **Planned**: a GNAT template is still needed, ideally with `gprbuild`-style multi-unit handling rather than a simple one-file compiler invocation.

### Objective-C / Objective-C++ (via Clang) ✓ template support exists
- **What**: Clang compiles `.m` and `.mm` files natively.
- **Template**: `clang.rhai` claims `.m` as `objc`; `clang++.rhai` claims `.mm` as `objcpp`.
  Platform frameworks such as `-framework Foundation` can be supplied through manifest linker flags or a build script.

### Swift (`swiftc`)
- **What**: Apple's Swift compiler. Produces object files linkable with C.
- **Template**: Different flag scheme (`-O` for release, `-g` for debug, `-I{path}` for includes).
  Module output is `.swiftmodule`.
- **Challenge [needs Rust]**: Swift uses its own module system incompatible with the C++20 module
  pipeline. Inter-module dependencies would need a dedicated scanner.

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

### `rr` (Mozilla Record & Replay)
- **What**: Records a program's execution and allows deterministic replay. Invaluable for
  hard-to-reproduce bugs. Linux only, x86-64.
- **Template addition**: `toolchains/debuggers/rr.rhai` with `binary = "rr"`,
  `separator = ""` (rr takes the program as first arg), no DAP support yet.
- **CLI**: `freight debug --debugger rr` would record; a separate `freight debug --replay` command
  could re-attach.

### WinDbg / CDB (Windows)
- **What**: Microsoft's debuggers for Windows user and kernel debugging.
- **Template addition**: `toolchains/debuggers/cdb.rhai` or `windbg.rhai`.
- **Challenge**: No standard DAP support; WinDbg Preview has some DAP support in preview.

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

---

## Package Manager Integration

### Conan
- **What**: C/C++ package manager. Can generate build system integrations (`cmake`, `msbuild`,
  `compiler_args`).
- **Integration**: `conan install .` produces `conanbuildinfo.txt` or a `generators/` directory.
  Freight could optionally read Conan-generated compiler/linker flags from `conanbuildinfo.args`.

### vcpkg
- **What**: Microsoft's C++ package manager for libraries. Installs to a `vcpkg_installed/`
  directory with standard include/lib layout.
- **Integration [needs Rust]**: Detect `vcpkg.json` manifest; read installed packages from
  `vcpkg_installed/<triplet>/include/` and `vcpkg_installed/<triplet>/lib/`. Could be a new
  dep kind: `mylib = { vcpkg = "mylib" }`.
