# Include-hygiene — implementation audit

Running log of what changed while implementing
[`include-hygiene.md`](include-hygiene.md), so the work can be traced and backed
out commit-by-commit. Newest entries at the top.

## Status

- **Phase 1 (warn):** in progress.
- **Phase 2 (build enforcement):** not started.
- **Phase 3 (system libs + stdlib matching):** stdlib matching folded into Phase 1.

## Log

### Step 2 — `[lints]` manifest table

- `src/manifest/types.rs`: added `LintLevel { Allow, Warn(default), Deny }`
  (serde lowercase) and `LintsConfig { undeclared_include: LintLevel }`
  (`#[serde(rename = "undeclared-include")]`). New `Manifest.lints` field
  (`#[serde(default)]`).
- Re-exported `LintLevel`, `LintsConfig` from `src/manifest/mod.rs`.
- Default is `warn` even when `[lints]` is absent (matches the decision).
- 2 parse tests in `validate.rs` (default = warn; deny/allow parse).
- Test helpers build manifests from TOML strings, so no struct-literal breakage.

### Step 1 — classification core (`src/build/include_policy.rs`)

- New module `include_policy` (registered in `src/build/mod.rs`).
- `IncludeClass { Project, Dependency(name), Stdlib, Undeclared }`.
- `Language { C, Cxx }` + `Language::from_path` (`.c` → C, else C++ superset).
- `IncludeAllowlist::new(language, project_roots, dep_roots)` (canonicalises) +
  `classify(header_name, resolved_abs)`.
  - Order: project root → dep root → std-name → undeclared, so a project/dep file
    named like a std header is attributed to its owner (refines the plan's
    std-first order).
- Static `C_HEADERS` / `CXX_HEADERS` tables (C89–C23, C++98–C++23); C++ set =
  C++ ∪ C headers. Built once via `OnceLock`.
- 5 unit tests pass: stdlib-by-name, POSIX→undeclared, third-party→undeclared,
  project/dep override name, C excludes C++ headers.
- **Not yet wired** to the real resolver — `IncludeAllowlist::new` takes roots
  directly; a `from_resolved(manifest, …)` constructor comes in the wiring step.

### Step 0 — design committed (freight 3690123)

- `docs/include-hygiene.md` — the plan, with the decided stdlib-by-name policy.
- `docs/include-hygiene-audit.md` — this file.

## Decisions (frozen)

- **Stdlib-only is implicit**, matched by header *name* per language/`std` (not by
  directory — glibc and POSIX share `/usr/include`). POSIX/OS headers require a
  declared dependency.
- **Default lint level `warn`**, configurable via
  `[lints].undeclared-include = "allow" | "warn" | "deny"`.
- Diagnostics `source = "freight"`, `code = "undeclared-include"`.

## Phase 1 task checklist

- [ ] `src/build/include_policy.rs` — `IncludeAllowlist`, `IncludeClass`,
      `classify(spelling, resolved_abs)`, std-header tables, unit tests.
- [ ] `[lints].undeclared-include` in the manifest model + validation default.
- [ ] `src/lsp/include_hygiene.rs` — inclusion list → classified diagnostics.
- [ ] Hook into the LSP diagnostic merge (`src/lsp/mod.rs`).
- [ ] Fixture + integration test (`<zlib.h>` and `<pthread.h>` → one warning each).
- [ ] `manifest-reference.md` `[lints]` section.
