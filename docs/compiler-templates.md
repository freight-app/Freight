# Compiler Templates

Crane's compiler system is fully data-driven. Every supported compiler or assembler is described
by a `.toml` file in `toolchains/` — no Rust changes required. Adding a new compiler means writing
a new TOML file, installing it, and restarting crane.

---

## Loading order

Crane merges templates from two locations at startup:

1. **Bundled templates** — shipped with the crane binary in `toolchains/`
2. **User templates** — installed in `~/.crane/templates/` via `crane toolchain add <path>`

User templates with the same `name` as a bundled template take precedence (override). This lets
you ship a patched version of `gcc.toml` without modifying the bundled copy.

---

## Template structure

A compiler template is a flat TOML file. Here is a fully annotated example:

```toml
# ── Identity ──────────────────────────────────────────────────────────────────

name          = "gcc"      # logical name; used for backend = "gcc" in crane.toml
binary        = "g++"      # executable used for linking (and default compilation)
version_arg   = "--version"
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b"  # first capture group → version string

# ── Source extensions handled by this template ────────────────────────────────
# Extensions listed here determine which files are routed to this template during
# source discovery. A file is only compiled if its extension appears here OR in a
# [linking.<key>].extensions list.

[extensions]
handles = [".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"]

# ── Flag table ────────────────────────────────────────────────────────────────
# crane resolves abstract settings (opt-level, debug, …) to concrete flags
# using these tables. Missing keys are silently omitted.

[flags]
opt.0            = "-O0"
opt.1            = "-O1"
opt.2            = "-O2"
opt.3            = "-O3"
debug.true       = "-g"
debug.false      = ""
warnings.none    = ""
warnings.default = "-Wall"
warnings.all     = "-Wall -Wextra -Wpedantic"
warnings.error   = "-Wall -Wextra -Wpedantic -Werror"
lto.true         = "-flto"
lto.false        = ""
strip.true       = "-s"
strip.false      = ""
sanitize         = "-fsanitize={values}"   # {values} = comma-joined list, e.g. "address,undefined"
cpu_extension    = "-m{name}"              # {name} = extension name, e.g. avx2 → -mavx2
                                           # empty string = unsupported (cpu_extensions are silently skipped)

# ── Language standards ────────────────────────────────────────────────────────
# Keyed by the value the user writes in [language.<key>].std.
# crane passes the value from this table verbatim to the compiler.

[standards]
"c11"   = "-std=c11"
"c17"   = "-std=c17"
"c23"   = "-std=c23"
"c++17" = "-std=c++17"
"c++20" = "-std=c++20"
"c++23" = "-std=c++23"
"f2003" = "-std=f2003"
"f2008" = "-std=f2008"

# ── Structural flags ──────────────────────────────────────────────────────────
# Patterns for flags whose content varies per invocation.
# {path}, {name}, {value}, {triple} are substituted at build time.

[structure]
include_dir  = "-I{path}"
define       = "-D{name}"
define_value = "-D{name}={value}"
output       = "-o {path}"
compile_only = "-c"
dep_file     = "-MMD -MF {path}"   # Makefile dep file for header dirty-tracking; omit if unsupported
target       = "--target={triple}" # empty string = unsupported (e.g. GCC needs a cross binary instead)
sysroot      = "--sysroot={path}"  # empty string = unsupported

# ── C++20 module support ──────────────────────────────────────────────────────
# Set supported = false (or omit) for compilers that don't support modules.

[modules]
supported  = true

# GCC one-step strategy: a single invocation produces both .o and .pcm.
enable_flag   = "-fmodules-ts"
compile_miu   = "-fmodule-output={pcm_path}"
import_module = "-fmodule-file={name}={pcm_path}"

# Clang two-step strategy: --precompile → .pcm, then -c → .o
# enable_flag  = ""
# precompile   = "--precompile"
# import_module = "-fmodule-file={name}={pcm_path}"

# ── Passthrough mode ──────────────────────────────────────────────────────────
# When enabled, the binary is not probed with version_arg and is always treated
# as available. All flags are forwarded unchanged. Useful for wrapper scripts.

[passthrough]
enabled = false
prefix  = ""

# ── Architecture-dependent flags ─────────────────────────────────────────────
# Looked up as "arch.os" first, then "arch" as fallback.
# Used by NASM-style assemblers to select the output format.

[arch_flags]
"x86_64.linux"   = "-f elf64"
"x86_64.macos"   = "-f macho64"
"x86_64.windows" = "-f win64"
"i686.linux"     = "-f elf32"
"aarch64.linux"  = "-f elf64"

# ── Language linking keys ─────────────────────────────────────────────────────
# Each [linking.<key>] declares this template's role for one language.
# A template can handle multiple languages (e.g. gcc handles both C and C++).

[linking.c]
abi            = "c"        # ABI name used for compatibility checking between deps
compile_binary = "gcc"      # override the top-level binary for compilation only (linking still uses `binary`)
compatible     = ["fortran", "asm"]  # ABIs this language can link with
linker         = ""         # custom linker binary (empty = use `binary`)
extensions     = [".c", ".s", ".S"]

[linking.cpp]
abi        = "c++"
compatible = ["c", "fortran", "asm"]
linker     = ""
extensions = [".cpp", ".cppm", ".cc", ".cxx", ".c++"]
```

