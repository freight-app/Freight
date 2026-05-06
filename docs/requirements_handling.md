# Requirements Handling

Freight should validate that every compiler a project needs is actually present
before starting any compilation. Currently it discovers missing tools mid-build,
which produces confusing errors deep in the build pipeline.

---

## Design

`[language.<key>]` declarations are the requirement. Declaring `[language.cuda]`
already means "this project needs a CUDA compiler." No new manifest section is
needed — the fix is enforcing that declaration upfront.

The one optional addition is an `arch` field on language sections for
architecture-specific tools (primarily assembly).

### Rhai callbacks, not hardcoded Rust

Validation logic lives in the `.rhai` template, not in the Rust executable.
This is consistent with the existing architecture — each template already has a
`fn check()` callback for detection-time validation. The same pattern extends to
project-level requirements via a new `fn validate(constraints)` callback.

| Callback | When called | Input | Returns |
|---|---|---|---|
| `fn check()` | During detection — is this compiler usable on this machine? | `arch`, `os`, `env`, `find_tool` globals | `bool` |
| `fn validate(constraints)` | After selection — does this compiler meet the project's requirements? | map from the manifest's `[compiler.<name>]` or `[language.<key>]` section | `""` on success, error string on failure |

This means the Rust side stays thin: call `validate(constraints)`, surface the
returned string as a `FreightError` if non-empty. The template author decides
what constraints make sense and how to evaluate them — no semver parsing or arch
comparison baked into the binary. Templates without a `validate` function pass
unconditionally (backwards compatible).

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

Used primarily for `[language.asm]` to declare which ISA the assembly targets.
Passed to the template's `fn validate(constraints)` callback; not interpreted
by Rust directly.

**Example manifest usage:**
```toml
[language.asm]
arch = "x86_64"
```

---

### 3. Add pre-build compiler validation — `build/mod.rs`

After `detect_all_cached()` and before any compilation:

1. For every `[language.<key>]` in the manifest, verify `select_compiler()`
   returns something. If not, error immediately.
2. For every selected compiler, call its `fn validate(constraints)` Rhai
   callback with the relevant constraints map from the manifest. Surface any
   non-empty return value as a `FreightError`.

```rust
fn check_compiler_requirements(
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
) -> Result<(), FreightError> {
    for lang_key in manifest.language.keys() {
        let Some(dc) = select_compiler(lang_key, backend, detected, None) else {
            return Err(FreightError::CompilerNotFound(format!(
                "no compiler found for language '{lang_key}' \
                 — install the required tool and ensure it is on PATH"
            )));
        };

        // Pass language settings (arch, std, …) to fn validate(constraints).
        let constraints = manifest.effective_language_settings(lang_key).to_constraint_map();
        dc.template.call_validate(&constraints)?;
    }

    // Compiler-level constraints from [compiler.<name>] sections.
    for (name, constraints) in &manifest.compiler {
        let Some(dc) = detected.iter().find(|d| d.template.name == *name) else {
            continue; // absent — only an error if also required by a language above
        };
        dc.template.call_validate(&constraints.as_map())?;
    }

    Ok(())
}
```

`CompilerTemplate::call_validate` reads the `validate` variable from the parsed
scope and calls it as a `FnPtr`. If the variable is absent the call is a no-op
(backwards compatible with existing templates).

Call site in `build_project`, immediately after the compiler detection block and
before `discover()`.

---

### 4. Compiler constraints manifest type — `manifest/types.rs`

```rust
/// Free-form key/value constraints for `[compiler.<name>]` sections.
/// Passed verbatim to the template's `validate` closure.
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

### 5. `validate` anonymous function in Rhai templates

Templates assign an anonymous function to `validate`. This fits the existing
style — all other template behaviour is expressed as data assignments, not named
functions. The closure receives a map of constraints from the manifest and
returns `""` on success or an error string on failure.

**`nasm.rhai` / `yasm.rhai`** — arch constraint:

```rhai
validate = |constraints| {
    if constraints["arch"] != () && constraints["arch"] != arch {
        return "assembler requires arch '" + constraints["arch"] +
               "' but the effective target is '" + arch + "'";
    }
    ""
};
```

**`nvcc.rhai`** — version constraint:

```rhai
validate = |constraints| {
    if constraints["min_version"] != () && version < constraints["min_version"] {
        return "nvcc " + version + " is below required minimum " +
               constraints["min_version"];
    }
    ""
};
```

**`clang.rhai` / `gcc.rhai`** — version constraints:

```rhai
validate = |constraints| {
    if constraints["min_version"] != () && version < constraints["min_version"] {
        return name + " " + version + " is below required minimum " +
               constraints["min_version"];
    }
    if constraints["max_version"] != () && version > constraints["max_version"] {
        return name + " " + version + " exceeds required maximum " +
               constraints["max_version"];
    }
    ""
};
```

---

## Manifest syntax

```toml
# CUDA project — freight errors immediately if nvcc is not on PATH
[language.cuda]

# x86-64 assembly — errors if no asm compiler found;
# arch constraint passed to nasm/yasm's fn validate()
[language.asm]
arch = "x86_64"

# Fortran — errors if gfortran / flang not found
[language.fortran]
std = "f2018"

# Compiler version constraints — passed to fn validate() in the named template
[compiler.clang]
min_version = "14.0"

[compiler.nvcc]
min_version = "11.8"

[compiler.gcc]
min_version = "12.0"
```

---

## Error messages

Error strings come from the template's `fn validate()` return value, so each
template can tailor the wording. Examples with the implementations above:

| Situation | Message |
|---|---|
| `[language.cuda]` declared, nvcc not on PATH | `no compiler found for language 'cuda' — install the required tool and ensure it is on PATH` |
| `[language.asm]` declared, no asm template installed | `no compiler found for language 'asm' — install the required tool and ensure it is on PATH` |
| `[language.asm]` `arch = "x86_64"`, target is `aarch64-linux-gnu` | `assembler requires arch 'x86_64' but the effective target is 'aarch64-linux-gnu'` |
| `[compiler.clang]` `min_version = "14.0"`, clang 13.0.1 detected | `clang 13.0.1 is below required minimum 14.0` |
| `[compiler.gcc]` `max_version = "13.0"`, gcc 14.1.0 detected | `gcc 14.1.0 exceeds required maximum 13.0` |

---

## Files touched

| File | Change |
|---|---|
| `crates/freight-core/src/build/discover.rs` | Remove asm always-active block |
| `crates/freight-core/src/manifest/types.rs` | Add `arch` to `LanguageSettings`; add `CompilerConstraints`; add `compiler` map to `Manifest` |
| `crates/freight-core/src/toolchain/template.rs` | Add `call_validate()` method to `CompilerTemplate` |
| `crates/freight-core/src/build/mod.rs` | Add `check_compiler_requirements()`, call before `discover()` |
| `toolchains/nasm.rhai`, `toolchains/yasm.rhai` | Add `fn validate(constraints)` — arch check |
| `toolchains/nvidia/nvcc.rhai` | Add `fn validate(constraints)` — version check |
| `toolchains/llvm/clang.rhai`, `toolchains/gnu/gcc.rhai` | Add `fn validate(constraints)` — version check |
| `docs/manifest-reference.md` | Document `arch` under `[language.*]`; document `[compiler.*]` section |
