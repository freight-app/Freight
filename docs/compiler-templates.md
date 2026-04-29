# Compiler Templates

Crane's compiler system is fully data-driven. Every supported compiler or assembler is described
by a `.rhai` script in `toolchains/` — no Rust changes required. Adding a new compiler means writing
a new script and installing it with `crane toolchain add`.

---

## Loading order

Crane merges templates from two locations at startup:

1. **Bundled scripts** — shipped with the crane binary in `toolchains/`
2. **User scripts** — installed in `~/.crane/templates/` via `crane toolchain add <path>`

User scripts with the same `name` as a bundled script take precedence (override).

---

## Script structure

A compiler script is a Rhai file that calls registered API functions to declare the toolchain.
Here is a fully annotated example:

```rhai
// toolchains/gcc.rhai

set_name("gcc");
set_binary("g++");            // binary used for linking (and default compilation)
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

// Role overrides — for external tools like archivers or strippers
set_toolset("cc",  "gcc");    // C compilation uses gcc, not g++
set_toolset("ar",  "ar");

set_extensions([".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"]);

// Flag table — abstract setting → concrete compiler flag
set_flag("opt",      "0", "-O0");
set_flag("opt",      "1", "-O1");
set_flag("opt",      "2", "-O2");
set_flag("opt",      "3", "-O3");
set_flag("debug",    "true",  "-g");
set_flag("debug",    "false", "");
set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-Wall");
set_flag("warnings", "all",     "-Wall -Wextra -Wpedantic");
set_flag("warnings", "error",   "-Wall -Wextra -Wpedantic -Werror");
set_flag("lto",      "true",  "-flto");
set_flag("lto",      "false", "");
set_flag("strip",    "true",  "-s");
set_flag("strip",    "false", "");
set_flag("sanitize", "template", "-fsanitize={values}"); // {values} = comma-joined list
set_flag("cpu_ext",  "template", "-m{name}");            // e.g. avx2 → -mavx2

// Language standards — value the user writes in [language.<key>].std
set_standard("c11",   "-std=c11");
set_standard("c17",   "-std=c17");
set_standard("c23",   "-std=c23");
set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");
set_standard("c++23", "-std=c++23");

// Structure templates — {path}, {name}, {value}, {triple} substituted at build time
set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "-MMD -MF {path}");
set_structure("target",       "");            // empty = GCC cross-compiles via dedicated binary
set_structure("sysroot",      "--sysroot={path}");

// Arch-specific flags — "arch.os" key wins over "arch" alone
// set_arch_flag("x86_64.linux", "");        // not needed for GCC

// C++20 module support
set_module_style("gcc", #{
    enable_flag:   "-fmodules-ts",
    compile_miu:   "-fmodule-output={pcm_path}",   // one-step: produces .o + .pcm
    import_module: "-fmodule-file={name}={pcm_path}",
});

// Language linking keys — one per language this template handles
set_linking("c", #{
    abi:            "c",
    compile_binary: "gcc",          // compile C files with gcc, not g++
    compatible:     ["fortran", "asm"],
    extensions:     [".c", ".s", ".S"],
});
set_linking("cpp", #{
    abi:        "c++",
    compatible: ["c", "fortran", "asm"],
    extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"],
});

// check() — return false to hide this toolchain when unavailable
fn check() {
    find_tool("g++") != ()
}

// load() — called at detection time; arch and os variables are in scope
fn load() {
    if arch == "x86_64" { add_flags("cxx", "-m64"); }
    if arch == "x86"    { add_flags("cxx", "-m32"); }
}
```

### Two-step Clang module strategy

Clang requires a separate `--precompile` step before compilation. Use `precompile` instead of
`compile_miu`:

```rhai
set_module_style("clang", #{
    precompile:    "--precompile",             // step 1: src → .pcm (no object)
    import_module: "-fmodule-file={name}={pcm_path}",
});
```

---

## Rhai API reference

