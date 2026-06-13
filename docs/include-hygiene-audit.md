# Include-hygiene — implementation audit

Running log of what changed while implementing
[`include-hygiene.md`](include-hygiene.md), so the work can be traced and backed
out commit-by-commit. Newest entries at the top.

## Status

- **Phase 1 (warn):** ✅ complete and verified end-to-end.
- **Phase 2 (build enforcement):** ✅ pre-compile validation pass in `freight
  build` — `deny` fails the build, `warn` emits build warnings, `allow` skips.
- **Phase 3 (system-lib header ownership):** ✅ first cut — declared
  packages/slots own system headers via a Tier-A ownership table + Tier-B
  pkg-config dedicated dirs, in both the build pass and the LSP. Remaining:
  hosting/generating the per-OS Tier-A data file and a `pkg-config --list-all`
  global reverse index for naming packages of *unknown* headers (see Step 11).

## Log

### Step 11 — Phase 3: system-library header ownership

A declared `system`/pkg-config dep can now own its headers, so a legitimately
used `<zlib.h>`/`<cblas.h>` is no longer flagged under `deny`. New module
`src/build/header_ownership.rs`, two complementary sources:

- **Tier A** — `OwnershipData`: package/slot → header globs, keyed by **freight
  package name** (distro-portable). In-core per-OS `seed()` (Linux: zlib,
  sqlite3, bzip2, lzma, expat, pcre2, gmp, mpfr, ncurses, readline, uuid + BLAS
  and LAPACK **slots** with their interchangeable providers) plus an optional
  downloaded override at `~/.config/freight/header-ownership-<os>.toml` (merged
  over the seed; missing/malformed → seed, **fail-open**). Shared headers are an
  OR (declare any one BLAS provider → `cblas.h` is owned), never a conflict.
- **Tier B** — `pkg_config_dedicated_dirs(dep)`: a declared dep's pkg-config
  `--cflags` dirs **excluding** default roots (`/usr/include`,
  `/usr/local/include`, `/include`) so a dedicated `…/SDL2` is allowed without
  over-allowing all of `/usr/include`.

Wiring:
- **Build** (`validate_include_hygiene`): declared-dep Tier-B dirs added to the
  allowlist; findings whose header matches a declared dep/slot's Tier-A glob are
  suppressed; remaining diagnostics name candidate packages (`<cblas.h> is
  provided by openblas, atlas, mkl — add one`). `FreightError::UndeclaredInclude`
  message simplified to the per-finding lines.
- **LSP** (`compute_include_hygiene`): same Tier-A suppression + candidate
  naming (Tier-A only — subdir libs already reach the LSP via compile_commands,
  and the hot path must avoid per-keystroke pkg-config). So editor and build now
  agree.

Two guards (the whole point):
1. **No over-allow** — Tier B never folds in a default system root; bare
   `/usr/include` headers are attributed *only* by Tier A's explicit lists.
2. **Fail-open** — absent ownership data attributes nothing; it can only ever
   *add* "declared", never manufacture an undeclared finding.

Tests: 4 unit (`header_ownership`: globs, slot attribution, candidates, default-
dir exclusion) + integration `declared_owner_suppresses_system_header`
(`examples/broken/undeclared-include-owned/`). E2e verified both build and LSP:
declaring `zlib` suppresses `<zlib.h>` while `<pthread.h>` stays flagged, and an
undeclared `<zlib.h>` is reported as "provided by zlib".

**Still open for Phase 3:** (a) host + generate the per-OS Tier-A files (hook the
existing vcpkg/registry scraper; let registry stubs carry `provides-headers`);
(b) a lazy `pkg-config --list-all` reverse index so even headers *not* in Tier A
can name their owning package in the diagnostic; (c) macOS/Windows seeds;
(d) finalize the POSIX/OS-header policy.

### Step 10 — Phase 2: build-time enforcement (+ a non-ASCII crash fix)

The include-hygiene check now runs in `freight build`, not just the LSP.

- **`build::validate_include_hygiene`** (`src/build/mod.rs`) runs at the top of
  `build_sources`, before any compiler is invoked. It re-runs the Phase-1
  `include_policy::check_includes` over every C-family source's directives
  (`c`/`cpp`/`cuda`/`hip`/`objc`/`objcpp`), using the build's declared include
  dirs as the allowlist and probing each source's compiler (`select_compiler`)
  for its system dirs (cached per compiler+language) to confirm an undeclared
  header actually exists. Per `[lints].undeclared-include`:
  - `deny` → `FreightError::UndeclaredInclude` (new variant) — build fails with
    a `path:line: <header> is not provided…` list.
  - `warn` → one `BuildEvent::Warning` per finding; build proceeds.
  - `allow` → pass skipped entirely (no cost).
