# Requirements Handling

Freight should validate that every compiler a project needs is actually present
before starting any compilation. Currently it discovers missing tools mid-build,
which produces confusing errors deep in the build pipeline.

---

## Design

`[language.<key>]` declarations are the requirement. Declaring `[language.cuda]`
already means "this project needs a CUDA compiler." No new manifest section is
needed — the fix is enforcing that declaration upfront.

### Per-option callbacks via `add_compiler_option`

Each option a template wants to support is registered with its own anonymous
callback. The Rust side exposes `add_compiler_option(name, callback)` as a Rhai
function during template evaluation. When freight validates a project, it looks
up which options the manifest declares under `[compiler.<name>]` or
`[language.<key>]`, finds the registered callback for each one, and calls it
with a `ctx` object carrying relevant information from the manifest and the
detected compiler.

```rhai
add_compiler_option("min_version", |ctx| {
    if ctx.version < ctx.value {
        return "clang " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});
```

This keeps each option self-contained. The Rust binary never interprets option
names or values — it just dispatches to whatever the template registered.
Templates without registered options pass validation unconditionally.

### `ctx` fields

| Field | Type | Description |
|---|---|---|
| `ctx.value` | string | The value from the manifest for this option |
| `ctx.version` | string | Detected compiler version string |
| `ctx.arch` | string | Effective target architecture (e.g. `"x86_64"`) |
| `ctx.os` | string | Effective target OS (e.g. `"linux"`) |
| `ctx.name` | string | Template name (e.g. `"clang"`) |

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

### 2. Add `arch` field to `LanguageSettings` — `manifest/types.rs`

```rust
pub struct LanguageSettings {
    pub std:    Option<String>,
    pub stdlib: Option<String>,
    pub arch:   Option<String>,   // new: e.g. "x86_64", "aarch64"
}
```

**Example manifest usage:**
```toml
[language.asm]
arch = "x86_64"
```

---

### 3. Compiler constraints manifest type — `manifest/types.rs`

```rust
/// Free-form key/value options declared under `[compiler.<name>]`.
/// Each key is dispatched to the callback registered via add_compiler_option().
/// Keys and their meaning are defined by each template, not by freight.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CompilerConstraints(pub HashMap<String, String>);
```

Add to `Manifest`:

```rust
#[serde(default)]
pub compiler: HashMap<String, CompilerConstraints>,
```

---

### 4. `add_compiler_option` Rhai API — `toolchain/template.rs`

During template evaluation, freight registers `add_compiler_option` as a Rhai
function. Internally it stores a map of `option_name -> FnPtr` on the template.

```rust
// Stored on CompilerTemplate after evaluation:
pub option_handlers: HashMap<String, rhai::FnPtr>,
```

The Rust registration (pseudocode):

```rust
engine.register_fn("add_compiler_option", move |name: String, handler: FnPtr| {
    option_handlers.insert(name, handler);
});
```

---

### 5. Pre-build validation — `build/mod.rs`

After `detect_all_cached()` and before `discover()`:

1. For every `[language.<key>]` in the manifest, verify `select_compiler()`
   returns something. If not, error immediately.
2. For each declared option in the manifest, look up the template's registered
   callback and call it with a `ctx` built from the manifest + detected info.

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

        let settings = manifest.effective_language_settings(lang_key);
        let options = settings.to_option_map(); // e.g. {"arch": "x86_64"}
        dc.template.run_option_handlers(&options, &dc.version, effective_arch, effective_os)?;
    }

    for (name, constraints) in &manifest.compiler {
        let Some(dc) = detected.iter().find(|d| d.template.name == *name) else {
            continue; // absent — only an error if also required by a language above
        };
        dc.template.run_option_handlers(&constraints.0, &dc.version, effective_arch, effective_os)?;
    }

    Ok(())
}
```

`run_option_handlers` iterates the provided option map, looks up each key in
`template.option_handlers`, builds the `ctx` map, and calls the `FnPtr`. Returns
the first non-empty string as a `FreightError`. Unknown option keys (no handler
registered) are silently ignored.

---

### 6. Per-option callbacks in Rhai templates

**`nasm.rhai` / `yasm.rhai`:**

```rhai
add_compiler_option("arch", |ctx| {
    if ctx.arch != ctx.value {
        return "assembler requires arch '" + ctx.value +
               "' but the effective target is '" + ctx.arch + "'";
    }
    ""
});
```

**`nvcc.rhai`:**

```rhai
add_compiler_option("min_version", |ctx| {
    if ctx.version < ctx.value {
        return "nvcc " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});
```

**`clang.rhai` / `gcc.rhai`:**

```rhai
add_compiler_option("min_version", |ctx| {
    if ctx.version < ctx.value {
        return ctx.name + " " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});

add_compiler_option("max_version", |ctx| {
    if ctx.version > ctx.value {
        return ctx.name + " " + ctx.version + " exceeds required maximum " + ctx.value;
    }
    ""
});
```

---

## Manifest syntax

```toml
# CUDA project — freight errors immediately if nvcc is not on PATH
[language.cuda]

# x86-64 assembly — errors if no asm compiler found;
# "arch" dispatched to nasm/yasm's add_compiler_option("arch", ...) callback
[language.asm]
arch = "x86_64"

# Fortran — errors if gfortran / flang not found
[language.fortran]
std = "f2018"

# Compiler option constraints — each key dispatched to its registered callback
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

Error strings are returned by the callback, so each template controls the
wording. Examples with the implementations above:

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
| `crates/freight-core/src/manifest/types.rs` | Add `arch` to `LanguageSettings`; add `CompilerConstraints`; add `compiler` map to `Manifest` |
| `crates/freight-core/src/toolchain/template.rs` | Register `add_compiler_option` Rhai function; store `option_handlers` map; add `run_option_handlers()` |
| `crates/freight-core/src/build/mod.rs` | Add `check_compiler_requirements()`, call before `discover()` |
| `toolchains/nasm.rhai`, `toolchains/yasm.rhai` | Register `"arch"` option callback |
| `toolchains/nvidia/nvcc.rhai` | Register `"min_version"` option callback |
| `toolchains/llvm/clang.rhai`, `toolchains/gnu/gcc.rhai` | Register `"min_version"` and `"max_version"` option callbacks |
| `docs/manifest-reference.md` | Document `arch` under `[language.*]`; document `[compiler.*]` section |
