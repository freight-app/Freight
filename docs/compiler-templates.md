# Compiler Templates

Freight's compiler system is fully data-driven. Every supported compiler, assembler, or debugger
is described by a `.rhai` script in `toolchains/` — no Rust changes required to add a new one.
Adding a new compiler means writing a script and installing it with `freight toolchain add`.

---

## Directory layout

Templates are organised into family subdirectories. Files prefixed with `_` are shared includes,
not standalone templates.

```
toolchains/
├── gnu/
│   ├── _gnu-base.rhai       # shared flags/toolset for all GNU compilers
│   ├── g++.rhai
│   ├── gcc.rhai
│   ├── gfortran.rhai
│   ├── gdc.rhai             # D (GCC frontend)
│   └── gdb.rhai             # kind = "debugger"
├── llvm/
│   ├── _llvm-base.rhai
│   ├── clang++.rhai
│   ├── clang.rhai
│   ├── flang.rhai
│   ├── ldc2.rhai            # D (LLVM frontend)
│   ├── lldb.rhai            # kind = "debugger"
│   ├── clang-format.rhai    # kind = "formatter"
│   └── clang-tidy.rhai      # kind = "linter"
├── nvidia/
│   ├── _nvhpc-base.rhai
│   ├── nvc++.rhai
│   ├── nvc.rhai
│   ├── nvfortran.rhai
│   └── nvcc.rhai            # requires_toolchain = ["cpp"]
├── intel/
│   ├── _intel-base.rhai
│   ├── icpx.rhai
│   ├── ifx.rhai
│   └── ispc.rhai            # requires_toolchain = ["cpp"]
├── amd/
│   └── hipcc.rhai           # requires_toolchain = ["cpp"]
├── asm/
│   ├── _asm-base.rhai
│   ├── gas.rhai             # requires_toolchain = ["c"]
│   ├── nasm.rhai
│   └── yasm.rhai
├── languages/
│   ├── _cpp.rhai            # extensions, defaults, standards, linking for C++
│   ├── _c.rhai              # extensions, defaults, standards for C
│   └── _fortran.rhai        # extensions, defaults, standards, linking for Fortran
├── astyle/
│   └── astyle.rhai          # kind = "formatter"
├── uncrustify/
│   └── uncrustify.rhai      # kind = "formatter"
├── fprettify/
│   └── fprettify.rhai       # kind = "formatter"  (Fortran)
├── cppcheck/
│   └── cppcheck.rhai        # kind = "linter"
├── cpplint/
│   └── cpplint.rhai         # kind = "linter"
├── flawfinder/
│   └── flawfinder.rhai      # kind = "linter"
├── dmd.rhai                 # D reference compiler
├── msvc.rhai
├── tcc.rhai
├── opencl.rhai              # requires_toolchain = ["cpp"]
└── system-libs/             # TOML stubs for OS libraries used by dep resolution
    ├── pthread.toml
    ├── ws2_32.toml
    └── ...
```

---

## Loading order

Freight merges templates from two locations at startup:

1. **Bundled scripts** — shipped with the freight binary in `toolchains/`
2. **User scripts** — installed in `~/.freight/templates/` via `freight toolchain add <path>`

User scripts with the same `name` as a bundled script take precedence. Files starting with `_`
are shared includes — they are never loaded as standalone templates. The bundled directory is
found via `FREIGHT_TEMPLATES_DIR`, `<binary-dir>/toolchains/`, or the cargo-development layout.

Compiler templates that do **not** register runtime option callbacks are cached in
`~/.freight/template-cache.msgpack`. The cache key includes the template contents and directly
included base files. Templates that call `compiler_option` or `language_option` are evaluated live
so their Rhai callback pointers remain available during builds.

A fast `kind` pre-check reads the first `kind = "..."` line of each file before doing a full
Rhai evaluation, routing each template to the correct loader:

| `kind` | Loaded by | Used by |
|---|---|---|
| `"compiler"` (default) | `load_templates()` | `freight build`, `freight toolchain list` |
| `"debugger"` | `load_debugger_templates()` | `freight debug` |
| `"formatter"` | `load_formatter_templates()` | `freight fmt` |
| `"linter"` | `load_linter_templates()` | `freight lint` |

---

## Shared base files and `include`

Common code is factored into `_base` files and included with the `include` keyword:

