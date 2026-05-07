# Requirements Handling

This document covers how freight validates compiler availability and routes
per-compiler options from the manifest into compiler invocations — without
hardcoding any compiler knowledge in Rust.

---

## Language detection and requirement check

Languages are detected automatically from the source files in `src/`. A `.cu`
file activates the `cuda` language key; a `.asm` file activates `asm`; and
so on — no manifest declaration needed.

`[language.<key>]` sections are optional configuration. They are applied only
when that language is actually present in the source tree. If no source files
for a declared language exist, the section is silently ignored. There is no
error for a `[language.cuda]` block when no `.cu` files are found.

The compiler requirement check runs *after* source discovery, over the set of
languages that were actually found. If freight discovers `.cu` files but no
CUDA compiler is on `PATH`, that is the error — not a missing manifest
declaration.

---

## Per-option callbacks

Options that every compiler shares (opt level, warnings, LTO, debug, sanitizers,
standard) are handled by the existing template maps (`opt`, `dbg`, `warnings`, `lto`,
`standards`, etc.) and are not part of this system.

`compiler_option` and `language_option` are exclusively for **compiler-specific
options** — things that only make sense for one particular tool and have no
universal equivalent. Examples:

- `nvcc`: GPU compute architecture (`sm_arch = "sm_89"`)
- `nasm`/`yasm`: arch validation (`arch = "x86_64"`)
- `clang++`/`g++`: version constraints (`min_version`, `max_version`)
- `clang++`: ThinLTO mode (`lto_mode = "thin"`)

Two registration functions are available inside `.rhai` templates:

| Function | Reads from | Typical use |
|---|---|---|
| `compiler_option("key", \|ctx\| { })` | `[compiler.<name>]` in manifest | compiler-specific flags, version constraints |
| `language_option("key", \|ctx\| { })` | `[language.<key>]` in manifest | compiler-specific per-language flags, arch checks |

When freight evaluates a project, it collects every option declared in the
manifest for the active languages, looks up the registered callback for each
key, and calls it with a `ctx` object. The callback can validate, inject extra
compiler flags, or both. Unknown keys (no callback registered) are silently
ignored — forwards compatible by default.

The Rust binary never interprets option names or values itself. It only
dispatches and surfaces errors.

### `ctx` fields

| Field | Type | Description |
|---|---|---|
| `ctx.value` | string | The value from the manifest for this option |
| `ctx.version` | string | Detected compiler version string |
| `ctx.arch` | string | Effective target architecture (e.g. `"x86_64"`) |
| `ctx.os` | string | Effective target OS (e.g. `"linux"`) |
| `ctx.name` | string | Template name (e.g. `"clang++"`) |

The callback returns `""` on success (no error, no extra flags) or a non-empty
string as an error message. To inject extra compiler flags, the callback calls
the global `add_flag(s)` function as a side effect.

### Flag scope

Flags added via `add_flag` are scoped to the language that triggered the
callback:

- `language_option` callbacks always target a specific language — flags apply
  to all source files of that language.
- `compiler_option` callbacks are triggered by the `[compiler.<name>]` section
  and their flags apply globally (to all languages the compiler handles).

### `compiler_option` on non-active compilers

A `[compiler.<name>]` section in the manifest applies even when that compiler is
not the active backend — the intent is to enforce a constraint on any detected
instance of that tool. If the named compiler is not detected at all, the callbacks
are skipped silently (not an error). If it is detected but not active, the
callbacks still run for validation; `add_flag` calls in that case are discarded
since the compiler isn't producing any output.

---

## Implementation

### Manifest types — `manifest/types.rs`

`LanguageSettings` captures unknown keys via `#[serde(flatten)]` so
compiler-specific options (e.g. `output_format` for nasm) aren't rejected by
serde and are available for `language_option` dispatch:

```rust
pub struct LanguageSettings {
    pub std:    Option<String>,
    pub stdlib: Option<String>,
    pub arch:   Option<String>,
    /// Compiler-specific options not covered by the typed fields above.
    #[serde(flatten, default)]
    pub extra: HashMap<String, String>,
    /// Flags injected by language_option handlers at build time.
    #[serde(skip)]
    pub injected_flags: Vec<String>,
}
```

Free-form options for `[compiler.<name>]` sections:

