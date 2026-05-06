# Compiler Templates

Freight's compiler system is fully data-driven. Every supported compiler or assembler is described
by a `.rhai` script in `toolchains/` — no Rust changes required to add a new compiler. Adding a
new compiler means writing a script and installing it with `freight toolchain add`.

---

## Loading order

Freight merges templates from two locations at startup:

1. **Bundled scripts** — shipped with the freight binary in `toolchains/`
2. **User scripts** — installed in `~/.freight/templates/` via `freight toolchain add <path>`

User scripts with the same `name` as a bundled script take precedence (override).

---

## Script structure

Templates use direct assignment for scalar fields, map subscript syntax for flag tables, and
standard Rhai `fn` blocks for the detection and load hooks. Here is a fully annotated example:

```rhai
// toolchains/gnu/gcc.rhai

// Optional: Rhai code to compute binary names at load time.
let _gxx = "g++";
for _b in ["g++", "g++-14", "g++-13", "g++-12"] {
    if find_tool(_b) != () { _gxx = _b; break; }
}
let _gcc = if _gxx == "g++" { "gcc" } else { "gcc" + _gxx.sub_string(3) };

// ── Identity ──────────────────────────────────────────────────────────────────

name          = "gcc";          // template identifier; used in `backend = "gcc"` for standalones
family        = "gnu";          // family group; `freight toolchain use gnu` selects all "gnu" compilers
                                // leave empty ("") for standalone compilers (tcc, msvc)
homepage      = "https://gcc.gnu.org/";
binary        = _gxx;           // primary binary; probed with version_arg for detection
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";   // capture group 1 is the version string

// ── Guest / extension ─────────────────────────────────────────────────────────

// requires_toolchain = ["cpp"];   // uncomment for wrappers like nvcc, hipcc
                                   // requires_toolchain = ["c"] for assemblers like nasm, yasm
                                   // when non-empty: compiler is a guest — it extends the active
                                   // toolchain and cannot be selected via `freight toolchain use`

// ── Discovery ─────────────────────────────────────────────────────────────────

extensions    = [".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"];
sanitizers    = ["address", "undefined", "thread", "leak"];   // sanitize values this compiler supports
passthrough        = false;    // true for nvcc-style -Xcompiler wrappers
passthrough_prefix = "";       // e.g. "-Xcompiler" for nvcc

// ── Toolset roles ─────────────────────────────────────────────────────────────

toolset["cc"]    = _gcc;       // C compilation binary
toolset["cxx"]   = _gxx;       // C++ compilation binary
toolset["ld"]    = _gxx;       // linker binary
toolset["ar"]    = "ar";       // static archive creator
toolset["strip"] = "strip";    // strip binary
// toolset["as"] = "nasm";     // assembler override (usually not needed)

// ── Compiler flags ────────────────────────────────────────────────────────────

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Oz";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall -Wextra -Wpedantic";
flags["warnings"]["error"]   = "-Wall -Wextra -Wpedantic -Werror";

flags["lto"]["true"]  = "-flto";
flags["lto"]["false"] = "";

flags["sanitize"]["template"] = "-fsanitize={values}";  // {values} = comma-joined list
flags["cpu_ext"]["template"]  = "-m{name}";              // e.g. avx2 → -mavx2

// ── Language standards ────────────────────────────────────────────────────────

standards["c11"]   = "-std=c11";
standards["c17"]   = "-std=c17";
standards["c23"]   = "-std=c23";
standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

// ── Structure templates ───────────────────────────────────────────────────────
// {path}, {name}, {value}, {triple} are substituted at build time.

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";      // used when output_obj / output_bin are not set
compile_only = "-c";
dep_file     = "-MMD -MF {path}";  // empty string = no dep file (mtime-only dirty check)
target       = "";               // empty = GCC cross-compiles via dedicated binary (e.g. aarch64-linux-gnu-g++)
sysroot      = "--sysroot={path}";

// ── Arch-specific flags ───────────────────────────────────────────────────────
// "arch.os" key is checked first; "arch" is the fallback.

// arch_flags["x86_64.linux"]   = "-f elf64";   // NASM-style output format selection
// arch_flags["x86_64.macos"]   = "-f macho64";

// ── C++20 module support ──────────────────────────────────────────────────────

modules["style"]         = "gcc";                         // "gcc", "clang", or "none"
modules["compile_miu"]   = "-fmodule-output={pcm_path}"; // GCC one-step: .o + .pcm together
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-fmodule-header";

// Clang two-step alternative:
// modules["style"]         = "clang";
// modules["precompile"]    = "--precompile";   // step 1: src → .pcm (no object)
// modules["import_module"] = "-fmodule-file={name}={pcm_path}";

// ── Linking metadata ──────────────────────────────────────────────────────────
// One entry per language key this template handles.

linking["c"] = #{
    abi:            "c",              // ABI family for compatibility checks
    compatible:     ["fortran", "asm"],  // can link objects with these ABIs
    compile_binary: _gcc,             // use gcc not g++ for C files
    linker:         "",               // empty = use toolset["ld"]
    extensions:     [".c", ".s", ".S"],
};

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"],
};

// ── Detection hook ────────────────────────────────────────────────────────────

fn check() {
    // Return false to hide this toolchain when unavailable.
    let bins = ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"];
    for b in bins {
        if find_tool(b) != () { return true; }
    }
    false
}

// ── Load hook ─────────────────────────────────────────────────────────────────

fn load() {
    // Called at detection time. `arch` and `os` are in scope.
    // Append flags using load_flags["role"] += ["flag"].
    if arch == "x86_64" {
        load_flags["cc"]  += ["-m64"];
        load_flags["cxx"] += ["-m64"];
        load_flags["ld"]  += ["-m64"];
    } else if arch == "x86" {
        load_flags["cc"]  += ["-m32"];
        load_flags["cxx"] += ["-m32"];
        load_flags["ld"]  += ["-m32"];
    }
}
```