```rhai
// gnu/g++.rhai
include "_gnu-base";           // resolves relative to this file's directory
include "../languages/_cpp";   // language-shared extensions, standards, linking
```

`include` is evaluated inline — variables and assignments in the included file take effect in
the calling file's scope. Any field can be overridden after an include.

---

## Language shared files

`toolchains/languages/` holds files that are identical across all compilers of the same language:
extensions, default standards, and linking metadata. Both `gnu/g++.rhai` and `llvm/clang++.rhai`
include `../languages/_cpp`, so C++ parameters only need to be maintained in one place.

```rhai
// languages/_cpp.rhai
extensions = [".cpp", ".cppm", ".cc", ".cxx", ".c++"];

defaults["std"] = "c++17";   // applied when [language.cpp] std is not set in freight.toml

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"],
};
```

---

## Compiler script structure

Here is a fully annotated compiler template based on the GNU family:

```rhai
// gnu/g++.rhai

include "_gnu-base";         // shared flags, toolset defaults, arch flags
include "../languages/_cpp"; // extensions, standards, defaults, linking

// ── Identity ──────────────────────────────────────────────────────────────────

name     = "g++";
kind     = "compiler";   // "compiler" (default) or "debugger"
family   = "gnu";        // family group; leave "" for standalone compilers (tcc, msvc)
homepage = "https://gcc.gnu.org/";
binary   = _gxx;         // primary binary; probed with version_arg during detection
                         // (_gxx is resolved in _gnu-base via find_tool())

// ── Guest / extension ─────────────────────────────────────────────────────────
// Uncomment when this compiler wraps another toolchain (nvcc, hipcc, nasm…).

// requires_toolchain = ["cpp"];  // marks this as a guest — cannot be selected via
                                  // `freight toolchain use`; auto-active when a
                                  // C++ toolchain is detected

// ── Detection ─────────────────────────────────────────────────────────────────

version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";  // capture group 1 = version string
sanitizer_options = ["address", "undefined", "thread", "leak"];
passthrough        = false;   // true for nvcc-style -Xcompiler wrappers
passthrough_prefix = "";      // e.g. "-Xcompiler" for nvcc

// supported_archs = ["x86_64", "aarch64"];  // hide on unlisted host architectures
// supported_os    = ["linux", "windows"];   // hide on unlisted host OSes
// required_tools  = ["ptxas", "fatbinary"]; // extra binaries that must be on PATH
// required_env    = ["ONEAPI_ROOT"];       // SDK environment variables that must be set
// min_version     = "12.0";                // hide if detected version is older

// ── Toolset roles ─────────────────────────────────────────────────────────────

toolset["cc"]    = _gcc;    // C compilation binary
toolset["cxx"]   = _gxx;   // C++ compilation binary
toolset["ld"]    = _gxx;   // linker binary
toolset["ar"]    = "ar";   // static archive creator
toolset["strip"] = "strip";

// ── Flag maps ─────────────────────────────────────────────────────────────────

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-Os";
opt["z"] = "-Oz";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra -Wpedantic";
warnings["error"]   = "-Wall -Wextra -Wpedantic -Werror";

lto["true"]  = "-flto";
lto["false"] = "";
// lto_link is optional and used when the link-step spelling differs (MSVC /LTCG).
// lto_link["true"]  = "/LTCG";
// lto_link["false"] = "";

sanitize = "-fsanitize={values}";  // {values} = comma-joined sanitizer list
cpu_ext  = "-m{name}";             // e.g. avx2 → -mavx2

// ── Runtime / stdlib ──────────────────────────────────────────────────────────

stdlib["libstdc++"] = "";
stdlib["none"]      = "-nostdlib";

runtime["glibc"]  = "";
runtime["musl"]   = "-static";
runtime["none"]   = "-nostdlib -nodefaultlibs";

// ── Defaults ──────────────────────────────────────────────────────────────────
// Applied when the corresponding [language.*] key is absent in freight.toml.
// Usually declared in the language shared file (languages/_cpp.rhai).

// defaults["std"] = "c++17";  // already set by ../languages/_cpp

// ── Structure templates ───────────────────────────────────────────────────────

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";  // empty = no dep files (mtime-only dirty check)
target       = "";                  // empty = this template does not emit a target flag
sysroot      = "--sysroot={path}";
// output_obj   = "/Fo{path}";          // optional compile-step output override
// output_bin   = "/Fe{path}";          // optional link-step output override
// dep_file_mode = "file";              // "file" (default), "stdout", or "none"
// system_lib   = "-l{name}";           // default; MSVC uses "{name}.lib"

// ── Arch-specific flags ───────────────────────────────────────────────────────
// "arch.os" is checked first; "arch" is the fallback.

// arch_flags["x86_64.linux"]   = "-f elf64";   // NASM-style output format selection
// arch_flags["x86_64.macos"]   = "-f macho64";

// ── C++20 module support ──────────────────────────────────────────────────────

modules["style"]         = "gcc";                          // "gcc", "clang", or "none"
modules["enable_flag"]   = "-fmodules-ts";                 // automatically added for GCC-style modules
modules["compile_miu"]   = "-fmodule-output={pcm_path}";  // GCC one-step: .o + .pcm
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-fmodule-header";

// Clang two-step alternative:
// modules["style"]      = "clang";
// modules["precompile"] = "--precompile";  // step 1: src → .pcm
// modules["import_module"] = "-fmodule-file={name}={pcm_path}";

// ── PCH support ───────────────────────────────────────────────────────────────

pch["compile"]   = "-x c++-header";
pch["use"]       = "-include {header_path}";
pch["extension"] = ".gch";

// ── Detection hook ────────────────────────────────────────────────────────────

fn check() {
    for b in ["g++", "g++-14", "g++-13", "g++-12"] {
        if find_tool(b) != () { return true; }
    }
    false
}

// ── Arch-conditional flags ────────────────────────────────────────────────────
// load_flags are evaluated at template load time and folded into default flags.

if arch == "x86_64" {
    load_flags["cc"]  += ["-m64"];
    load_flags["cxx"] += ["-m64"];
    load_flags["ld"]  += ["-m64"];
}
```

