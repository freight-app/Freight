# Plan: Toolchain Roles + Scripted Templates

## Motivation

The current TOML template assigns one `binary` field that does triple duty:
compile, link, and implicitly archive. That works for GCC/Clang because
`g++`/`clang++` handle all three. It breaks for anything else:

| Problem | Root cause |
|---|---|
| MSVC needs `lib.exe` for static libs | No `ar` role |
| MSVC needs `link.exe` for linking | No separate `ld` role |
| LTO `/GL` (compile) + `/LTCG` (link) | One flag set, not two |
| `/Fo` for objects, `/Fe` for executables | Single `output` field |
| System libs as `name.lib` not `-lname` | Hardcoded `-l{name}` format |
| Header dep tracking via stdout (`/showIncludes`) | Only `.d` file mode |
| Arch-conditional flags (e.g. `-m64` only on x86_64) | No runtime logic |

xmake solves this with explicit toolset roles and Lua callbacks (`on_check`,
`on_load`). A similar two-part extension fits crane well:

1. **Toolset roles in TOML** — static, covers ~90% of compilers including MSVC.
2. **Optional Rhai scripting** — for arch-conditional flags and custom detection.
   Rhai is pure Rust, sandboxed, and has a familiar C-like syntax.

---

## Part 1 — Toolset roles in TOML

### New `[toolset]` section

```toml
[toolset]
cc    = "gcc"        # C compilation
cxx   = "g++"        # C++ compilation
ld    = "g++"        # final link (binary / shared lib)
ar    = "ar"         # static archive creation
strip = "strip"      # strip debug symbols
as    = "as"         # assembler (if separate)
```

All fields are optional. Fallback order:

- `cc` → falls back to `binary`
- `cxx` → falls back to `binary`
- `ld` → falls back to `binary`
- `ar` → falls back to platform default (`ar` on Unix, `lib.exe` on Windows when `name = "msvc"`)
- `strip` → falls back to platform default or skipped if empty

This immediately lets MSVC declare:

```toml
[toolset]
cc    = "cl.exe"
cxx   = "cl.exe"
ld    = "link.exe"
ar    = "lib.exe"
strip = ""           # not applicable
```

### Separate output flags: `output_obj` and `output_bin`

```toml
[structure]
output_obj  = "-o {path}"     # GCC/Clang: object file
output_bin  = "-o {path}"     # GCC/Clang: executable (same for GCC)

# MSVC:
output_obj  = "/Fo{path}"
output_bin  = "/Fe{path}"
```

`compile.rs` calls `output_obj_flag()`; `link.rs` calls `output_bin_flag()`.
The current `output` field is kept as a fallback for templates that don't
distinguish (i.e. every existing template works without changes).

### Per-role flag overrides: `[flags.link]`

Some flags mean different things at compile vs link time:

```toml
[flags]
lto.true  = "/GL"      # compile-side (existing behaviour)
lto.false = ""

[flags.link]
lto.true  = "/LTCG"   # link-side, passed to ld/ar invocations
lto.false = ""
```

When `[flags.link]` is absent, the top-level `[flags]` values are used for
both — so existing templates are unaffected.

### System library flag format

```toml
[structure]
system_lib = "-l{name}"        # GCC/Clang (default)
# MSVC:
system_lib = "{name}.lib"
```

`collect_system_lib_flags` in `link.rs` uses this instead of hardcoding `-l`.

### Dependency file mode

```toml
[structure]
dep_file      = "-MMD -MF {path}"    # writes a .d file (existing)
dep_file_mode = "file"               # "file" | "stdout" | "none"
```

`"stdout"` means the compiler writes `Note: including file: path\to\header.h`
to stdout during compilation. `compile.rs` intercepts and parses those lines
instead of reading a `.d` file. `"none"` disables header tracking entirely.

---

## Part 2 — Optional Rhai scripting

For cases that can't be expressed statically — arch-conditional flags,
environment probing, versioned toolchain variants — a small embedded script
block handles it.

