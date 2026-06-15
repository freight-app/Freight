# Include hygiene: declared-dependency enforcement

**Status:** planned · **Owner:** —
**Related:** [`docs/lsp-architecture.md`](lsp-architecture.md), [`docs/pipeline.md`](pipeline.md), [`docs/manifest-reference.md`](manifest-reference.md)

## Goal

A Freight build (and the editor experience it drives through `freight lsp`) should
only see the headers of packages the project actually declares. Concretely:

- An `#include` may resolve **only** to:
  1. the project's own sources / include dirs, or
  2. an include directory exported by a dependency declared in `freight.toml`
     (`[dependencies]`, `[build-dependencies]`, `[dev-dependencies]`, and the
     conditional `[os.*]` / `[arch.*]` variants), or
  3. a **language standard-library** header — identified by name, not directory
     (see [Standard-library policy](#standard-library-policy)).
- A **system library** (zlib, OpenSSL, …) is allowed only when it is named in
  `freight.toml` (e.g. a bare-version dep `zlib = "1.2"` resolved via pkg-config/registry).
- **POSIX / OS-SDK headers** (`<unistd.h>`, `<pthread.h>`, `<windows.h>`, …) are
  **not** part of the language standard library and therefore require a declared
  dependency, just like any other system library. This is deliberate — an
  undeclared `#include <pthread.h>` is exactly the portability bug we want to
  catch. *(Decision: "language stdlib only" — confirmed.)*
- Anything else — a header that resolves from a default system search path but is
  owned by no declared package and is not a standard-library header — is
  **undeclared** and must be surfaced.

This makes builds reproducible (no accidental dependence on whatever happens to be
installed on the build host) and turns "works on my machine" include drift into a
visible, fixable signal.

## Phasing

We ship this in three phases so each step is independently useful and low-risk.

| Phase | Behaviour | Risk |
|------|-----------|------|
| **1. Warn (this milestone)** | LSP emits an inline warning on any `#include` that resolves to an undeclared header. No build behaviour changes. | Low — advisory only. |
| **2. Enforce in the build** | The compile command's include search path is restricted to the allowlist; undeclared includes become hard compile errors. Opt-in via a lint level. | Medium — can break builds that relied on ambient headers. |
| **3. Declared system libs + standard-library matching** | System deps named in the manifest contribute their include dirs (resolved via pkg-config); language standard-library headers are recognised by name so they pass on every platform, while POSIX/OS headers require declaration. | Medium. |

Phase 1 is the immediate deliverable. Phases 2–3 are designed here so Phase 1's
data structures are the right shape to reuse.

---

## Core concept: the allowlist of include roots

Both the warning (Phase 1) and the enforcement (Phase 2) are driven by a single
set of **allowed include roots** — absolute directory paths under which an
included header is considered "declared". Computing this set once and reusing it
keeps the editor and the build in agreement.

Classification is two-stage — a **directory** check for project/dependency
headers and a **name** check for the standard library. The name check is
necessary because libc's `stdio.h` and POSIX's `unistd.h` live in the same
`/usr/include` and cannot be told apart by path; only the set of language-
*standard* header names is fixed and knowable.

```
classify(include_spelling, resolved_abs_path):
    if include_spelling in std_header_set(language, std):  -> Stdlib
    if resolved_abs_path under a project include root:      -> Project
    if resolved_abs_path under dep_include_dirs(d):         -> Dependency(d.name)
    if resolved_abs_path under a declared system dep's
       pkg-config include dirs (Phase 3):                  -> Dependency(s.name)
    otherwise:                                             -> Undeclared

project roots  = src/, include/, inc/, generated dirs (proto, header units)
dep roots      = union over each *active* declared dependency d of
                     dep_include_dirs(d.dir, d.manifest)    // build/deps.rs
std_header_set = the C / C++ / ... standard-library header names for the active
                 language and `std` — a fixed static table (see below)
```

- `dep_include_dirs()` already exists (`src/build/deps.rs`) and returns the
  include dirs a dependency exports to its dependants (from `lib.hdrs`, or
  auto-detected `include/` / `inc/`, plus `[compiler].includes`). Phase 1 reuses
  it verbatim.
- "Active" means the dependency passes the current `[os.*]` / `[arch.*]` /
  feature gating — the same filtering the resolver already applies when building
  the dep graph.
- The **standard-header set** is matched on the include *spelling* (`<stdio.h>`,
  `<vector>`), not on a resolved directory, so it is identical on Linux, macOS,
  and Windows. POSIX/OS headers are deliberately absent from it and fall through
  to `Undeclared` unless a declared dep owns their resolved path.

A small helper crate-internal module owns this:

```
src/build/include_policy.rs
    pub struct IncludeAllowlist {
        project_roots: Vec<PathBuf>,                    // canonicalised
        dep_roots:     Vec<(String, PathBuf)>,          // (dep name, canonical dir)
        std_headers:   &'static HashSet<&'static str>,  // for the active language
    }
    impl IncludeAllowlist {
        pub fn from_resolved(project_dir, manifest, resolved_deps, profile) -> Self
        pub fn classify(&self, spelling: &str, resolved_abs: &Path) -> IncludeClass
    }
    pub enum IncludeClass { Project, Dependency(String), Stdlib, Undeclared }
```

`classify()` returns *why* a header is allowed (or not), which lets the warning
name the owning package and powers a future "add this dependency" quick-fix.

---

## Phase 1 — inline warnings in the LSP

### Where the data comes from

`freight lsp` already merges its own diagnostics into the stream it publishes to
the editor (it intercepts clangd's `textDocument/publishDiagnostics` and combines
clangd + clang-tidy diagnostics before re-publishing — see `src/lsp/mod.rs`
around the `publish_diagnostics` / passthrough-intercept path). Undeclared-include
warnings are a **third source** merged into that same stream, so they appear
inline in the editor with no client change.

Resolving `#include` directives to absolute paths:

- The clang bridge already enumerates a translation unit's inclusions
  (`cb_inclusions` / the document-link path), giving, per directive in the main
  file: the directive's line/column range and the **resolved** included file
  path. We reuse this — it is exactly the "what did this `#include` resolve to"
  data we need, and it works whether the C/C++ front end is the bridge or clangd
  (we can also parse the directive ourselves and resolve against the compile
  command's `-I` set as a fallback).

### Algorithm (per open C/C++ document)

```
1. Build / fetch the cached IncludeAllowlist for the file's project + profile.
2. For each #include directive in the main file:
       spelling     = the include as written, e.g. "<pthread.h>" / "\"foo.h\""
       resolved_abs = resolved path of the include (skip if unresolved — that is
                      already a clangd "file not found" error, not our concern)
       if allowlist.classify(spelling, resolved_abs) == Undeclared:
           emit Diagnostic {
               range:    the directive's path-token range,
               severity: <lint level, default Warning>,
               source:   "freight",
               code:     "undeclared-include",
               message:  "`<name>` is not provided by any declared dependency; \
                          add the dependency that provides it to [dependencies] in freight.toml",
           }
3. Merge these into the URI's diagnostic set and publish.
```

Notes:
- **Scope to the main file only** — do not warn on transitive includes pulled in
  by a dependency's own headers (those are the dependency's problem, not the
  user's). The bridge inclusion data is already filtered to the main file.
- **Stdlib is not flagged in Phase 1** (see policy below) — only third-party,
  ambient headers are. This keeps the first cut quiet and high-signal.
- Re-run on `didOpen` / `didChange` / `didSave` and whenever the manifest changes
  (the LSP already watches `freight.toml` and calls `refresh_flags`).

### Config surface

A new manifest lints table, read at validation time:

```toml
[lints]
undeclared-include = "warn"   # "allow" | "warn" | "deny"   (default: "warn")
```

- `allow` — never emit (escape hatch).
- `warn` — LSP Warning diagnostic (Phase 1 default).
- `deny` — LSP Error diagnostic now; hard build failure in Phase 2.

### Deliverables for Phase 1

- [ ] `src/build/include_policy.rs` — `IncludeAllowlist` + `classify()`.
- [ ] Standard-header tables (C / C++, keyed by `std`) — a static set used by
      `classify()`; the single source of truth for "what is the stdlib".
- [ ] `src/lsp/include_hygiene.rs` — produce diagnostics from the bridge's
      inclusion list + the allowlist; hook into the diagnostic merge.
- [ ] `[lints].undeclared-include` parsed in `src/manifest/` (default `warn`).
- [ ] Tests: classification (project / dep / **stdlib by name** / **POSIX
      `<pthread.h>` → undeclared** / declared-system → allowed); a fixture
      project with a deliberately-undeclared `#include <zlib.h>` asserting one
      `undeclared-include` warning on the right line, and `#include <pthread.h>`
      asserting the same (portability case).
- [ ] Docs: short section in `manifest-reference.md` for `[lints]`.

---

## Phase 2 — enforce in the build

**Status: done** via the "lighter intermediate option" below — a pre-compile
validation pass (`build::validate_include_hygiene`) that re-runs the Phase-1
classification and fails the build on `deny` / warns on `warn`. The stronger
hermetic-includes variant (stop relying on the compiler's default search paths)
remains optional/future. See `include-hygiene-audit.md` Step 10.

Make the compile command itself unable to reach undeclared headers, so an
undeclared include is a real build error (matching `clangd`'s view in the
editor).

- `compile_commands::generate()` (`src/build/compile_commands.rs`) already
  assembles the full `-I` / `-isystem` set from project + dep include dirs. The
  change is to **stop relying on the compiler's default search paths** for
  third-party headers: pass the allowlist as the *only* non-stdlib include path
  source, and add `-isystem` entries for the toolchain stdlib explicitly (see
  Phase 3) rather than letting `/usr/include` leak in.
- This is gated by `[lints].undeclared-include = "deny"` (or a dedicated
  `[build].hermetic-includes = true`) so existing projects are unaffected until
  they opt in.
- A lighter intermediate option: keep the default search paths but add a
  pre-compile validation pass that re-runs the Phase-1 classification over the
  full preprocessed include set and fails the build on `deny`. Cheaper to
  implement, weaker guarantee (catches what got included, not what *could* be).

---

## Phase 3 — declared system libs & standard-library policy

### Declared system libraries

A dependency declared as a system lib must contribute its headers to the
allowlist, or every `#include <openssl/ssl.h>` from a declared `openssl` dep would
be flagged. Resolution order mirrors the existing dependency resolution chain:

1. `pkg-config --cflags <name>` → parse `-I` / `-isystem` dirs.
2. System-stub map (the hardcoded common-libs table) for libs without a `.pc`.
3. Explicit `include = [...]` on the detailed dep table as an override.

These dirs are added to `allowed_include_roots` for system deps. (Detailed-dep
field `include` already exists for header-only / foreign deps — reuse it.)

### Standard-library policy

**Decided:** the language standard library is the *only* thing implicitly allowed.
It is selected by `edition` / `std` in the manifest — part of the toolchain, not a
package you "depend on" — and, critically, it is **portable**: `<stdio.h>` and
`<vector>` mean the same thing on Linux, macOS, and Windows. That portability is
exactly why it is safe to allow without a declaration, and why nothing else gets
the same treatment.

- **Match by header name, per language/std.** Classification uses a static table
  of standard header *spellings* (`std_header_set` above), keyed by language and
  standard version:
  - C: the freestanding + hosted headers — `<stdio.h>`, `<stdlib.h>`,
    `<string.h>`, `<math.h>`, `<stdint.h>`, `<stddef.h>`, … (the set grows by
    `std`: C11 adds `<threads.h>`, `<stdatomic.h>`; C23 adds `<stdbit.h>`; etc.).
  - C++: `<vector>`, `<string>`, `<memory>`, … plus the C compatibility headers
    `<cstdio>`/`<cstdlib>`/…; the set grows by `-std` (C++20 `<concepts>`,
    `<ranges>`, `<span>`; C++23 `<expected>`, `<print>`; etc.).
  - Other front ends (Fortran, CUDA, …) get their own tables as support lands.
  - Matching by name (not directory) means the rule is identical across
    platforms and toolchains — no per-host stdlib-dir probing, no `/usr/include`
    leakage.
- **POSIX / OS-SDK headers are not standard-library headers** and are therefore
  absent from the table: `<unistd.h>`, `<pthread.h>`, `<sys/socket.h>`,
  `<windows.h>`, `<dlfcn.h>`, … An undeclared use of any of these is flagged — it
  is a platform dependency that belongs in `freight.toml` (e.g. via a
  `system`/pkg-config dep or an `[os.*]` section), which is what makes the
  portability requirement enforceable.
- A future strict mode (`[lints].undeclared-include = "deny"` +
  `strict-stdlib = true`) could require even standard-library use to be unlocked
  by an explicit `std`, but that is out of scope.

---

## Edge cases & decisions to confirm

- **`import` / `#import`**: header-bringing forms — `#import <h>` (Objective-C)
  and `import <h>;` / `export import "h";` (C++20 header units) — resolve to a
  header and are checked exactly like `#include` (done in Phase 1). A
  named-module import (`import foo;`, `import std;`) has no header path; it is
  classified against a module→package map instead (**done**): `lsp::index::
  ModuleIndex` scans the project's and each declared package's sources for
  `export module …;` (via `build::modules::parse_export_module`). An import of a
  module no declared package exports — and that isn't a `std`/`std.*` module —
  is flagged with an `undeclared-module` diagnostic and a `⚠ undeclared` inlay
  hint; a resolved one is labelled with its owning package, completes from the
  declared set, and goto-definition opens its interface unit.
- **Generated headers** (proto codegen, `[language.proto]`, header units): their
  output dirs must be in the allowlist. Wire from the existing generated-include
  paths in the pipeline.
- **`#include "local.h"`** quote-includes within the project: always Project, never
  flagged.
- **Conditional deps**: a header only declared under `[os.windows.dependencies]`
  should not warn on Windows but *should* warn on Linux — the allowlist is
  computed for the *current* host/profile, which gives this for free.
- **Path / git / url deps**: covered by `dep_include_dirs()` once fetched; no
  special handling.
- **False positives while a dep is unfetched**: if `.deps/` is not yet populated,
  the dep's include dir may not exist and its headers could be flagged. Mitigate
  by treating "declared but unfetched" as allowed (suppress) and surfacing a
  separate "run `freight fetch`" hint instead.
- **Performance**: the allowlist is cheap (path-prefix checks); compute once per
  project/profile and invalidate on manifest change. Canonicalise roots once.

## Link-feature hints (system libraries)

A system-library header like `<pthread.h>` is *allowed* by hygiene (it's
compiler/OS-provided), but including it is a silent trap: the code compiles yet
won't link without `[os.unix] features = ["pthread"]` (`undefined reference to
pthread_create`). Rather than forbidding it, the LSP emits a **Hint**-severity
diagnostic (`code: "link-feature-hint"`, `source: "freight"`) on the include when
the providing feature isn't declared in any `[os.*]`/`[arch.*] features`, with a
quick-fix **"Add `<feature>` to [os.<os>] features in freight.toml"**.

- Header → feature comes from the system-lib stub table (`system-libs.toml`
  `headers`); the `[os.*]` section is derived from the stub's `supports`.
- The hint is independent of `[lints].undeclared-include` (it's a *link* concern,
  not a hygiene violation) — it shows even under `allow`.
- The feature+os ride in the diagnostic's `data` field, so the quick-fix needs no
  server-side state; `insert_os_feature_toml` writes the `[os.*] features` array
  (formatting preserved).
- System-library headers are **not** indexed as ordinary headers. The inlay label
  and include-hover report the header's *standard origin* — `← POSIX` (`pthread.h`),
  `← stdlib` (`math.h` — ISO C even though it links `-lm`), `← Windows SDK`,
  `← Darwin` — kept **separate** from the *link library* (the `pthread`/`m`
  feature), which is conveyed in the hover/diagnostic. Origin is decided by the
  ISO stdlib name tables vs the stub's `[os.*]` section; header → link feature uses
  the stub `headers` table.

## Implementation checklist (Phase 1 first)

1. `src/build/include_policy.rs` — `IncludeAllowlist::from_resolved`, `classify`,
   unit tests.
2. `[lints].undeclared-include` in the manifest model + validation default.
3. `src/lsp/include_hygiene.rs` — bridge inclusion list → classified diagnostics.
4. Hook into the LSP diagnostic merge (`src/lsp/mod.rs`).
5. Fixture + integration test (undeclared `<zlib.h>` → one warning).
6. `manifest-reference.md` `[lints]` section; link this doc from `roadmap.md`.