---

## Field reference

### Identity and discovery

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Template identifier. Used in `backend = "..."` for standalone compilers. Named after the actual binary (e.g. `"g++"`, `"clang++"`). |
| `kind` | string | `"compiler"` (default), `"debugger"`, `"formatter"`, or `"linter"`. Determines which loader handles this file. |
| `family` | string | Family group (`"gnu"`, `"llvm"`, `"intel"`, `"nvidia"`, …). Compilers sharing a family are shown together in `freight toolchain list` and selected as a unit by `freight toolchain use <family>`. Leave empty for standalone compilers (`"tcc"`, `"msvc"`). |
| `requires_toolchain` | `[string]` | Language keys that must be provided by another detected compiler. Non-empty marks a **guest**: it extends the active toolchain but cannot be chosen via `freight toolchain use`. Use `["cpp"]` for wrappers (nvcc, hipcc, ispc), `["c"]` for assemblers (nasm, yasm). Guests are silently dropped when no host satisfying the requirement is detected. |
| `homepage` | string | Informational URL shown in docs. |
| `binary` | string | Binary probed to detect this toolchain. |
| `version_arg` | string | Argument passed to `binary` to print its version. Empty string = invoke with no arguments (MSVC). |
| `version_regex` | string | Regex with one capture group extracting the version string from the output. |
| `extensions` | `[string]` | File extensions this template claims during source discovery. Usually declared in the shared language file. |
| `sanitizer_options` | `[string]` | Sanitize values this compiler supports (for validation). |
| `passthrough` | bool | `true` for nvcc-style `-Xcompiler` wrappers. |
| `passthrough_prefix` | string | The wrapper prefix, e.g. `"-Xcompiler"`. |
| `supported_archs` | `[string]` | If non-empty, hide this toolchain on unlisted host architectures. |
| `supported_os` | `[string]` | If non-empty, hide this toolchain on unlisted host OSes. |
| `required_tools` | `[string]` | Extra binaries that must be on PATH for this toolchain to be considered available. |
| `required_env` | `[string]` | Environment variables that must be set for this toolchain to be considered available (useful for SDK setup scripts). |
| `min_version` | string | Minimum detected version required during discovery. Older binaries are skipped with a warning. |
| `always_flags` | `[string]` | Flags always prepended to every compiler invocation. |

### Toolset roles