| Function | Purpose |
|----------|---------|
| `set_name(s)` | Toolchain identifier (used in `backend = "..."` in crane.toml) |
| `set_binary(s)` | Primary binary (detection + linker fallback) |
| `set_toolset(role, binary)` | Role overrides: `"cc"`, `"cxx"`, `"ld"`, `"ar"`, `"strip"`, `"as"` |
| `set_extensions(list)` | File extensions this template claims during source discovery |
| `set_flag(cat, key, val)` | Flag map entry; categories: `opt`, `debug`, `warnings`, `lto`, `strip`, `sanitize`, `cpu_ext` |
| `set_standard(key, flag)` | Language standard mapping |
| `set_structure(key, tmpl)` | Structure templates: `include_dir`, `define`, `define_value`, `output`, `compile_only`, `dep_file`, `target`, `sysroot` |
| `set_arch_flag(key, flag)` | Arch/OS-specific flags: `"x86_64.linux"`, `"x86_64"`, etc. |
| `set_module_style(style, params)` | `"gcc"` or `"clang"` with param map |
| `set_linking(lang, params)` | Declares ABI, compatible ABIs, extensions, optional `compile_binary` |
| `set_passthrough(bool, prefix)` | nvcc-style `-Xcompiler` wrapping |
| `add_always_flag(flag)` | Unconditional flag appended to every invocation |
| `set_supported_archs(list)` | Hide toolchain on unlisted host archs (`"x86_64"`, `"aarch64"`, …); empty = no restriction |
| `set_supported_os(list)` | Hide toolchain on unlisted host OSes (`"linux"`, `"windows"`, `"macos"`, …); empty = no restriction |
| `fn check()` | Return `false` to hide toolchain when binary not found or arch unsupported |
| `fn load()` | Called at detection time; `arch` and `os` variables in scope; can call `add_flags()` |
| `find_tool(name)` | Search `$PATH`; returns path string or `()` |
| `arch()` | Returns host CPU arch string (same as `std::env::consts::ARCH`) |
| `os()` | Returns host OS string (same as `std::env::consts::OS`) |
| `env(key)` | Read environment variable; returns string or `()` |

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
vscode_type  = "lldb"     # "type" field in launch.json config (CodeLLDB)
mi_mode      = "lldb"     # "MIMode" field (only relevant for cppdbg type)
```

`crane debug` uses debugger templates to invoke an interactive session and to generate
`.vscode/launch.json` entries via `crane debug --launch-json`.

---

## Writing a new compiler script

### Checklist

1. Create `toolchains/<name>.rhai`
2. Call `set_name`, `set_binary`, `set_version_arg`, `set_version_regex`
3. Call `set_extensions` with the file extensions this compiler claims
4. Add `set_flag` entries for `opt`, `debug`, `warnings` at minimum
5. Add `set_standard` for every standard string users might write
6. Call `set_structure` — at minimum `include_dir`, `define`, `output`, `compile_only`
7. Add `dep_file` structure if the compiler supports Makefile dep files (`-MMD -MF`)
8. Call `set_linking` for every language the template handles
9. Add `fn check()` to hide the toolchain when the binary is absent
10. Test with `crane toolchain list` to verify detection, then `crane build` on a real project

### Installing

```sh
crane toolchain add path/to/mycompiler.rhai
```

The script is validated and copied to `~/.crane/templates/mycompiler.rhai`.
The new template is loaded on the next crane invocation.

---

## Key design decisions

### One script per toolchain, not per language

`gcc.rhai` handles both C and C++. The `compile_binary` override in `set_linking("c", ...)`
selects `gcc` for C compilation while `g++` is still used for linking (required for mixed
C/C++ projects).

### Arch-specific flags

`set_arch_flag("x86_64.linux", "-f elf64")` is how NASM selects its output format. Crane
looks up `"arch.os"` first, then `"arch"` as a fallback. GCC and Clang don't need this —
they infer the output format from the target triple.

### `fn load()` for arch-conditional flags

`fn load()` runs at detection time with `arch` and `os` in scope. Use it for flags that
depend on the host environment:

```rhai
fn load() {
    if arch == "x86_64" { add_flags("cxx", "-m64"); }
}
```

### `fn check()` for availability detection

Return `false` to hide the toolchain when its binary isn't present or the platform is
unsupported. The script body (outside `fn check`) is not the right place for this — it
runs before detection.

```rhai
fn check() {
    find_tool("nvcc") != ()
}
```