```rust
pub struct CompilerToolOptions {
    #[serde(flatten)]
    pub options: HashMap<String, String>,
}

// In CompilerConfig:
#[serde(flatten, default)]
pub per_tool: HashMap<String, CompilerToolOptions>,
```

### Option handler storage — `toolchain/template.rs`

Handlers are stored on the template as `FnPtr` values alongside an `Arc<Engine>`
and `AST` so they can be called at build time:

```rust
pub struct CompilerTemplate {
    // ... existing fields ...
    pub handler_engine: Option<Arc<Engine>>,
    pub handler_ast: Option<AST>,
    pub compiler_option_handlers: HashMap<String, FnPtr>,
    pub language_option_handlers: HashMap<String, FnPtr>,
}
```

Version comparison helpers registered in the engine:

```rust
engine.register_fn("version_gte", |a: &str, b: &str| -> bool { /* ... */ });
engine.register_fn("version_lte", |a: &str, b: &str| -> bool { /* ... */ });
engine.register_fn("version_gt",  |a: &str, b: &str| -> bool { /* ... */ });
engine.register_fn("version_lt",  |a: &str, b: &str| -> bool { /* ... */ });
```

These compare version strings component-by-component (splitting on `.`),
ignoring any `-suffix`. Malformed strings fall back to lexicographic comparison.

### Pre-build validation — `build/mod.rs`

`inject_option_handler_flags` runs after project loading, before compilation:

- For each `[language.<key>]` section with extra options: runs `language_option`
  handlers on the active compiler for that language.
- For each `[compiler.<name>]` section: runs `compiler_option` handlers on the
  detected compiler. Flags are only applied if the compiler is the active backend
  for at least one discovered language; otherwise handlers run for validation only.

---

## Callbacks in Rhai templates

**`nasm.rhai` / `yasm.rhai`** — arch validation via `language_option`:

```rhai
language_option("arch", |ctx| {
    if ctx.arch != ctx.value {
        return "assembler requires arch '" + ctx.value +
               "' but the effective target is '" + ctx.arch + "'";
    }
    ""
});
```

**`nvcc.rhai`** — GPU target architecture via `compiler_option`:

```rhai
compiler_option("sm_arch", |ctx| {
    add_flag("--gpu-architecture=" + ctx.value);
    ""
});

compiler_option("min_version", |ctx| {
    if !version_gte(ctx.version, ctx.value) {
        return "nvcc " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});
```

**`clang++.rhai` / `g++.rhai`** — version constraints via `compiler_option`:

```rhai
compiler_option("min_version", |ctx| {
    if !version_gte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});

compiler_option("max_version", |ctx| {
    if !version_lte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " exceeds required maximum " + ctx.value;
    }
    ""
});
```

---

## Manifest syntax

```toml
# A project with C++ and CUDA sources needs no [language.*] at all —
# freight detects them automatically from file extensions.

# Optional: configure the C++ standard used for .cpp files.
[language.cpp]
std = "c++20"

# Optional: configure assembly arch validation (nasm/yasm-specific).
# Only applied if .asm files are present; ignored otherwise.
[language.asm]
arch = "x86_64"

# Optional: configure Fortran standard.
[language.fortran]
std = "f2018"

# Compiler-specific options — dispatched to compiler_option() callbacks.
[compiler.clang++]
min_version = "14.0"

[compiler.nvcc]
min_version = "11.8"
sm_arch     = "sm_89"

[compiler.g++]
min_version = "12.0"
max_version = "14.0"
```

---

## Error messages

| Situation | Message |
|---|---|
| `.cu` files found, nvcc not on PATH | `found source files for language 'cuda' but no compiler is on PATH` |
| `.asm` files found, no asm template installed | `found source files for language 'asm' but no compiler is on PATH` |
| `[language.asm]` `arch = "x86_64"`, target is `aarch64-linux-gnu` | `assembler requires arch 'x86_64' but the effective target is 'aarch64'` |
| `[compiler.clang++]` `min_version = "14.0"`, clang++ 13.0.1 detected | `clang++ 13.0.1 is below required minimum 14.0` |
| `[compiler.g++]` `max_version = "14.0"`, g++ 14.1.0 detected | `g++ 14.1.0 exceeds required maximum 14.0` |