```rhai
toolset["cc"]    = "gcc";      // C compilation
toolset["cxx"]   = "g++";     // C++ compilation
toolset["ld"]    = "g++";     // final link
toolset["ar"]    = "ar";      // static archive
toolset["strip"] = "strip";   // strip debug symbols
```

### Flag maps

```rhai
opt["0"] = "-O0";           // optimization levels: 0 1 2 3 s z
opt["s"] = "-Os";

dbg["true"]  = "-g";       // debug on/off
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra -Wpedantic";
warnings["error"]   = "-Wall -Wextra -Wpedantic -Werror";

lto["true"]  = "-flto";    // compile-step LTO on/off
lto["false"] = "";

// Optional link-step LTO map when the linker uses different spelling.
lto_link["true"]  = "/LTCG";
lto_link["false"] = "";

sanitize = "-fsanitize={values}";   // {values} = comma-joined sanitizer list
cpu_ext  = "-m{name}";              // e.g. avx2 → -mavx2

stdlib["libc++"]    = "-stdlib=libc++";  // selected by [language.cpp] stdlib
stdlib["libstdc++"] = "";
runtime["musl"]     = "-static";         // selected by [compiler] runtime
```

### Defaults map

`defaults["key"] = "value"` provides fallback values applied when the corresponding
`[language.*]` setting is absent from `freight.toml`. Typically declared in the shared
language file so it applies consistently across all compilers of that language:

```rhai
defaults["std"] = "c++17";   // used when [language.cpp] omits std =
```

`defaults["stdlib"]` works the same way for C++ standard-library selection when a template
defines the `stdlib` flag map. The manifest value still wins when present.

### Runtime, stdlib, and link-step flags

Use `stdlib[...]` for language-level C++ standard library selection and `runtime[...]` for
compiler/runtime choices such as glibc vs musl. These maps are converted into flags by the build
settings layer; unknown keys simply produce no flag.

```rhai
defaults["stdlib"] = "libstdc++";
stdlib["libstdc++"] = "";
stdlib["libc++"]    = "-stdlib=libc++";
stdlib["none"]      = "-nostdlib++";

runtime["glibc"] = "";
runtime["musl"]  = "-static";
runtime["none"]  = "-nostdlib -nodefaultlibs";
```

`lto[...]` is emitted during compilation. If a compiler needs a different flag during final
linking, also set `lto_link[...]`; otherwise Freight reuses the compile-step LTO behaviour.

```rhai
lto["true"]      = "/GL";
lto["false"]     = "";
lto_link["true"] = "/LTCG";
```

### Structure templates

| Field | Description |
|-------|-------------|
| `include_dir` | Include path flag. `{path}` is substituted. |
| `define` | Define flag. `{name}` is substituted. |
| `define_value` | Define-with-value flag. `{name}` and `{value}` are substituted. |
| `output` | Output path flag. `{path}` is substituted. Used for both compile and link unless overridden. |
| `output_obj` | Optional compile-step output override. Falls back to `output`. MSVC uses `/Fo{path}`. |
| `output_bin` | Optional link-step output override. Falls back to `output`. MSVC uses `/Fe{path}`. |
| `compile_only` | Flag to compile without linking (usually `-c`). |
| `dep_file` | Dep file generation flag. `{path}` substituted. Empty = no dep files (mtime-only). |
| `dep_file_mode` | Header dependency mode: `"file"` (default), `"stdout"` for compiler include tracing, or `"none"`. |
| `target` | Cross-compilation target flag. `{triple}` substituted. Empty = this template does not emit a target flag. |
| `sysroot` | Sysroot flag. `{path}` substituted. |
| `system_lib` | System library flag. `{name}` substituted. Default: `"-l{name}"`. MSVC uses `"{name}.lib"`; D compilers use `"-L-l{name}"`. |

### C++20 modules

```rhai
modules["style"]         = "gcc";                          // "gcc", "clang", or "none"
// GCC one-step (produces .o + .pcm in a single invocation):
modules["enable_flag"]   = "-fmodules-ts";
modules["compile_miu"]   = "-fmodule-output={pcm_path}";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-fmodule-header";
// Clang two-step (.pcm first, then .o):
modules["precompile"]    = "--precompile";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
```

### Linking metadata