---

## Key design decisions

### One template per toolchain

A single `gcc.toml` handles both C and C++. The `[linking.c]` section overrides
`compile_binary = "gcc"` so C files are compiled with `gcc`, not `g++`, while `g++` is still used
for linking (as required for mixed C/C++ projects).

### Extensions vs. language keys

Extensions listed in `[extensions].handles` and in `[linking.<key>].extensions` both route files
to this template during source discovery. The `[linking.<key>].extensions` list is the authoritative
set for a specific language binding — it drives the ABI-compatibility check for path deps.

### Two-step vs. one-step module compilation

GCC and Clang handle C++20 module interface units differently:
- **GCC** uses `compile_miu` (a single `-fmodule-output=` pass that produces both `.o` and `.pcm`)
- **Clang** uses `precompile` (produces only `.pcm`) followed by a regular `-c` pass

The template field that is absent signals which strategy is used: if `precompile` exists, crane
uses the two-step path; if `compile_miu` exists, one-step.

### `compile_binary` override

For templates where the linker binary (`binary`) differs from the compiler binary
(e.g. `g++` links, `gcc` compiles C), set `compile_binary` in the relevant `[linking.<key>]`
section. The override applies only to compilation, not linking.

---

## Debugger templates

Debugger templates live in `toolchains/debuggers/` and are loaded separately from compiler
templates. They share the same TOML-file philosophy but have a different schema:

```toml
name          = "lldb"
binary        = "lldb"
version_arg   = "--version"
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b"

[launch]
separator = "--"   # separator between debugger flags and the program + its args
                   # lldb uses "--", gdb uses "--args"

[dap]
# DAP adapter binaries searched in order; first found is used as the adapter path
binaries    = ["lldb-dap", "lldb-vscode"]
vscode_type = "lldb"    # "type" in the generated launch.json config (CodeLLDB uses "lldb")
mi_mode     = "lldb"    # "MIMode" in cppdbg-type configs; irrelevant for "lldb" type
```

`crane debug` uses debugger templates to:
1. Select and invoke an interactive debugger session (`exec()` on Unix for a clean terminal hand-off)
2. Generate `.vscode/launch.json` entries via `crane debug --launch-json`

---

## Writing a new compiler template

### Checklist

1. Create `toolchains/<name>.toml`
2. Fill `name`, `binary`, `version_arg`, `version_regex`
3. Add `[extensions].handles` with the file extensions this compiler claims
4. Fill `[flags]` — `opt`, `debug`, `warnings` at minimum
5. Add `[standards]` for every standard string users might write
6. Fill `[structure]` — at minimum `include_dir`, `define`, `output`, `compile_only`
7. Add `dep_file` if the compiler supports Makefile dep files (`-MMD -MF`)
8. Add `[linking.<key>]` for every language the template handles
9. Set `[modules].supported = false` (or skip `[modules]`) unless the compiler supports C++20 modules
10. Test with `crane toolchain list` to verify detection, then `crane build` on a real project

### Installing

```sh
crane toolchain add path/to/mycompiler.toml
```

This validates the template (checking required fields and type correctness) and copies it to
`~/.crane/templates/mycompiler.toml`. The new template is loaded on the next crane invocation.

### Template for a hypothetical `zig cc` wrapper

```toml
name          = "zig-cc"
binary        = "zig"
version_arg   = "version"
version_regex = "(\\d+\\.\\d+\\.\\d+)"

[extensions]
handles = [".c", ".cpp", ".cc"]

[flags]
opt.0            = "-O0"
opt.1            = "-O1"
opt.2            = "-O2"
opt.3            = "-O3"
debug.true       = "-g"
debug.false      = ""
warnings.none    = ""
warnings.default = "-Wall"
warnings.all     = "-Wall -Wextra"
warnings.error   = "-Wall -Wextra -Werror"
lto.true         = "-flto"
lto.false        = ""
strip.true       = "-s"
strip.false      = ""
cpu_extension    = "-m{name}"

[standards]
"c11"   = "-std=c11"
"c17"   = "-std=c17"
"c++17" = "-std=c++17"
"c++20" = "-std=c++20"

[structure]
include_dir  = "-I{path}"
define       = "-D{name}"
define_value = "-D{name}={value}"
output       = "-o {path}"
compile_only = "-c"
dep_file     = "-MMD -MF {path}"
target       = "-target {triple}"
sysroot      = "--sysroot {path}"

[modules]
supported = false

[passthrough]
enabled = false
prefix  = ""

[linking.c]
abi            = "c"
compile_binary = "zig cc"   # zig cc for C, zig c++ for C++
compatible     = ["asm"]
linker         = ""
extensions     = [".c"]

[linking.cpp]
abi        = "c++"
compatible = ["c", "asm"]
linker     = ""
extensions = [".cpp", ".cc", ".cxx"]
```
