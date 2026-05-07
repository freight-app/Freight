# Requirements Handling

This document covers how freight validates compiler availability and routes
per-compiler options from the manifest into compiler invocations — without
hardcoding any compiler knowledge in Rust.

**Implemented:** language auto-detection, requirement check after discovery.
**Planned:** `compiler_option` / `language_option` callback system described below.

---

## Implemented: language detection and requirement check

Languages are detected automatically from the source files in `src/`. A `.cu`
file activates the `cuda` language key; a `.asm` file activates `asm`; and so
on — no manifest declaration needed.

`[language.<key>]` sections are optional configuration. They are applied only
when that language is actually present in the source tree. If no source files
for a declared language exist, the section is silently ignored. There is no
error for a `[language.cuda]` block when no `.cu` files are found.

The compiler requirement check runs *after* source discovery, over the set of
languages that were actually found. If freight discovers `.cu` files but no
CUDA compiler is on `PATH`, that is the error — not a missing manifest
declaration.

---

## Planned: per-option callbacks

Options that every compiler shares (opt level, warnings, LTO, debug, sanitizers,
standard) are handled by the existing template maps (`opt`, `dbg`, `warnings`, `lto`,
`standards`, etc.) and are not part of this system.

`compiler_option` and `language_option` are exclusively for **compiler-specific
options** — things that only make sense for one particular tool and have no
universal equivalent. Examples:

- `nvcc`: GPU target architecture (`sm_arch = "sm_89"`)
- `nasm`/`yasm`: output format override (`output_format = "elf64"`)
- `msvc`: runtime library selection (`msvc_runtime = "MT"`)
- `clang++`: ThinLTO cache path (`thinlto_cache = "/tmp/thinlto"`)

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
  conditionally add flags only for `"cuda"` but not `"cpp"`, for example.

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

Lines 121–132 silently inject assembly language keys even when no `.asm` or
`.nasm` files are present. Remove this block. Assembly activates the same way
as every other language: a source file with a matching extension is found during
`walk_sources()`. No special case needed.

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

Add `arch` to `LanguageSettings` and capture unknown keys via `#[serde(flatten)]`
so compiler-specific options (e.g. `output_format` for nasm) aren't rejected by
serde and are available for `language_option` dispatch:

```rust
pub struct LanguageSettings {
    pub std:    Option<String>,
    pub stdlib: Option<String>,
    pub arch:   Option<String>,
    /// Compiler-specific options not covered by the typed fields above.
    /// Captured via flatten so unknown TOML keys are accepted rather than rejected.
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
}
```

`LanguageSettings::to_option_map()` merges the typed fields into the `extra` map
and returns the combined `HashMap<String, String>` for dispatch:

```rust
pub fn to_option_map(&self) -> HashMap<String, String> {
    let mut m = self.extra.clone();
    if let Some(v) = &self.arch   { m.insert("arch".into(),   v.clone()); }
    if let Some(v) = &self.std    { m.insert("std".into(),    v.clone()); }
    if let Some(v) = &self.stdlib { m.insert("stdlib".into(), v.clone()); }
    m
}
```

Add free-form options for `[compiler.<name>]` sections:

```rust
/// Options under [compiler.<name>] — dispatched to compiler_option() callbacks.
/// All keys are free-form; meaning is defined by each template.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CompilerOptions(pub HashMap<String, String>);
```

Add to `Manifest`:

```rust
#[serde(default)]
pub compiler: HashMap<String, CompilerOptions>,
```

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

Version comparison helpers are also registered so templates don't have to parse
version strings manually. String comparison (`<`, `>`) is lexicographic and gives
wrong results for versions like `"9.0"` vs `"14.0"`.

```rust
engine.register_fn("semver_gte", |a: &str, b: &str| -> bool { /* parse + compare */ });
engine.register_fn("semver_lte", |a: &str, b: &str| -> bool { /* parse + compare */ });
engine.register_fn("semver_gt",  |a: &str, b: &str| -> bool { /* parse + compare */ });
engine.register_fn("semver_lt",  |a: &str, b: &str| -> bool { /* parse + compare */ });
```

These use the `semver` crate already in the workspace. Malformed version strings
fall back to lexicographic comparison so templates don't crash on unusual outputs.

`run_option_handlers(handlers, options, ctx_base)` iterates the option map, looks
up each key in `handlers`, builds the `ctx` dynamic map, calls the `FnPtr`,
collects any flags added via `ctx.add_flag`, and returns the first non-empty error
string alongside the accumulated extra flags:

```rust
pub fn run_option_handlers(
    &self,
    handlers: &HashMap<String, rhai::FnPtr>,
    options: &HashMap<String, String>,
    version: &str,
    arch: &str,
    os: &str,
    lang_key: &str,
) -> Result<Vec<String>, FreightError> {
    let mut extra_flags: Vec<String> = Vec::new();
    for (key, value) in options {
        let Some(handler) = handlers.get(key) else { continue };
        // build ctx, call handler, collect flags / surface error ...
        extra_flags.extend(ctx.drain_flags());
    }
    Ok(extra_flags)
}
```