```rhai
linking["cpp"] = #{
    abi:        "c++",            // ABI family (used in compatibility checks)
    compatible: ["c", "fortran"], // can link objects with these ABIs
    linker:     "",               // linker binary override; empty = use toolset["ld"]
    extensions: [".cpp", ".cc"],  // file extensions routed to this language key
};
```

An optional `compile_binary` key overrides which binary compiles files for that language key
(e.g. `gcc` for C files when the template is `g++`).

### System-library link templates and stubs

`system_lib` controls how Freight turns a resolved system dependency into a linker argument for
this compiler family:

```rhai
system_lib = "-l{name}";     // GCC/Clang default
system_lib = "{name}.lib";   // MSVC
system_lib = "-L-l{name}";   // D compilers that forward linker args with -L
```

This is separate from the bundled `toolchains/system-libs/*.toml` stubs. Stubs participate in
dependency resolution for version-style manifest entries (after pkg-config, Conan, and vcpkg) and
provide a package name, supported platform expression, headers for display, and a logical link name.
The selected compiler template then renders that logical link name through `system_lib`.

A minimal stub looks like this:

```toml
[package]
name = "pthread"
supports = "unix"

[lib]
link = "pthread"
hdrs = ["pthread.h"]
```

For example, `pthread = "0"` can resolve to the `pthread` stub on Unix; a GNU-like compiler emits
`-lpthread`, while an MSVC-style template would render the same logical link name as `pthread.lib`.

### Hooks

```rhai
fn check() {
    // Return false to hide when the toolchain is unavailable.
    find_tool("g++") != ()
}

// Arch-conditional load-time flags.
if arch == "x86_64" {
    load_flags["cxx"] += ["-m64"];
}
```

### Utility functions available in scripts

| Function / variable | Returns | Description |
|---|---------|---|
| `find_tool(name)` | `string \| ()` | Search `$PATH`; return full path or `()` |
| `arch` | string | Host CPU arch (`"x86_64"`, `"aarch64"`, …) |
| `os` | string | Host OS (`"linux"`, `"windows"`, `"macos"`, …) |
| `env[key]` | `string \| ()` | Read environment variable; `()` if not set |
| `load_flags["role"]` | `Array` | Append runtime flags: `load_flags["cxx"] += ["-m64"]` |

---

## Per-option callbacks

`compiler_option` and `language_option` let a template register handlers for
**compiler-specific keys** that have no universal equivalent — things like GPU
architecture, minimum version requirements, or assembler arch validation.
Standard options (`opt-level`, `warnings`, `lto`, `std`, etc.) are handled by
the existing flag maps and are not part of this system.

### Registration

```rhai
// Reads from [compiler.<name>] in freight.toml.
// Runs on every detected instance of this compiler, even when it is not the
// active backend — the active backend applies the flags; others validate only.
compiler_option("key", "default-value", |ctx| {
    // validate, add flags, or both
    // ctx.value is the manifest value, or "default-value" when the option is omitted
    // return a non-empty error string to abort the build; no return means success
});

// If there is no useful default, omit it and the handler runs only when set.
compiler_option("key", |ctx| {
    add_flag("--key=" + ctx.value);
});

// Reads from [language.<key>] in freight.toml.
// Runs only when the compiler is the active backend for that language.
language_option("key", "default-value", |ctx| {
    // no return means success
});
```

Unknown keys (no registered callback) are silently ignored — templates are
forwards-compatible by default. Use the two-argument registration form
`compiler_option("key", |ctx| { ... })` / `language_option("key", |ctx| { ... })`
when an option has no useful default value.

### `ctx` fields

| Field | Type | Description |
|---|---|---|
| `ctx.value` | string | Value from the manifest for this option, or the registration default when omitted |
| `ctx.version` | string | Detected compiler version string |
| `ctx.arch` | string | Effective target architecture (e.g. `"x86_64"`) |
| `ctx.os` | string | Effective target OS (e.g. `"linux"`) |
| `ctx.name` | string | Template name (e.g. `"clang++"`) |

### Injecting flags

Call the global `add_flag(s)` inside any callback to append compiler flags:

```rhai
compiler_option("sm_arch", |ctx| {
    add_flag("--gpu-architecture=" + ctx.value);
});
```

Flags from `compiler_option` apply globally (all languages the compiler
handles). Flags from `language_option` apply only to sources of that language.

### Version comparison helpers

The following functions are available in all option callbacks:

