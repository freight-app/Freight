# Requirements Handling

Freight should validate that every compiler a project needs is actually present
before starting any compilation, and allow templates to handle arbitrary
per-option behaviour (extra flags, validation, configuration) driven entirely by
the manifest — without touching the Rust binary.

---

## Design

`[language.<key>]` declarations are the requirement. Declaring `[language.cuda]`
already means "this project needs a CUDA compiler." No new manifest section is
needed — the fix is enforcing that declaration upfront.

### Per-option callbacks

Two registration functions are available inside `.rhai` templates:

| Function | Reads from | Typical use |
|---|---|---|
| `compiler_option("key", \|ctx\| { })` | `[compiler.<name>]` in manifest | version constraints, toolchain-wide flags |
| `language_option("key", \|ctx\| { })` | `[language.<key>]` in manifest | arch checks, extra per-language flags, std overrides |

When freight evaluates a project, it collects every option declared in the
manifest, looks up the registered callback for each key, and calls it with a
`ctx` object. The callback can validate, inject extra compiler flags, or both.
Unknown keys (no callback registered) are silently ignored — forwards
compatible by default.

The Rust binary never interprets option names or values itself. It only
dispatches and surfaces errors.

### `ctx` fields

| Field | Type | Description |
|---|---|---|
| `ctx.value` | string | The value from the manifest for this option |
| `ctx.version` | string | Detected compiler version string |
| `ctx.arch` | string | Effective target architecture (e.g. `"x86_64"`) |
| `ctx.os` | string | Effective target OS (e.g. `"linux"`) |
| `ctx.name` | string | Template name (e.g. `"clang"`) |
| `ctx.lang_key` | string | Language being compiled (e.g. `"cpp"`, `"c"`). Set for `language_option` callbacks; also set for `compiler_option` callbacks when triggered by a specific language. |

The callback returns `""` on success (no error, no extra flags) or a non-empty
string as an error message. To inject extra compiler flags the callback calls
`ctx.add_flag(s)` as a side effect.

### Flag scope

Flags added via `ctx.add_flag` are scoped to the language that triggered the
callback:

- `language_option` callbacks always have a specific `lang_key` — flags apply
  to all source files of that language.
- `compiler_option` callbacks are called once per active language the compiler
  handles. `ctx.lang_key` identifies which language, so a callback can
  conditionally add flags only for `"cpp"` but not `"c"`, for example.

### `fn load()` and `compiler_option` / `language_option`

`fn load()` (currently used in `gcc.rhai`) runs once after detection and injects
flags unconditionally based on the host environment. It is **not** superseded —
it handles machine-level defaults that require no manifest input (e.g. `-m64` on
x86_64). `compiler_option` and `language_option` callbacks run later and are
driven by what the manifest declares. Both coexist; `fn load()` fires first.

### `compiler_option` on non-active compilers

A `[compiler.<name>]` section in the manifest applies even when that compiler is
not the active backend — the intent is to enforce a constraint on any detected
instance of that tool. If the named compiler is not detected at all, the callbacks
are skipped silently (not an error). If it is detected but not active, the
callbacks still run for validation; `ctx.add_flag` calls in that case are
discarded since the compiler isn't producing any output.

---

## Changes

### 1. Remove the asm always-active special case — `build/discover.rs`

Lines 121–132 silently inject assembly language keys even when `[language.asm]`
is not declared. Remove this block. Assembly must be declared explicitly, like
every other language.

**Before:**
```rust
// Assembly language keys are always active when their template is installed —
// users should not need to declare [language.asm] just to include .asm files.
const ASM_KEYS: &[&str] = &["asm"];
for &lang_key in ASM_KEYS {
    for template in templates {
        if let Some(linking) = template.linking.get(lang_key) {
            for ext in &linking.extensions {
                ext_map.entry(ext.clone()).or_insert_with(|| lang_key.to_string());
            }
        }
    }
}
```

**After:** block deleted entirely.

---

### 2. Manifest types — `manifest/types.rs`

Add `arch` to `LanguageSettings` (feeds `language_option` callbacks):

```rust
pub struct LanguageSettings {
    pub std:    Option<String>,
    pub stdlib: Option<String>,
    pub arch:   Option<String>,   // e.g. "x86_64", "aarch64"
}
```

Add free-form option maps for both sections:

```rust
/// Options under [compiler.<name>] — dispatched to compiler_option() callbacks.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CompilerOptions(pub HashMap<String, String>);

/// Options under [language.<key>] beyond the typed fields are collected here
/// and dispatched to language_option() callbacks.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LanguageOptions(pub HashMap<String, String>);
```

Add to `Manifest`:

```rust
#[serde(default)]
pub compiler: HashMap<String, CompilerOptions>,
```

`LanguageSettings::to_option_map()` converts the typed fields (`arch`, `std`,
`stdlib`) plus any unrecognised keys into a `HashMap<String, String>` for
dispatch.

---

### 3. Option handler storage — `toolchain/template.rs`

During template evaluation, freight registers both Rhai functions. Internally
the handlers are stored on the template:

