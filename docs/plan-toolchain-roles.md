# Plan: Rhai-based toolchain scripts

## Core idea

The TOML compiler templates are replaced entirely by Rhai scripts.
`freight.toml` stays — it is the project manifest.
Toolchain definitions move to `toolchains/*.rhai`.

```
freight/
├── freight.toml                  ← project: deps, language settings, profiles
└── toolchains/                 ← compiler definitions (was *.toml, now *.rhai)
    ├── gcc.rhai
    ├── clang.rhai
    ├── msvc.rhai
    ├── gfortran.rhai
    └── …
```

**Why this is better than TOML + optional scripting:**

- A TOML template is a static subset of what a script can already express.
  Having both formats means two parsers, two mental models, and a permanent
  impedance mismatch between what compilers need and what TOML can describe.
- Scripted definitions handle every MSVC problem natively: separate binaries
  per role, split LTO flags, stdout dep parsing, environment setup — all
  expressible as normal code.
- Community toolchains are first-class: drop a `.rhai` file in
  `~/.freight/toolchains/`, no Rust required.
- Versioned toolchains (`gcc-12`, `gcc-13`) are just parametric scripts, not
  separate files.

[Rhai](https://rhai.rs/) is pure Rust, sandboxed by default, ~200 KB,
no C FFI, familiar C-like syntax. Correct choice over Lua for a Rust project.

---

## Rhai API surface

Freight's engine exposes these functions to every toolchain script.

### Declaration functions (called at script evaluation time)

```rhai
set_name("gcc");                        // toolchain identifier
set_homepage("https://gcc.gnu.org/");   // informational

// Role → binary mapping
set_toolset("cc",    "gcc");    // C compilation
set_toolset("cxx",   "g++");    // C++ compilation
set_toolset("ld",    "g++");    // final link (binary / shared lib)
set_toolset("ar",    "ar");     // static archive creation
set_toolset("strip", "strip");  // strip debug symbols
set_toolset("as",    "as");     // assembler

// Flag maps — key is the freight.toml abstract value
set_flag("opt",      "0", "-O0");
set_flag("opt",      "2", "-O2");
set_flag("debug",    "true",  "-g");
set_flag("debug",    "false", "");
set_flag("warnings", "all",   "-Wall -Wextra -Wpedantic");
set_flag("lto",      "true",  "-flto");
set_flag("lto_link", "true",  "-flto");  // link-step override for lto
set_flag("strip",    "true",  "-s");
set_flag("sanitize", "template", "-fsanitize={values}");
set_flag("cpu_ext",  "template", "-m{name}");

// Language standards
set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");

// Structure templates
set_structure("include_dir",   "-I{path}");
set_structure("define",        "-D{name}");
set_structure("define_value",  "-D{name}={value}");
set_structure("output_obj",    "-o {path}");
set_structure("output_bin",    "-o {path}");
set_structure("compile_only",  "-c");
set_structure("dep_file",      "-MMD -MF {path}");
set_structure("dep_file_mode", "file");    // "file" | "stdout" | "none"
set_structure("system_lib",    "-l{name}");
set_structure("target",        "--target={triple}");
set_structure("sysroot",       "--sysroot={path}");

// File extensions this toolchain handles
set_extensions([".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"]);

// Flags always appended (e.g. nvcc needs --expt-relaxed-constexpr)
add_always_flag("--some-flag");

// Version detection
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

// C++20 module strategy
set_module_style("gcc",   #{
    enable_flag:   "-fmodules-ts",
    compile_miu:   "-fmodule-output={pcm_path}",
    import_module: "-fmodule-file={name}={pcm_path}",
    header_unit:   "-fmodule-header",
});
// or:
set_module_style("clang", #{
    precompile:    "--precompile",
    import_module: "-fmodule-file={name}={pcm_path}",
    header_unit:   "-x c++-header",
});
// or: set_module_style("none");

// Linking metadata
set_linking("cpp", #{ abi: "c++", compatible: ["c", "fortran"], extensions: [".cpp", ".cc"] });
set_linking("c",   #{ abi: "c",   compatible: [],               extensions: [".c"] });

// Passthrough wrapper (nvcc -Xcompiler pattern)
set_passthrough(true, "-Xcompiler");
```

### Callbacks

```rhai
// Return true if this toolchain is available on the current system.
on_check(|| {
    find_tool("g++") != ()
});

// Mutate the toolchain after detection — runs once before any build.
// `arch` and `os` are pre-bound variables.
on_load(|| {
    if arch == "x86_64" || arch == "x64" {
        add_flags("cc",  ["-m64"]);
        add_flags("cxx", ["-m64"]);
        add_flags("ld",  ["-m64"]);
    } else if arch == "i386" || arch == "x86" {
        add_flags("cc",  ["-m32"]);
        add_flags("cxx", ["-m32"]);
        add_flags("ld",  ["-m32"]);
    }
});
```

### Utility functions available in scripts

| Function | Returns | Description |
|---|---|---|
| `find_tool(name)` | `string \| ()` | search `$PATH`, return full path or unit |
| `run(binary, args)` | `string \| ()` | run process, return stdout (version probing) |
| `env(key)` | `string \| ()` | read environment variable |
| `add_flags(role, flags)` | — | append flags to a role (inside `on_load`) |
| `set_toolset(role, bin)` | — | override binary at load time |

Sandbox: no filesystem write, no spawning arbitrary processes (only `find_tool`
and `run` with an allowlisted binary set), no network, step-limited engine.

---

## Built-in scripts (shipped with freight)

Built-in scripts are embedded at compile time with `include_str!` so freight
works without access to the toolchains directory.

### `toolchains/gcc.rhai`

```rhai
set_name("gcc");
set_homepage("https://gcc.gnu.org/");

set_toolset("cc",    "gcc");
set_toolset("cxx",   "g++");
set_toolset("ld",    "g++");
set_toolset("ar",    "ar");
set_toolset("strip", "strip");

set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_extensions([".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c", ".s", ".S"]);

set_flag("opt", "0", "-O0");  set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");  set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-Os");  set_flag("opt", "z", "-Oz");
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
set_flag("sanitize", "template", "-fsanitize={values}");
set_flag("cpu_ext",  "template", "-m{name}");

set_standard("c11", "-std=c11");  set_standard("c17", "-std=c17");
set_standard("c23", "-std=c23");
set_standard("c++17", "-std=c++17");  set_standard("c++20", "-std=c++20");
set_standard("c++23", "-std=c++23");

set_structure("include_dir",   "-I{path}");
set_structure("define",        "-D{name}");
set_structure("define_value",  "-D{name}={value}");
set_structure("output_obj",    "-o {path}");
set_structure("output_bin",    "-o {path}");
set_structure("compile_only",  "-c");
set_structure("dep_file",      "-MMD -MF {path}");
set_structure("dep_file_mode", "file");
set_structure("system_lib",    "-l{name}");
set_structure("target",        "");           // cross via dedicated binary
set_structure("sysroot",       "--sysroot={path}");

set_module_style("gcc", #{
    enable_flag:   "-fmodules-ts",
    compile_miu:   "-fmodule-output={pcm_path}",
    import_module: "-fmodule-file={name}={pcm_path}",
    header_unit:   "-fmodule-header",
});

set_linking("cpp", #{ abi: "c++", compatible: ["c", "fortran", "asm"],
                      extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"] });
set_linking("c",   #{ abi: "c",   compatible: ["fortran", "asm"],
                      compile_binary: "gcc", extensions: [".c", ".s", ".S"] });

on_check(|| { find_tool("g++") != () });

on_load(|| {
    if arch == "x86_64" || arch == "x64" {
        add_flags("cc",  ["-m64"]);
        add_flags("cxx", ["-m64"]);
        add_flags("ld",  ["-m64"]);
    } else if arch == "i386" || arch == "x86" {
        add_flags("cc",  ["-m32"]);
        add_flags("cxx", ["-m32"]);
        add_flags("ld",  ["-m32"]);
    }
});
```

### `toolchains/msvc.rhai` (fully expressible, no Rust backend needed)

```rhai
set_name("msvc");
set_homepage("https://visualstudio.microsoft.com/");

set_toolset("cc",    "cl.exe");
set_toolset("cxx",   "cl.exe");
set_toolset("ld",    "link.exe");
set_toolset("ar",    "lib.exe");
set_toolset("strip", "");

set_version_arg("");
set_version_regex("Version (\\d+\\.\\d+\\.\\d+\\.\\d+)");

set_extensions([".cpp", ".cc", ".cxx", ".c++", ".c"]);

set_flag("opt", "0", "/Od");  set_flag("opt", "1", "/O1");
set_flag("opt", "2", "/O2");  set_flag("opt", "3", "/Ox");
set_flag("debug",    "true",  "/Zi /FS");
set_flag("debug",    "false", "");
set_flag("warnings", "none",    "/W0");
set_flag("warnings", "default", "/W3");
set_flag("warnings", "all",     "/W4");
set_flag("warnings", "error",   "/W4 /WX");
set_flag("lto",      "true",  "/GL");      // compile side
set_flag("lto",      "false", "");
set_flag("lto_link", "true",  "/LTCG");    // link side
set_flag("lto_link", "false", "");
set_flag("strip",    "true",  "");         // no strip flag; just omit PDB
set_flag("strip",    "false", "");
set_flag("sanitize", "template", "/fsanitize={values}");
set_flag("cpu_ext",  "template", "");      // /arch:AVX2 etc set via compiler.flags

set_standard("c++17", "/std:c++17");
set_standard("c++20", "/std:c++20");
set_standard("c++23", "/std:c++latest");
set_standard("c17",   "/std:c17");

set_structure("include_dir",   "/I{path}");
set_structure("define",        "/D{name}");
set_structure("define_value",  "/D{name}={value}");
set_structure("output_obj",    "/Fo{path}");
set_structure("output_bin",    "/Fe{path}");
set_structure("compile_only",  "/c");
set_structure("dep_file",      "/showIncludes");
set_structure("dep_file_mode", "stdout");
set_structure("system_lib",    "{name}.lib");
set_structure("target",        "");
set_structure("sysroot",       "");

set_module_style("none");

set_linking("cpp", #{ abi: "c++", compatible: ["c"],
                      extensions: [".cpp", ".cc", ".cxx", ".c++"] });
set_linking("c",   #{ abi: "c",   compatible: [],
                      extensions: [".c"] });

on_check(|| { find_tool("cl.exe") != () });

on_load(|| {
    // Detect vcvars environment — if INCLUDE is not set, cl.exe won't find headers.
    if env("INCLUDE") == () {
        // Could call a helper here, or just warn; exact approach TBD.
    }
});
```

---

## Changes to the Rust codebase

### New: `crates/freight-core/src/toolchain/engine.rs`

- Owns the `rhai::Engine` instance.
- Registers all `set_*` / `on_check` / `on_load` / `find_tool` / `run` / `env`
  functions as Rhai native functions.
- Executes a script into a `ToolchainDef` struct (same fields as the current
  `CompilerTemplate`).
- Built-in scripts embedded with `include_str!`; user scripts from
  `~/.freight/toolchains/*.rhai` layered on top (user overrides same name).

### Modified: `crates/freight-core/src/toolchain/template.rs`

- `CompilerTemplate` struct is unchanged — it remains the in-memory
  representation used by compile/link/detect.
- `CompilerTemplate::from_toml()` is **removed**.
- `CompilerTemplate::from_rhai(src: &str) -> Result<Self>` is added —
  executes the script via `engine.rs` and maps the collected `ToolchainDef`
  into `CompilerTemplate`.
- `ModuleStyle` enum, `LinkingInfo`, `BuildSettings`, `StructureFlags` are all
  unchanged.

### Modified: `crates/freight-core/src/toolchain/mod.rs`

- `load_templates(dir)` now globs `*.rhai` instead of `*.toml`.
- `load_templates_embedded()` returns compiled-in scripts.

### Modified: `build/compile.rs`

- `output_flag()` → calls `output_obj_flag()` (uses `output_obj` structure field).
- `dep_file_mode = "stdout"`: capture stdout during compilation, parse
  `Note: including file:` lines, write synthetic `.d` file — rest of
  dirty-check logic unchanged.
- Compile binary comes from `toolset.cc` / `toolset.cxx`.

### Modified: `build/link.rs`

- Link binary: `toolset.ld`.
- Archive binary: `toolset.ar` (replaces hardcoded `ar`).
- Strip binary: `toolset.strip`.
- `output_bin_flag()` uses `output_bin` structure field.
- `assemble_link_flags()` uses `lto_link` flag category when present.
- `system_lib_flag(name)` uses `system_lib` structure template.

---

## Backwards compatibility

There is none to maintain for toolchain files — they are internal to freight,
not user-facing config. The `.rhai` scripts replace the `.toml` files entirely.
`freight.toml` (project manifest) is completely unaffected.

---

## Phases

| # | Work |
|---|---|
| 1 | Add `rhai` crate; implement `engine.rs` with registration of all `set_*` functions; wire `load_templates` to glob `.rhai` |
| 2 | Port existing 10 compiler templates from TOML to Rhai (mechanical translation) |
| 3 | Delete `from_toml()`, remove TOML template parsing code |
| 4 | Add `on_check` / `on_load` execution in `detect.rs` |
| 5 | Add `[toolset]` role dispatch in compile/link/ar callers |
| 6 | Add `output_obj`/`output_bin`, `lto_link`, `system_lib`, `dep_file_mode = "stdout"` |
| 7 | Write `msvc.rhai` using the new capabilities |
| 8 | Update `freight toolchain add <path>` to accept `.rhai` files |
| 9 | Update `docs/compiler-templates.md` → `docs/toolchain-scripts.md` |

Phases 1–3 are a straight mechanical port with no user-visible change.
Phases 4–6 add the capabilities that make MSVC and arch-conditional toolchains
possible. Phases 7–9 are payoff.