| Function | Returns |
|---|---|
| `version_gte(a, b)` | `true` if `a >= b` |
| `version_lte(a, b)` | `true` if `a <= b` |
| `version_gt(a, b)` | `true` if `a > b` |
| `version_lt(a, b)` | `true` if `a < b` |

Versions are compared component-by-component after splitting on `.`, ignoring
any `-suffix`. Malformed strings fall back to lexicographic comparison.

### Examples from bundled templates

**`nvcc.rhai`** — GPU target architecture:

```rhai
compiler_option("sm_arch", |ctx| {
    add_flag("--gpu-architecture=" + ctx.value);
});
```

**`nasm.rhai` / `yasm.rhai`** — arch validation via `language_option`:

```rhai
language_option("arch", |ctx| {
    if ctx.arch != ctx.value {
        return "assembler requires arch '" + ctx.value +
               "' but the effective target is '" + ctx.arch + "'";
    }
});
```

### Manifest syntax

Compiler version constraints use semver range syntax (same as package
dependencies) in the `version` field of `[compiler.<name>]`. Freight
validates the constraint at build time and aborts with a clear error.

```toml
# Compiler-specific options — dispatched to compiler_option() callbacks.
[compiler.clang++]
version = ">=14.0"      # require clang++ 14 or newer

[compiler.nvcc]
version  = ">=11.8"     # require nvcc 11.8+
sm_arch  = "sm_89"

[compiler.g++]
version = ">=12.0, <15" # pin to a specific g++ range

# Language-specific options — dispatched to language_option() callbacks.
# Only applied when that language's source files are actually present.
[language.asm]
arch = "x86_64"
```

---

## Debugger templates

Debugger templates live alongside their compiler family and use `kind = "debugger"` to identify
themselves to the loader. The same `include` mechanism works for sharing base code.

```rhai
// gnu/gdb.rhai

kind          = "debugger";
name          = "gdb";
binary        = "gdb";
version_arg   = "--version";
version_regex = "GNU gdb[^\\d]+(\\d+\\.\\d+)";

// gdb --args <binary> [args]
launch["separator"] = "--args";  // separator between debugger flags and the debuggee

// DAP adapter configuration for IDE integration.
dap["binaries"]    = [];          // adapter binaries probed in order (empty = none)
dap["vscode_type"] = "cppdbg";
dap["mi_mode"]     = "gdb";

// Named settings resolved through ~/.freight/config.toml [debugger.gdb].
settings["tui"]   = "--tui";    // full-screen TUI mode
settings["quiet"] = "-q";       // suppress banner
settings["batch"] = "--batch";  // non-interactive batch mode

// default_args = ["--quiet"];  // unconditional extra flags

fn check() {
    find_tool("gdb") != ()
}
```

Debugger configuration is a **developer concern**, not a project concern. It lives in
`~/.freight/config.toml` (global) or `<project>/.freight/config.toml` (local override).
Neither file is part of `freight.toml`.

```toml
# ~/.freight/config.toml
default_debugger = "gdb"

[debugger.gdb]
args  = ["--tui"]   # raw extra flags
tui   = true        # resolved via gdb.rhai's settings map → --tui
quiet = true
```

Project-local overrides use the same format in `<project>/.freight/config.toml`.
For example, a developer can keep package metadata portable in `freight.toml`
while selecting LLVM tools locally:

```toml
# <project>/.freight/config.toml
default_backend = "clang"
default_debugger = "lldb"
```

---

## Formatter and linter templates

Formatter and linter templates use `kind = "formatter"` or `kind = "linter"`. They support
the same `include` mechanism and `fn check()` hook as other template types. They live in their
own subdirectory when standalone, or alongside their family (e.g. `llvm/clang-format.rhai`).

```rhai
// llvm/clang-format.rhai

kind          = "formatter";
name          = "clang-format";
family        = "llvm";
binary        = "clang-format";
version_arg   = "--version";
version_regex = "clang-format version (\\d+\\.\\d+\\.\\d+)";
extensions    = [".cpp", ".cc", ".cxx", ".c", ".h", ".hpp", ".cu"];

// fix:   reformat files in-place
// check: exit non-zero if any file would change (CI use)
run["fix"]   = "-i";
run["check"] = "--dry-run --Werror";

// Named settings resolved from [formatter] in freight.toml.
// Pattern {value} is substituted with the manifest value.
settings["style"]  = "--style={value}";
settings["config"] = "--style=file:{value}";

// Valid values exposed to the LSP for completions and printed as hints.
// Omit keys whose values are freeform (paths, numbers, regex strings).
values["style"] = ["Google", "LLVM", "Mozilla", "WebKit", "Chromium", "Microsoft", "GNU", "file"];

fn check() {
    find_tool("clang-format") != ()
}
```