The caller merges the returned flags into the per-language compiler invocation.

---

### 4. Pre-build validation — `build/mod.rs`

Runs after `discover()`, over the set of languages actually found in source
files. `[language.*]` config from the manifest is applied only for languages
present in `discovered_langs`; sections for absent languages are skipped.

Returns a map of extra flags per language key so the build engine can merge them
into compiler invocations:

```rust
fn check_compiler_requirements(
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    discovered_langs: &[String],
    effective_arch: &str,
    effective_os: &str,
) -> Result<HashMap<String, Vec<String>>, FreightError> {
    let mut extra: HashMap<String, Vec<String>> = HashMap::new();

    for lang_key in discovered_langs {
        let Some(dc) = select_compiler(lang_key, backend, detected, None) else {
            return Err(FreightError::CompilerNotFound(format!(
                "found source files for language '{lang_key}' but no compiler is on PATH"
            )));
        };

        // Apply [language.<key>] config only if the section exists.
        if let Some(settings) = manifest.language.get(lang_key) {
            let options = settings.to_option_map();
            let flags = dc.template.run_option_handlers(
                &dc.template.language_option_handlers,
                &options, &dc.version, effective_arch, effective_os, lang_key,
            )?;
            extra.entry(lang_key.clone()).or_default().extend(flags);
        }
    }

    for (name, options) in &manifest.compiler {
        let Some(dc) = detected.iter().find(|d| d.template.name == *name) else {
            continue;
        };
        // Run once per active language this compiler handles; discard flags if
        // compiler is not the active backend.
        for lang_key in discovered_langs {
            if select_compiler(lang_key, backend, detected, None)
                .map(|d| d.template.name == *name)
                .unwrap_or(false)
            {
                let flags = dc.template.run_option_handlers(
                    &dc.template.compiler_option_handlers,
                    &options.0, &dc.version, effective_arch, effective_os, lang_key,
                )?;
                extra.entry(lang_key.clone()).or_default().extend(flags);
            } else {
                // Detected but not active — run for validation only, discard flags.
                dc.template.run_option_handlers(
                    &dc.template.compiler_option_handlers,
                    &options.0, &dc.version, effective_arch, effective_os, lang_key,
                )?;
            }
        }
    }

    Ok(extra)
}
```

The returned map is threaded into `settings_for_lang` so extra flags are appended
to each language's compiler invocation.

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
    if !semver_gte(ctx.version, ctx.value) {
        return "nvcc " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});
```

**`clang++.rhai` / `g++.rhai`** — version constraints via `compiler_option`:

```rhai
compiler_option("min_version", |ctx| {
    if !semver_gte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " is below required minimum " + ctx.value;
    }
    ""
});

compiler_option("max_version", |ctx| {
    if !semver_lte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " exceeds required maximum " + ctx.value;
    }
    ""
});
```

**`nvcc.rhai`** — GPU target architecture via `compiler_option`:

```rhai
compiler_option("sm_arch", |ctx| {
    ctx.add_flag("--gpu-architecture=" + ctx.value);
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

# Optional: configure assembly output format (nasm/yasm-specific option).
# Only applied if .asm/.nasm files are present; ignored otherwise.
[language.asm]
arch = "x86_64"

# Optional: configure Fortran standard.
[language.fortran]
std = "f2018"

# Compiler-specific options — applied regardless of which language is active,
# dispatched to compiler_option() callbacks.
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
| `[language.asm]` `arch = "x86_64"`, target is `aarch64-linux-gnu` | `assembler requires arch 'x86_64' but the effective target is 'aarch64-linux-gnu'` |
| `[compiler.clang++]` `min_version = "14.0"`, clang++ 13.0.1 detected | `clang++ 13.0.1 is below required minimum 14.0` |
| `[compiler.g++]` `max_version = "14.0"`, g++ 14.1.0 detected | `g++ 14.1.0 exceeds required maximum 14.0` |

---

## Files to touch (planned)

| File | Change |
|---|---|
| `crates/freight-core/src/manifest/types.rs` | Add `CompilerOptions`; add `compiler` map to `Manifest` |
| `crates/freight-core/src/toolchain/template.rs` | Register `compiler_option` and `language_option` Rhai functions; store handler maps; add `run_option_handlers()` |
| `crates/freight-core/src/build/mod.rs` | Add `check_compiler_requirements()`, call after `discover()` |
| `toolchains/asm/nasm.rhai`, `toolchains/asm/yasm.rhai` | Register `language_option("arch", ...)` |
| `toolchains/nvidia/nvcc.rhai` | Register `compiler_option("min_version", ...)` and `compiler_option("sm_arch", ...)` |
| `toolchains/llvm/clang++.rhai`, `toolchains/gnu/g++.rhai` | Register `compiler_option("min_version", ...)` and `compiler_option("max_version", ...)` |