---

## Field reference

### Identity and discovery

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Template identifier. Used in `backend = "..."` for standalone compilers. |
| `family` | string | Family group (`"gnu"`, `"llvm"`, `"intel"`, `"nvidia"`, …). Compilers that share a family are shown together in `freight toolchain list` and selected as a unit by `freight toolchain use <family>`. Leave empty for standalone compilers (`"tcc"`, `"msvc"`). |
| `requires_toolchain` | `[string]` | Language keys that must be provided by another detected compiler. Non-empty marks a **guest/extension**: it extends the active toolchain but cannot be chosen via `freight toolchain use`. Use `["cpp"]` for wrappers (nvcc, hipcc, ispc), `["c"]` for assemblers (nasm, yasm). Guests are silently dropped when no host compiler satisfying the requirement is detected. |
| `homepage` | string | Informational URL shown in docs. |
| `binary` | string | Binary probed to detect this toolchain. |
| `version_arg` | string | Argument passed to `binary` to print its version. Empty string = invoke with no arguments (MSVC). |
| `version_regex` | string | Regex with one capture group extracting the version string from the output. |
| `extensions` | `[string]` | File extensions this template claims during source discovery. |
| `sanitizers` | `[string]` | Sanitize values this compiler supports (for validation). |
| `passthrough` | bool | `true` for nvcc-style `-Xcompiler` wrappers. |
| `passthrough_prefix` | string | The wrapper prefix, e.g. `"-Xcompiler"`. |
| `supported_archs` | `[string]` | If non-empty, hide this toolchain on unlisted host architectures. |
| `supported_os` | `[string]` | If non-empty, hide this toolchain on unlisted host OSes. |

### Toolset roles

```rhai
toolset["cc"]    = "gcc";      // C compilation
toolset["cxx"]   = "g++";      // C++ compilation
toolset["ld"]    = "g++";      // final link
toolset["ar"]    = "ar";       // static archive
toolset["strip"] = "strip";    // strip debug symbols
toolset["as"]    = "nasm";     // assembler override
```

### Flag maps

```rhai
flags["opt"]["0"] = "-O0";          // optimization levels 0-3, s, z
flags["debug"]["true"] = "-g";      // debug on/off
flags["warnings"]["all"] = "...";   // none | default | all | error
flags["lto"]["true"] = "-flto";     // link-time optimization
flags["lto_link"]["true"] = "/LTCG"; // link-step LTO override (MSVC)
flags["sanitize"]["template"] = "-fsanitize={values}";  // {values} = comma-joined list
flags["cpu_ext"]["template"] = "-m{name}";              // e.g. avx2 → -mavx2
```

### Structure templates

| Field | Description |
|-------|-------------|
| `include_dir` | Include path flag. `{path}` is substituted. |
| `define` | Define flag. `{name}` is substituted. |
| `define_value` | Define-with-value flag. `{name}` and `{value}` are substituted. |
| `output` | Output path flag. `{path}` is substituted. Fallback when `output_obj`/`output_bin` are absent. |
| `output_obj` | Output flag for object files (`-o {path}` for GCC, `/Fo{path}` for MSVC). |
| `output_bin` | Output flag for binaries (`-o {path}` for GCC, `/Fe{path}` for MSVC). |
| `compile_only` | Flag to compile without linking (usually `-c`). |
| `dep_file` | Dep file generation flag. `{path}` substituted. Empty = no dep files (mtime-only). |
| `dep_file_mode` | `"file"` (default) or `"stdout"` (MSVC `/showIncludes`) or `"none"`. |
| `target` | Cross-compilation target flag. `{triple}` substituted. Empty = unsupported or uses dedicated binary. |
| `sysroot` | Sysroot flag. `{path}` substituted. |
| `system_lib` | System library flag. `{name}` substituted. Default: `"-l{name}"`. MSVC uses `"{name}.lib"`. |

### C++20 modules

```rhai
modules["style"]         = "gcc";                          // "gcc", "clang", or "none"
// GCC one-step (produces .o + .pcm in a single invocation):
modules["compile_miu"]   = "-fmodule-output={pcm_path}";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-fmodule-header";
// Clang two-step (.pcm first, then .o):
modules["precompile"]    = "--precompile";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-x c++-header";
```

### Linking metadata