### Formatter / linter fields

| Field | Description |
|---|---|
| `kind` | `"formatter"` or `"linter"` — required |
| `name` | Tool name; matched against `[formatter] name` / `[linter] name` in `freight.toml` |
| `family` | Family group (cosmetic; used for display) |
| `binary` | Binary probed on PATH |
| `version_arg` | Argument to print version (e.g. `"--version"`) |
| `version_regex` | Regex with one capture group extracting the version string |
| `extensions` | File extensions this tool acts on |
| `run["fix"]` | Flags for in-place modification mode (`freight fmt` / `freight lint --fix`) |
| `run["check"]` | Flags for report-only mode (`freight fmt --check` / `freight lint`) |
| `settings["key"]` | Flag pattern with `{value}` substituted from `freight.toml` |
| `values["key"]` | Array of valid choices for a setting (used by LSP completions) |

### Formatter/linter config in `freight.toml`

```toml
[formatter]
# name is optional — freight picks the first detected formatter when absent
name   = "clang-format"
style  = "Google"          # → --style=Google   (from settings["style"])
# config = ".clang-format" # → --style=file:.clang-format

[linter]
name   = "cppcheck"
enable = "warning,style"   # → --enable=warning,style
std    = "c++17"           # → --std=c++17
```

Settings keys come directly from the template's `settings` map — unknown keys are silently
ignored, so switching tools doesn't require removing old settings.

On first run without any `[formatter]`/`[linter]` config, freight prints a hint listing the
available settings and their valid values for the detected tool.

### Bundled formatter templates

| Template | Tool | Languages | Check mode |
|---|---|---|---|
| `llvm/clang-format.rhai` | `clang-format` | C, C++, CUDA | `--dry-run --Werror` |
| `astyle/astyle.rhai` | `astyle` | C, C++, Java, C# | `--dry-run` (exits 0) |
| `uncrustify/uncrustify.rhai` | `uncrustify` | C, C++, Java, C# | `--check` |
| `fprettify/fprettify.rhai` | `fprettify` | Fortran | `--check` |

### Bundled linter templates

| Template | Tool | Focus |
|---|---|---|
| `llvm/clang-tidy.rhai` | `clang-tidy` | Modernization, bugprone, readability |
| `cppcheck/cppcheck.rhai` | `cppcheck` | Static analysis, undefined behaviour |
| `cpplint/cpplint.rhai` | `cpplint` | Google style guide enforcement |
| `flawfinder/flawfinder.rhai` | `flawfinder` | Security / CWE scanning |

---

## Writing a new compiler script

### Checklist

1. Create `toolchains/<family>/<name>.rhai` (or `toolchains/<name>.rhai` for standalones)
2. Set `kind = "compiler"` and `name` matching the actual binary (e.g. `"clang++"`)
3. Set `family` if the compiler belongs to a suite; leave empty for standalone tools
4. Set `requires_toolchain` if the compiler wraps another toolchain; leave empty for self-contained compilers
5. Use `include` to pull in a `_base` file for shared flags and a `languages/_<lang>` file for extensions/standards/linking
6. Set `binary`, `version_arg`, `version_regex`
7. Add `required_tools`, `required_env`, or `min_version` when the binary alone is not enough to prove availability
8. Add flag maps: at minimum `opt`, `dbg`, `warnings`, `lto`; add `lto_link` when link-time spelling differs
9. Add `defaults["std"]` in the language shared file if a default standard applies
10. Set structure fields — at minimum `include_dir`, `define`, `output`, `compile_only`
11. Add `output_obj`, `output_bin`, `dep_file_mode`, or `system_lib` for compilers whose compile/link conventions differ
12. Add `dep_file` if the compiler supports Makefile dep files
13. Add `fn check()` to hide the toolchain when the binary is absent
14. Test with `freight toolchain list` to verify detection, then `freight build` on a real project

### Writing a new debugger script