```rust
pub struct CompilerTemplate {
    // ... existing fields ...
    pub compiler_option_handlers: HashMap<String, rhai::FnPtr>,
    pub language_option_handlers: HashMap<String, rhai::FnPtr>,
}
```

Registration (pseudocode):

```rust
engine.register_fn("compiler_option", |name: String, handler: FnPtr| {
    compiler_option_handlers.insert(name, handler);
});
engine.register_fn("language_option", |name: String, handler: FnPtr| {
    language_option_handlers.insert(name, handler);
});
```

`run_option_handlers(handlers, options, ctx)` iterates the option map, looks up
each key in `handlers`, builds the `ctx` dynamic map, calls the `FnPtr`, collects
`ctx.extra_flags`, and returns the first non-empty error string.

---

### 4. Pre-build validation — `build/mod.rs`

After `detect_all_cached()` and before `discover()`:

```rust
fn check_compiler_requirements(
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    effective_arch: &str,
    effective_os: &str,
) -> Result<(), FreightError> {
    for lang_key in manifest.language.keys() {
        let Some(dc) = select_compiler(lang_key, backend, detected, None) else {
            return Err(FreightError::CompilerNotFound(format!(
                "no compiler found for language '{lang_key}' \
                 — install the required tool and ensure it is on PATH"
            )));
        };

        let options = manifest.effective_language_settings(lang_key).to_option_map();
        dc.template.run_option_handlers(
            &dc.template.language_option_handlers,
            &options, &dc.version, effective_arch, effective_os,
        )?;
    }

    for (name, options) in &manifest.compiler {
        let Some(dc) = detected.iter().find(|d| d.template.name == *name) else {
            continue;
        };
        dc.template.run_option_handlers(
            &dc.template.compiler_option_handlers,
            &options.0, &dc.version, effective_arch, effective_os,
        )?;
    }

    Ok(())
}
```

---

### 5. Callbacks in Rhai templates

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

**`nvcc.rhai`** — version validation via `compiler_option`:

```rhai
compiler_option("min_version", |ctx| {
    if ctx.version < ctx.value {
        return "nvcc " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});
```

**`clang.rhai` / `gcc.rhai`** — version validation and extra flags:

```rhai
compiler_option("min_version", |ctx| {
    if ctx.version < ctx.value {
        return ctx.name + " " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});

compiler_option("max_version", |ctx| {
    if ctx.version > ctx.value {
        return ctx.name + " " + ctx.version + " exceeds required maximum " + ctx.value;
    }
    ""
});
```

**Any template** — extra flags via `language_option`:

```rhai
language_option("sanitize", |ctx| {
    ctx.add_flag("-fsanitize=" + ctx.value);
    ""
});
```

---

## Manifest syntax

```toml
# CUDA project — freight errors immediately if nvcc is not on PATH
[language.cuda]

# x86-64 assembly — "arch" dispatched to language_option("arch", ...) in nasm/yasm
[language.asm]
arch = "x86_64"

# Fortran
[language.fortran]
std = "f2018"

# Compiler-level options — dispatched to compiler_option() callbacks
[compiler.clang]
min_version = "14.0"

[compiler.nvcc]
min_version = "11.8"

[compiler.gcc]
min_version = "12.0"
max_version = "14.0"
```

---

## Error messages

| Situation | Message |
|---|---|
| `[language.cuda]` declared, nvcc not on PATH | `no compiler found for language 'cuda' — install the required tool and ensure it is on PATH` |
| `[language.asm]` declared, no asm template installed | `no compiler found for language 'asm' — install the required tool and ensure it is on PATH` |
| `[language.asm]` `arch = "x86_64"`, target is `aarch64-linux-gnu` | `assembler requires arch 'x86_64' but the effective target is 'aarch64-linux-gnu'` |
| `[compiler.clang]` `min_version = "14.0"`, clang 13.0.1 detected | `clang 13.0.1 is below required minimum 14.0` |
| `[compiler.gcc]` `max_version = "14.0"`, gcc 14.1.0 detected | `gcc 14.1.0 exceeds required maximum 14.0` |

---

## Files touched

| File | Change |
|---|---|
| `crates/freight-core/src/build/discover.rs` | Remove asm always-active block |
| `crates/freight-core/src/manifest/types.rs` | Add `arch` to `LanguageSettings`; add `CompilerOptions`, `LanguageOptions`; add `compiler` map to `Manifest` |
| `crates/freight-core/src/toolchain/template.rs` | Register `compiler_option` and `language_option` Rhai functions; store handler maps; add `run_option_handlers()` |
| `crates/freight-core/src/build/mod.rs` | Add `check_compiler_requirements()`, call before `discover()` |
| `toolchains/nasm.rhai`, `toolchains/yasm.rhai` | Register `language_option("arch", ...)` |
| `toolchains/nvidia/nvcc.rhai` | Register `compiler_option("min_version", ...)` |
| `toolchains/llvm/clang.rhai`, `toolchains/gnu/gcc.rhai` | Register `compiler_option("min_version", ...)` and `compiler_option("max_version", ...)` |
| `docs/manifest-reference.md` | Document `arch` under `[language.*]`; document `[compiler.*]` section |
