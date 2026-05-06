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
Ignored by the compiler (the assembler already knows its ISA); used only for the
validation check below.

**Example manifest usage:**
```toml
[language.asm]
arch = "x86_64"
```

---

### 3. Add pre-build compiler validation — `build/mod.rs`

After `detect_all_cached()` and before any compilation, iterate every language
key declared in the manifest and verify a compiler can be selected for it.

```rust
fn check_compiler_requirements(
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    effective_target: Option<&str>,
) -> Result<(), FreightError> {
    for lang_key in manifest.language.keys() {
        if select_compiler(lang_key, backend, detected, None).is_none() {
            return Err(FreightError::CompilerNotFound(format!(
                "no compiler found for language '{lang_key}' \
                 — install the required tool and ensure it is on PATH"
            )));
        }

        // Architecture check for tools that declare an ISA requirement.
        if let Some(required_arch) = manifest
            .effective_language_settings(lang_key)
            .arch
            .as_deref()
        {
            if let Some(target) = effective_target {
                if !target.starts_with(required_arch) {
                    return Err(FreightError::ManifestParse(format!(
                        "language '{lang_key}' requires arch '{required_arch}' \
                         but the effective target is '{target}'"
                    )));
                }
            }
        }
    }
    Ok(())
}
```

Call site in `build_project`, immediately after the compiler detection block and
before `discover()`.

---

## Manifest syntax (no new section needed)

```toml
# CUDA project — freight errors immediately if nvcc is not on PATH
[language.cuda]

# x86-64 assembly — errors if no asm compiler found,
# and additionally if the effective target is not x86_64
[language.asm]
arch = "x86_64"

# Fortran — errors if gfortran / flang not found
[language.fortran]
std = "f2018"
```

---

## Error messages

| Situation | Message |
|---|---|
| `[language.cuda]` declared, nvcc not on PATH | `no compiler found for language 'cuda' — install the required tool and ensure it is on PATH` |
| `[language.asm]` declared, no asm template installed | `no compiler found for language 'asm' — install the required tool and ensure it is on PATH` |
| `[language.asm]` with `arch = "x86_64"`, target is `aarch64-linux-gnu` | `language 'asm' requires arch 'x86_64' but the effective target is 'aarch64-linux-gnu'` |

---

## Files touched

| File | Change |
|---|---|
| `crates/freight-core/src/build/discover.rs` | Remove asm always-active block |
| `crates/freight-core/src/manifest/types.rs` | Add `arch: Option<String>` to `LanguageSettings` |
| `crates/freight-core/src/build/mod.rs` | Add `check_compiler_requirements()`, call before `discover()` |
| `docs/manifest-reference.md` | Document `arch` field under `[language.*]` |