1. Create `toolchains/<family>/<name>.rhai`
2. Set `kind = "debugger"` — this is mandatory
3. Set `name`, `binary`, `version_arg`, `version_regex`
4. Set `launch["separator"]` — the token between debugger flags and the debuggee binary
5. Set `dap[...]` entries for IDE integration
6. Add a `settings` map for named options the developer can enable in their config
7. Add `fn check()` to hide the tool when the binary is absent

### Writing a new formatter or linter script

1. Create `toolchains/<family>/<name>.rhai` (or `toolchains/<name>/<name>.rhai` for standalone tools)
2. Set `kind = "formatter"` or `kind = "linter"` — this is mandatory
3. Set `name`, `family`, `binary`, `version_arg`, `version_regex`, `extensions`
4. Set `run["fix"]` and `run["check"]` — flags for each mode
5. Add `settings["key"] = "--flag={value}"` for every configurable option
6. Add `values["key"] = [...]` for settings with a fixed set of valid choices
7. Add `fn check()` to hide the tool when the binary is absent

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
`g++.rhai` and `gfortran.rhai` and they appear together as the `gnu` toolchain:

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
- `nasm`, `yasm` (`requires_toolchain = ["c"]`) — active whenever any C toolchain is present

Guests appear in the "Extensions" section of `freight toolchain list`. If a required
language key is not provided by any detected compiler, the guest is silently dropped.

### One script per toolchain, not per language

`g++.rhai` handles both C and C++ by including both `_gnu-base` and `../languages/_cpp`.
The `linking["c"]` entry's optional `compile_binary` field selects `gcc` for C files while
`g++` handles C++ and linking.

### Language shared files

`toolchains/languages/_cpp.rhai`, `_c.rhai`, and `_fortran.rhai` centralise parameters that
are identical across all compilers of that language (extensions, default standard, standards
map, linking metadata). Compiler files `include` the relevant language file so there is one
place to update when a new standard is ratified.

### `defaults` map

`defaults["std"] = "c++17"` in a language file provides a fallback applied when `freight.toml`
omits `[language.cpp] std = ...`. It is resolved inside `assemble_flags` before the manifest
value, so the manifest always wins when present.

### Arch-specific flags

`arch_flags["x86_64.linux"] = "-f elf64"` is how NASM selects its output format. Freight
looks up `"arch.os"` first, then `"arch"` as a fallback.

### Load-time arch-conditional flags

Arch-conditional flags are set with `load_flags`. Top-level code is preferred for simple cases;
`fn load()` is also called after evaluation for templates that want to group load-time logic:

```rhai
if arch == "x86_64" {
    load_flags["cxx"] += ["-m64"];
}
```

### Automatic CPU tuning from target sysroots

For GNU-like C/C++ compilers (`family = "gnu"`, `"llvm"`, or `"intel"`), Freight can derive a
small set of CPU tuning flags from `[compiler] target` plus `[compiler] sysroot`. This happens only
when `auto_cpu_tuning` is enabled in the build settings, the template handles `c` or `cpp`, and no
explicit CPU-tuning flag is already present in template or manifest flags.

Examples of derived flags include:

| Target/sysroot hint | Derived flags |
|---|---|
| `aarch64...` with `neoverse-n2` in the sysroot path | `-mcpu=neoverse-n2` |
| generic `aarch64...` | `-march=armv8-a` |
| ARM EABI hard-float sysroots | `-mfloat-abi=hard` |
| `riscv64...` | `-march=rv64gc -mabi=lp64d` unless the sysroot names a more specific ISA/ABI |
| `x86_64...` with `x86-64-v3`, `znver4`, `skylake`, etc. in the sysroot path | `-march=<hint>` |

Templates do not need to opt in beyond using a GNU-like family/name and declaring C/C++ linking
metadata. Set explicit flags such as `-march=...`, `-mcpu=...`, `-mtune=...`, or `-mabi=...` in the
manifest when you need to override the heuristic.

### `fn check()` for availability detection

Return `false` to hide the toolchain when its binary isn't present or the platform is
unsupported. Load-time side effects should use top-level code or `fn load()` plus `load_flags`.

```rhai
fn check() {
    if arch != "x86_64" && arch != "aarch64" { return false; }
    find_tool("nvcc") != () && find_tool("ptxas") != ()
}
```