```rhai
linking["cpp"] = #{
    abi:            "c++",          // ABI family (used in compatibility checks)
    compatible:     ["c", "fortran"],  // can link objects with these ABIs
    compile_binary: "gcc",          // override which binary compiles files with this key (optional)
    linker:         "",             // linker binary override; empty = use toolset["ld"]
    extensions:     [".cpp", ".cc"],   // file extensions routed to this language key
};
```

### Hooks

```rhai
fn check() {
    // Return false to hide when the toolchain is unavailable.
    find_tool("g++") != ()
}

fn load() {
    // Called at detection time. Variables in scope: arch, os.
    // Append runtime flags via load_flags["role"] += ["flag"].
    if arch == "x86_64" { load_flags["cxx"] += ["-m64"]; }
}
```

### Utility functions

| Function | Returns | Description |
|----------|---------|-------------|
| `find_tool(name)` | `string \| ()` | Search `$PATH`; return full path or `()` |
| `arch` | string | Host CPU arch (`"x86_64"`, `"aarch64"`, …) — `std::env::consts::ARCH` |
| `os` | string | Host OS (`"linux"`, `"windows"`, `"macos"`, …) — `std::env::consts::OS` |
| `env(key)` | `string \| ()` | Read environment variable; `()` if not set |

---

## Debugger templates

Debugger templates live in `toolchains/debuggers/` as TOML files (separate from compiler scripts).

```toml
# toolchains/debuggers/lldb.toml

name          = "lldb"
binary        = "lldb"
version_arg   = "--version"
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b"

[launch]
separator = "--"    # arguments after this separator are passed to the debuggee
                    # gdb uses "--args" instead

[dap]
# Binaries searched in order; first found becomes the DAP adapter path
binaries     = ["lldb-dap", "lldb-vscode"]
vscode_type  = "lldb"
mi_mode      = "lldb"
```

---

## Writing a new compiler script

### Checklist

1. Create `toolchains/<name>.rhai` (or `toolchains/<family>/<name>.rhai`)
2. Set `name`, `binary`, `version_arg`, `version_regex`
3. Set `family` if the compiler belongs to a suite (`"gnu"`, `"llvm"`, …); leave empty for standalone tools
4. Set `requires_toolchain` if the compiler wraps or extends another toolchain (e.g. `["cpp"]` for nvcc, `["c"]` for nasm); leave empty for self-contained compilers
5. Set `extensions` with the file extensions this compiler claims
6. Add `flags` entries for `opt`, `debug`, `warnings` at minimum
7. Add `standards` for every standard string users might write
8. Set structure fields — at minimum `include_dir`, `define`, `output`, `compile_only`
9. Add `dep_file` if the compiler supports Makefile dep files (`-MMD -MF`)
10. Add `linking[...]` entries for every language key the template handles
11. Add `fn check()` to hide the toolchain when the binary is absent
12. Test with `freight toolchain list` to verify detection, then `freight build` on a real project

### Installing

```sh
freight toolchain add path/to/mycompiler.rhai
```

The script is validated and copied to `~/.freight/templates/mycompiler.rhai`.
The new template is loaded on the next freight invocation.

---

## Key design decisions

### Family grouping

`freight toolchain list` groups compilers by `family`. Set the same `family` value in
`gcc.rhai` and `gfortran.rhai` and they appear together as the `gnu` toolchain. The
`family` value is what the user passes to `freight toolchain use`:

```sh
freight toolchain use gnu    # selects all compilers with family = "gnu"
freight toolchain use llvm   # selects all compilers with family = "llvm"
freight toolchain use msvc   # standalone compiler (family = ""), selected by name
```

### Guest/extension compilers

Compilers with `requires_toolchain` non-empty are **guests** — they extend the active
toolchain and are auto-selected when the active family satisfies their requirements:

- `nvcc` (`requires_toolchain = ["cpp"]`) — CUDA extension for any C++ toolchain
- `hipcc` (`requires_toolchain = ["cpp"]`) — HIP/ROCm extension
- `nasm`, `yasm` (`requires_toolchain = ["c"]`) — assembly; active whenever any C toolchain is present

Guests appear in the "Extensions" section of `freight toolchain list`. If a required
language key is not provided by any detected compiler, the guest is silently dropped with
a warning.

### One script per toolchain, not per language

`gcc.rhai` handles both C and C++. The `compile_binary` field in `linking["c"]` selects
`gcc` for C files while `g++` is used for linking (required for mixed C/C++ projects).

### Arch-specific flags

`arch_flags["x86_64.linux"] = "-f elf64"` is how NASM selects its output format. Freight
looks up `"arch.os"` first, then `"arch"` as a fallback.

### `fn load()` for arch-conditional flags

`fn load()` runs at detection time with `arch` and `os` in scope. Append flags via
`load_flags["role"] += ["flag"]`:

```rhai
fn load() {
    if arch == "x86_64" { load_flags["cxx"] += ["-m64"]; }
}
```

### `fn check()` for availability detection

Return `false` to hide the toolchain when its binary isn't present or the platform is
unsupported:

```rhai
fn check() {
    find_tool("nvc++") != ()
}
```