[Rhai](https://rhai.rs/) is pure Rust, sandboxed, ~200 KB added to the binary,
no C FFI. Syntax is familiar (C-like). This is a deliberate non-Lua choice:
no external dependency, no safety concerns, easy to sandbox.

### Template with scripting

```toml
name   = "gcc"
binary = "g++"
# ... rest of template unchanged ...

[toolset]
cc    = "gcc"
cxx   = "g++"
ld    = "g++"
ar    = "ar"
strip = "strip"

# Called once at startup to confirm the toolchain is present.
# Return true = available, false = skip this template.
on_check = """
    let tool = find_tool("g++");
    tool != ()
"""

# Called after detection, before any build. Mutates the toolchain config.
on_load = """
    if arch == "x86_64" || arch == "x64" {
        add_flags("cc",  "-m64");
        add_flags("cxx", "-m64");
        add_flags("ld",  "-m64");
    } else if arch == "i386" || arch == "x86" {
        add_flags("cc",  "-m32");
        add_flags("cxx", "-m32");
        add_flags("ld",  "-m32");
    }
"""
```

### Rhai API exposed to scripts

| Function / variable | Type | Description |
|---|---|---|
| `arch` | `string` | host (or target) arch, e.g. `"x86_64"` |
| `os` | `string` | host OS, e.g. `"linux"`, `"windows"` |
| `find_tool(name)` | `string \| ()` | searches `$PATH`, returns path or unit |
| `add_flags(role, flags)` | — | appends flags to a toolset role's compile/link invocation |
| `set_binary(role, path)` | — | overrides a toolset binary at load time |
| `env(key)` | `string \| ()` | reads an environment variable |

The sandbox: no filesystem access, no process spawning, no network. Only the
above surface is exposed. Scripts run in a step-limited engine (max 100 000
ops) to prevent infinite loops.

---

## What this changes in Rust

### `toolchain/template.rs`

- `RawTemplate` gains `toolset: Option<RawToolset>`, `on_check: Option<String>`,
  `on_load: Option<String>`, `flags_link: Option<RawFlags>`,
  `structure.output_obj`, `structure.output_bin`, `structure.system_lib`,
  `structure.dep_file_mode`.
- `CompilerTemplate` gains `toolset: ToolsetRoles` (resolved with fallbacks),
  `link_flags: Option<FlagSet>`, `system_lib_fmt: String`, `dep_file_mode: DepFileMode`.
- `assemble_flags()` unchanged — still compile-side only.
- New `assemble_link_flags()` — uses `flags_link` when present, otherwise same as `assemble_flags()`.
- New `output_obj_flag()` and `output_bin_flag()` replacing single `output_flag()`.
- New `system_lib_flag(name)` using `system_lib_fmt`.

### `toolchain/detect.rs`

- `detect_compiler()` runs `on_check` script (if present) to confirm availability.
- `load_template()` runs `on_load` script (if present) after parse, mutates `ToolsetRoles` and `always_flags`.

### `build/compile.rs`

- Compile binary comes from `toolset.cc` / `toolset.cxx` instead of `linking[lang].compile_binary`.
- Uses `output_obj_flag()`.
- `dep_file_mode = "stdout"`: spawn compiler with stdout captured, parse `Note: including file:` lines, write synthetic `.d` file to same location as before (so the rest of the dirty-check logic is unchanged).

### `build/link.rs`

- Link binary: `toolset.ld` instead of `template.binary`.
- Archive binary: `toolset.ar` instead of hardcoded `ar`.
- Strip binary: `toolset.strip` instead of hardcoded `strip` (or the linker `-s` flag — template decides).
- Uses `output_bin_flag()`.
- Uses `assemble_link_flags()` instead of `assemble_flags()` for the link step.
- `system_lib_flag()` replaces hardcoded `-l{name}`.

---

## Resulting MSVC template

```toml
name          = "msvc"
binary        = "cl.exe"
version_arg   = ""
version_regex = "Version (\\d+\\.\\d+\\.\\d+\\.\\d+)"

[toolset]
cc    = "cl.exe"
cxx   = "cl.exe"
ld    = "link.exe"
ar    = "lib.exe"
strip = ""

[extensions]
handles = [".cpp", ".cc", ".cxx", ".c++", ".c"]

[flags]
opt.0            = "/Od"
opt.1            = "/O1"
opt.2            = "/O2"
opt.3            = "/Ox"
debug.true       = "/Zi /FS"
debug.false      = ""
warnings.none    = "/W0"
warnings.default = "/W3"
warnings.all     = "/W4"
warnings.error   = "/W4 /WX"
lto.true         = "/GL"
lto.false        = ""
strip.true       = ""
strip.false      = ""
sanitize         = "/fsanitize={values}"
cpu_extension    = ""

[flags.link]
lto.true  = "/LTCG"
lto.false = ""

[standards]
"c++17"  = "/std:c++17"
"c++20"  = "/std:c++20"
"c++23"  = "/std:c++latest"
"c17"    = "/std:c17"

[structure]
include_dir   = "/I{path}"
define        = "/D{name}"
define_value  = "/D{name}={value}"
output_obj    = "/Fo{path}"
output_bin    = "/Fe{path}"
compile_only  = "/c"
dep_file      = "/showIncludes"
dep_file_mode = "stdout"
system_lib    = "{name}.lib"
target        = ""
sysroot       = ""

[modules]
supported = false

[passthrough]
enabled = false
prefix  = ""

[linking.cpp]
abi        = "c++"
compatible = ["c"]
linker     = ""
extensions = [".cpp", ".cc", ".cxx", ".c++"]

[linking.c]
abi        = "c"
compatible = []
linker     = ""
extensions = [".c"]

on_check = """
    let tool = find_tool("cl.exe");
    tool != ()
"""

on_load = """
    if arch == "x86_64" || arch == "x64" {
        add_flags("cc",  "/arch:AVX2");   # example; user would set via [compiler] flags
    }
"""
```

Every bottleneck from the MSVC analysis is now addressed in TOML — no
dedicated Rust backend required.

---

## Backwards compatibility

- Every existing template (gcc, clang, gfortran, etc.) parses without changes.
- `[toolset]` is optional; missing roles fall back to `binary` exactly as today.
- `output_obj` / `output_bin` are optional; `output` is the fallback.
- `[flags.link]` is optional; missing means link uses compile flags (current behaviour).
- `system_lib` defaults to `"-l{name}"` if absent.
- `dep_file_mode` defaults to `"file"` if absent.
- `on_check` / `on_load` are optional; missing means current detection path.

---

## Phases

| # | Work | Rhai needed? |
|---|---|---|
| 1 | `[toolset]` roles, fallback logic, update compile/link/ar callers | No |
| 2 | `output_obj` / `output_bin`, `[flags.link]`, `system_lib` | No |
| 3 | `dep_file_mode = "stdout"` stdout parsing | No |
| 4 | Rhai engine embed, `on_check` / `on_load` callbacks | Yes |
| 5 | MSVC template using the new fields | No (after 1-3) |
| 6 | Port existing templates to declare explicit `[toolset]` | No |

Phases 1–3 are pure TOML extension + Rust wiring. They solve MSVC and the
linker separation problem without introducing any scripting dependency.
Phase 4 (Rhai) is only needed for arch-conditional flags and versioned
toolchains (`gcc-12`, `gcc-13`). It can ship later without blocking MSVC.