- **Crash fixed:** `parse_includes` computed the directive column via
  `raw.len() - rest.len()`, but `rest` is a slice of the *comment-stripped*
  `line`, not `raw`. With a multi-byte char after the directive (e.g. a non-ASCII
  comment) the resulting byte index landed inside a char and **panicked** — in
  both the build pass and the LSP. Now computed against `line` (a valid suffix
  boundary). Regression test `parse_includes_handles_non_ascii_comment`.
- **Fixture + tests:** `examples/broken/undeclared-include/` (`<pthread.h>`
  under `deny`); integration tests `undeclared_include_blocks_build_under_deny`
  and `undeclared_include_names_the_header` (asserts `<stdio.h>` is *not*
  flagged). End-to-end verified for all three lint levels.

**Phase 3 still open — the design constraint:** a `system =`/pkg-config dep
should contribute its headers to the allowlist so `#include <openssl/ssl.h>`
from a declared `openssl` isn't flagged. The naive fix (add `pkg-config
--cflags` `-I` dirs to the allowlist) is unsafe: pkg-config commonly returns
`/usr/include`, which would mark *every* header there declared and gut the
check. Phase 3 needs to add only the dep-specific include subdirs (and honour
the detailed-dep `include = [...]` override) while still treating bare
`/usr/include` as undeclared. Until then, `deny` is reliable for projects whose
deps are project-local / path / git / url / cached-package; it can
false-positive on pkg-config/`system` deps, so those projects should stay on
`warn` (the default).

### Step 9 — named C++20 module imports resolved to packages (`import foo;`)

The last open piece of the include/import hints: a named-module import had no
header path, so it was hardcoded to a generic `← module` label and never
classified. Now it is handled the same as a header `#include`:

- **`lsp::index::ModuleIndex`** — module name → owning package. Built alongside
  `HeaderIndex` (same `HeaderDirSpec` list + `.pkgs/`) by scanning each declared
  package's `src/` for `export module …;` via the now-`pub(crate)`
  `build::modules::parse_export_module`. Records the interface unit's path.
- **`include_policy`** — `IncludeDirective` gained a `DirectiveKind`
  (`Header` | `Module`); `parse_includes` now emits `Module` directives (a
  dotted identifier terminated by `;`; partitions `:part` and unterminated
  lines rejected) instead of silently dropping them, so a module-line edit also
  invalidates the hygiene fast-path. `check_includes` skips `Module` directives
  (no header to resolve).
- **Inlay hints** (`compute_inlay_hints`) — `import std;`/`std.*` → `← stdlib`;
  a module a declared package exports → `← <pkg>`; the project's own module →
  `← module`; anything else (when the lint isn't `allow`, on a complete `;`
  statement) → `⚠ undeclared`. Tooltips via `module_hover_markdown_for`.
- **Diagnostics** (`compute_include_hygiene`) — an undeclared module (not
  `std`, not in the `ModuleIndex`) is published as `source:"freight"
  code:"undeclared-module"` at the configured severity, parity with
  `undeclared-include`.
- **Completion** — `import …;` now offers `std`/`std.compat` **and** every
  declared module, each labelled with its package.
- **Goto-definition** — `import foo;` jumps to the interface unit
  (`export module foo;`) when a declared package provides one.

Tests: `parse_named_module_rejects_partitions_and_noise`,
`module_index_scans_export_module_declarations`,
`module_labels_reflect_provenance`,
`include_completion_module_suggests_declared_packages`, plus updated existing
parse/completion tests. End-to-end verified by driving `freight lsp` against a
temp project (declared path-dep module → `← vecmod`; `boost.json` →
`⚠ undeclared` + `undeclared-module` diagnostic; completion lists `vecmod.core`;
goto opens `vecmod/src/core.cppm`). `cargo test -p freight --lib` green
(679/680; the one failure is the known flaky `dap::server` parallel race, passes
alone). Uncommitted.

### Step 8 — `#include`/`import` completion scoped to declared libraries (freight 1303ae8)

- `textDocument/completion` inside an `#include` / `#import` / `import`
  directive is answered by freight instead of clangd (which lists every header
  on the include path — all of `/usr/include` — contradicting the policy):
  - angled `<…>` → stdlib tables (detail `C standard library` /
    `C++ standard library`) + declared-package headers,
  - quoted `"…"` → declared-package/project headers only,
  - named module `import st…` → `std` / `std.compat`.
- Each item's `detail` names the source library (`<pkg> <version>` /
  `this project`); textEdit appends the closing `>`/`"`/`;` if missing.
- New: `include_completion_context` + `include_completion` +
  `HeaderIndex::completion_entries` in `lsp/index.rs`;
  `c_std_headers`/`cxx_std_headers` accessors in `include_policy.rs`.
- 5 new unit tests; full lib suite 676 green. Non-directive completions still
  forward to clangd unchanged.

### Step 7 — per-keystroke hygiene cost (freight ba9c131)

- Hints lagged while typing: every `didChange` re-loaded + canonicalized
  `compile_commands.json`.
- Memoized the parsed directive list per document (skip the whole pass when
  includes are unchanged) and cached declared dirs + compiler per file.
- Both caches invalidated in `refresh_compile_commands`; per-doc entries
  dropped on `didClose`.

### Step 6 — also cover `import` / `#import` (header-bringing forms)

- `parse_includes` now recognises, in addition to `#include`:
  - `#import <h>` / `#import "h"` (Objective-C),
  - `import <h>;` / `import "h";` and `export import …;` (C++20 header units).
- Named-module imports (`import foo;`, `import std;`) carry no header token and
  are skipped — resolving a module name to a package needs a module→package map
  (a later step; noted in the plan).
- 1 new test (`parse_includes_handles_import_and_objc_forms`); **11 module tests.**
- End-to-end verified: `import <pthread.h>;` is flagged with the same
  undeclared-include warning; `import std;` and `<vector>` are not.

### Step 5 — LSP wiring (Phase 1 complete)

- `DiagCache` gained a `freight` field; both merge sites now chain
  clangd + tidy + freight.
- `ServerState.system_include_dirs: Option<Vec<PathBuf>>` (probed once, cached).
- `Server::compute_include_hygiene(uri, text)` — runs `check_includes` and stores
  the results as `source:"freight" code:"undeclared-include"` diagnostics
  (severity from the lint level: warn→2, deny→1; allow→cleared/no-op).
- Helpers: `undeclared_include_level()` (reads `[lints]` from the project
  manifest), `declared_dirs_and_compiler()` (parses `-I`/`-isystem`/`-iquote` and
  argv[0] from compile_commands.json), `cached_system_dirs()`.
- Called from `handle_did_open` / `handle_did_change` (full-text sync) /
  `handle_did_save`.
- **End-to-end verified** against the `freight lsp` binary on a real project
  (`/tmp/ih`): `#include <pthread.h>` → one Warning on the right span; `<vector>`
  and `<cstdio>` (stdlib) not flagged; `[lints] undeclared-include = "allow"`
  silences it (0 diagnostics).
- Works with the clang bridge gated off (bridge-free resolution path).
- Suite: my unit tests all pass. The 4 failing `*_hello_builds` integration tests
  are pre-existing/environmental (they invoke `freight build`; they fail
  identically with my changes stashed — the sandbox can't run them).

### Step 4 — system-dir probe + `check_includes` orchestration

- `system_include_dirs(compiler, language)` runs `<cc> -E -x <lang> - -v` and
  `parse_search_dirs()` extracts the `#include <...> search starts here:` block
  (handles macOS `(framework directory)` suffix). Empty on failure → safe (an
  unconfirmed header just isn't flagged).
- `UndeclaredInclude { line, start_col, end_col, spelling }`.
- `check_includes(source, file_dir, declared_dirs, system_dirs, language)` ties
  parse → resolve (declared then system) → classify → finding. Flags only headers
  that are undeclared **and** present; skips declared, stdlib (by name), and
  not-found (clangd's file-not-found).
- 2 new tests (system-block parse; full flow flags only `<pthread.h>`). **10
  include_policy tests total.**
- The whole classification/resolution logic is now complete and tested in
  isolation. Remaining: wire `check_includes` into the LSP diagnostic publish
  (gather declared_dirs from the file's compile command, probe system dirs once).

### Step 3 — `#include` directive parser + resolver (`include_policy.rs`)

- `IncludeDirective { name, angled, line, start_col, end_col }` (0-based, span
  includes delimiters).
- `parse_includes(source)` — line scan with a `/* */` + `//` comment state
  machine so commented-out includes aren't flagged. Handles `#  include`.
- `resolve_include(directive, file_dir, search_dirs)` — quote includes search
  the file's dir first, then the search path; returns the first existing file.
- 3 new tests (directive extraction incl. columns, multiline-block-comment skip,
  quote/angle/missing resolution). 8 include_policy tests total.
- **Resolution strategy (decided, bridge-free):** the LSP passes the file's
  compile-command `-I` dirs (declared project+dep) plus the compiler's probed
  system dirs as `search_dirs`. Resolved-under-declared → allowed; std-name →
  allowed; resolved-under-system → undeclared; unresolved → skip (clangd already
  reports file-not-found). Avoids depending on the (gated-off) bridge.

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
