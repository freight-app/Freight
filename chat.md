
### 2026-06-24 — Claude — native CMake migration via File API (branch adaptors-as-plugins)

`freight init --migrate --native` now extracts a project's REAL build data from
CMake's File API and writes a freight-NATIVE manifest, instead of a `build = "cmake"`
self-build. (`--native` implies `--migrate`.)

- NEW src/migration/{mod.rs,cmake_fileapi.rs} (pub mod migration in lib.rs).
  cmake_fileapi::extract(dir): throwaway build dir, drops a `codemodel-v2` File API
  query, runs `cmake -S -B -DBUILD_TESTING=OFF`, parses codemodel + per-target JSON →
  CmakeModel { targets: [{name, kind, sources, defines, includes, std, language}] }.
  Sources = compiled + project-relative (headers/generated/abs dropped); includes
  relativized to project (system/dep includes dropped). Targets under
  test/example/vendor/doc subdirs excluded via codemodel directoryIndex.
- migration::render_native_manifest: faithful for the single-library shape (exactly
  one STATIC/SHARED lib target; executables ignored as tests/examples) → [lib] with
  authoritative srcs + [language] std + [compiler] includes/defines. Else None →
  caller falls back to the foreign self-build.
- NEW `[package] auto-discover` (default true; manifest/types.rs + build/discover.rs):
  when false, freight skips the src/ auto-walk and compiles ONLY explicit
  [lib].srcs/[[bin]].src. Native migration sets it false so the extracted list is
  authoritative (else a stray src/fmt.cc module unit gets walked in and breaks).
- CLI: InitArgs --native; ScaffoldOutcome.migrate_mode (Some("native"|"cmake")) drives
  a one-line report.

Verified end-to-end on real fmt: `init --migrate --native` → native manifest
([lib] srcs=[src/format.cc, src/os.cc], includes=[include], auto-discover=false) →
`freight build` compiles exactly those 2 → libfmt-native.a. Multi-lib/exe/configure-fail
→ foreign fallback (unchanged). Tests: 5 migration unit (fileapi parse, dir-exclude,
render single-lib/fallbacks) + tests/cmake_migrate_native.rs (e2e: native manifest +
build, test exe ignored). Full suite green (21 suites), clippy clean. Docs updated.
Uncommitted; nothing pushed.

### 2026-06-24 — Claude — split `freight init` vs `freight init --migrate` (branch adaptors-as-plugins)

Adoption of a foreign build is now opt-in. Plain `freight init` always writes a
freight-native manifest (+ hello-world when src/ empty); the CMake adoption (foreign
self-build, find_package harvest, submodule/FetchContent/add_subdirectory conversion)
moved behind `--migrate`.

- `freight init`: native manifest. If a CMakeLists.txt is present, prints a hint
  ("Run `freight init --migrate` to adopt it") via new `ScaffoldOutcome.cmake_detected`.
- `freight init --migrate`: the full adoption path (was the automatic CMakeLists
  branch). Errors (`FreightError::OptionError`) when there's no CMakeLists.txt.
- `init_project(lang, migrate: bool)`; InitArgs gained `--migrate`; cmd_init updated.
  Note: prior behavior auto-adopted any dir with a CMakeLists — that's the behavior
  change. write_cmake_manifest unchanged (still used by --migrate + its unit test).

Verified all 3 paths: plain init in CMake dir → native + hint; --migrate → build=cmake
+ harvested zlib; --migrate with no CMakeLists → clean error. Full suite green, clippy
clean. Docs updated. Uncommitted; nothing pushed.

### 2026-06-24 — Claude — `freight init` converts vendored git submodules → deps (branch adaptors-as-plugins)

Decluttering migration: when adopting a CMake project that has a `.gitmodules`,
`freight init` now converts each vendored git submodule into a freight `{ url, rev }`
dependency, pinned to the exact gitlink commit the superproject references — so the
build can pull deps through freight instead of carrying `third_party/*` trees.

- Parse `.gitmodules` (path/url), resolve pinned commit via `git ls-tree HEAD <path>`.
- Resolved commit → active `name = { url, rev }` dep + path added to a prune report.
  Unresolved commit → commented suggestion (manifest stays valid). Name colliding
  with an already-harvested find_package dep → skipped (no duplicate keys).
- Name derived from submodule path basename (lowercased). `ScaffoldOutcome` gained
  `pruneable_paths`; CLI prints `git rm <path>` suggestions. freight NEVER deletes —
  user prunes after a clean build.

All in src/new.rs (detect_submodules, submodule_rev, submodule_dep_name,
submodule_dependency_lines; write_cmake_manifest now returns prune paths). Verified
on real gRPC: 11 submodules → url+rev deps (abseil/boringssl/cares/googletest/xds/…),
each pinned to its committed SHA; benchmark/protobuf/re2 skipped (collide with
find_package harvest). 5 new unit tests; full suite green (755 lib + integration),
clippy clean. Docs: manifest-reference.md "Automatic adoption". Uncommitted; not pushed.

NOTE this is the STATIC counterpart to the runtime cmake dependency provider: the
provider bypasses vendored trees at configure time without touching the repo; this
rewrites the manifest so the trees can actually be removed.

FOLLOW-UP (same day): FetchContent_Declare → freight deps also done. `freight init`
now parses FetchContent_Declare blocks (CMakeLists + cmake/*.cmake, in
resolve/cmake.rs: detect_fetchcontent_in[_project], FetchContentDep): git source
(GIT_REPOSITORY+GIT_TAG) → `{url, tag}` or `{url, rev}` when GIT_TAG is a hex commit;
archive (URL+URL_HASH SHA256=) → `{url, sha256}`. Rendered in new.rs
fetchcontent_dependency_lines, deduped against harvested find_package + submodule
names. 4 new tests (2 parser in cmake.rs, 1 render in new.rs, +skip-no-source).
Verified on synthetic project (googletest tag / fmt rev / json archive+sha all
correct). 758 lib tests green, clippy clean.

FOLLOW-UP 2 (same day): add_subdirectory(third_party/*) vendoring → deps also done.
resolve/cmake.rs detect_add_subdirectory_in[_project] parses literal source-dir args
(skips ${vars}). new.rs add_subdirectory_dependency_lines + git_checkout_origin:
a vendored path (own CMakeLists.txt AND under a vendor dir [third_party/external/
vendor/deps/extern/contrib/subprojects/...] OR its own .git) → if it's a git checkout,
recover remote.origin.url + HEAD via `git -C` (only when <dir>/.git exists, else git
walks up to the superproject and mislabels) → {url, rev} + pruneable; else commented
{path=...} suggestion. Project's own subdirs (src/, lib/) skipped. Deduped vs
find_package+submodule+fetchcontent names. CLI prune message generalized to "vendored
dependencies". 3 new tests. Verified on synthetic (git-checkout foo→url+rev+prune,
non-git bar→commented, src/ skipped). Full suite green, clippy clean.

Migration-declutter now covers the 3 structured cases: submodules, FetchContent,
add_subdirectory. Remaining (low confidence, not done): bundled single-header/source
copies. Uncommitted; nothing pushed.

### 2026-06-24 — Claude — exported (public/interface) compile defines (branch adaptors-as-plugins)

New: libraries can export preprocessor defines to their dependents, so a consumer
compiles in the same configuration the lib was built with — without restating the
define. Two channels (cf. CMake target_compile_definitions PUBLIC):
- `[lib].defines = [...]` — always-on exported defines (lib's own build + all
  dependents; interface-only on a `header` lib).
- `pub-define:NAME[=value]` feature-list entry — feature-gated exported define;
  plain `define:` stays private. Unification → lib + consumers flip in lockstep.

Why: native leaf-lib ports (fmt/json/spdlog, in scratchpad experiments) exposed a
silent-miscompile gap — include dirs propagated to dependents but defines did not, so
a spdlog consumer missing SPDLOG_COMPILED_LIB/SPDLOG_FMT_EXTERNAL still "built" but
inlined 3827 spdlog symbols header-only (main.o KB→MB), never used libspdlog.a, and
linked bundled-vs-external fmt (ODR landmine). Now fixed.

Impl: `FeatureResolution.public_defines` (build/features.rs, parse `pub-define:`);
`LibTarget.defines` (manifest/types.rs); own-build fold in `stage_features`; per-dep
collection in `build_resolved_deps` (BuiltDeps.public_defines → BuiltDepsOutput) and
applied to the consumer in `run_pipeline_at`. Also fixed a latent bug: a dep's own
`define:` feature entries weren't applied to the dep's own build (added).

Tested: 2 new features.rs unit tests + tests/public_defines.rs (3 e2e, incl. a negative
case proving a private `[compiler].defines` does NOT export). Full suite green (750 lib
+ all integration). Verified on the real spdlog native port: clean consumer (no restated
defines) now correctly links libspdlog.a (main.o back to KB, 6 undefined refs).
Docs: manifest-reference.md (`[lib]` + `[features]`).

Follow-up — that gap is now FIXED (same branch): `resolve_dep_graph` (build/deps.rs)
recursed into transitive deps with an empty activated-deps set and never resolved the
dep's own features, so an optional dep activated by a *non-root* package's feature
(spdlog's `dep:fmt` behind default `external-fmt`) was dropped from the graph entirely
— losing its include dirs, lib, and exported defines (consumer fell back to system
fmt). Now the BFS queue carries each dep's requested features (from the declaring
manifest, via new `requested_features_for`), resolves them per node, and passes the
real activated_deps to `direct_compilable_deps`. Direct deps were always fine. The
idiomatic spdlog port (optional fmt via `dep:fmt` + `pub-define:SPDLOG_FMT_EXTERNAL`)
now builds clean end-to-end against the fmt port (not system fmt). Regression test:
tests/transitive_optional_deps.rs. Full suite green. Uncommitted; nothing pushed.

### 2026-06-22 — Claude — init/add foreign-project adoption (step 2, branch adaptors-as-plugins)

Replaces `freight migrate`. Both commands now detect foreign build systems and
emit the `external = true` + plugin + `[<backend>] build` wiring automatically.

- **`freight init`** (src/new.rs): if cwd has a `CMakeLists.txt`, writes a
  cmake-delegating manifest instead of scaffolding native sources —
  `[dependencies] cmake = "0.1"` + `[cmake] build = "<self>"`. Harvests
  `find_package()` names from CMakeLists.txt (textual scan, dedup, source order),
  drops system pkgs (Threads/OpenMP/MPI/…), maps CMake→pkg-config names
  (ZLIB→zlib, PNG→libpng, …), probes `pkg-config --modversion`: found →
  active `external=true` dep; unknown → commented suggestion (manifest stays
  valid). Decision taken with Max: "Probe pkg-config, comment fallback."
- **`freight add <git-url>`** (src/cli/commands/add.rs): after cloning, if the
  dep has no freight.toml → `adopt_foreign_dep` rewrites it `external = true` and,
  if `detect_build_system` recognises it, calls new
  `dep_cmds::manifest_add_foreign_build` (adds plugin dep + extends `[<backend>]
  build`, string→array promotion, idempotent).

Tests: new::tests (3: harvest+skip-system, name-map, comment-fallback),
dep_cmds::foreign_build_tests (1: wire+promote+idempotent). 736 lib tests green,
clippy clean (the 2 add.rs warnings are pre-existing). e2e verified: init on a
CMakeLists with ZLIB/Threads/Unknown → zlib=1.3.2, Threads skipped, unknown
commented. Docs: manifest-reference.md "Automatic adoption" block.

### 2026-06-22 — Claude — CMake-discoverable export for freight-built deps (foundation)

Max chose: make freight-built transitive deps discoverable via generated .pc +
Config.cmake (so an external cmake parent's find_package resolves freight's copy).
This was the blocker for "automatic on build": freight native builds emit raw
.a/.so + headers but no Config.cmake/.pc, so find_package(bar) wouldn't find them.

DONE (foundation, proven with real cmake):
- NEW src/build/cmake_export.rs: export_cmake_package(prefix, ExportSpec{cmake_name,
  pc_name, version}) writes <prefix>/lib/pkgconfig/<pc>.pc AND
  <prefix>/lib/cmake/<CMakeName>/<CMakeName>Config.cmake (+ConfigVersion). Config
  is keyed by the find_package NAME (case-sensitive), defines <Name>::<Name>
  IMPORTED target + <Name>_FOUND/_VERSION/_INCLUDE_DIRS/_LIBRARIES. 2 unit tests.
- VALIDATED with real cmake: find_package(Foo 1.0 CONFIG REQUIRED) against a
  prefix with the generated config → "FOUND Foo version=1.2.3 target=Foo::Foo".
- 748 lib green, clippy clean on the new file.

KEY DESIGN POINTS for the wiring (next):
- Config dir/file/target MUST use the cmake_name (find_package arg), not the
  freight/pc name (find_package(ZLIB) → ZLIBConfig.cmake, even if pc is zlib).
- System-registry hits need NO export/build (already installed; cmake finds them).
  Only freight-registry hits (download+build) need export.

Also added assemble_export_prefix(prefix, include_dirs, lib_files, spec): copies a
built dep's header trees + libs into <prefix>/{include,lib} then exports — the
bridge from a raw freight build to a discoverable package. 3 cmake_export tests.

DONE — the orchestration (build/pipeline.rs Stage 6b):
- export_transitive_registry_deps(project_dir, root_dir, target_dir, config,
  profile, progress) reads .pkgs/cmake-resolution.json; for each Registry dep with
  via=Some (transitive) and pkg-config ABSENT (not system-installed): builds it
  natively via run_pipeline_at(dep_dir, …, Some(root_dir)), locates its lib in
  root/target/deps/<pkg>/<profile> + headers in <dep>/include, calls
  assemble_export_prefix → target/cmake-export/<CmakeName>, and adds that prefix
  to the cmake plugin's seed_prefixes (CFG.prefixes). Gated to top-level builds
  (parent_root.is_none()) → no nested recursion. Warns (never fatal) on
  not-yet-fetched deps / build / export failures. System-installed + direct deps
  are skipped by design.
- NEW tests/cmake_export_build.rs: real `freight build` on a plain C app with a
  crafted report + a real .pkgs/bar freight lib → asserts target/cmake-export/bar/
  {lib/cmake/bar/barConfig.cmake, lib/pkgconfig/bar.pc, include/bar.h, lib/libbar.a}.
  Passes. 812 crate tests green, clippy clean on new code.

"Automatic on build" is COMPLETE: freight fetch → resolver → report; freight build
→ builds+exports transitive registry deps → feeds prefixes → external parent's
find_package resolves freight's copy. (Per Max's scope: transitive only; direct
stay external; system-installed found by cmake directly.)

### 2026-06-23 — Claude — provider calls freight directly; deleted the resolver executable

Max: let the cmake script invoke freight to provide deps; remove the separate
executable. Done — big simplification.

- NEW `freight cmake-provide <name>` (hidden subcommand, src/cli/commands/
  cmake_provide.rs) → build::pipeline::provide_cmake_package(name, project_dir,
  profile): installed (pkg-config/cmake-config) → None; freight pkg in .pkgs →
  build native + generated .pc/Config.cmake; foreign cmake in .pkgs → build via
  plugin (its own install); prints the install prefix (silent build events).
- cmake.freight provider now runs `${FREIGHT_BIN} cmake-provide ${dep}
  --profile ${FREIGHT_PROFILE}`, prepends the printed prefix to CMAKE_PREFIX_PATH,
  then find_package(... BYPASS_PROVIDER). run_build_system + run_script push
  FREIGHT_BIN (current_exe); cmake configure gets -DFREIGHT_BIN/-DFREIGHT_PROFILE.
- DELETED: freight-cmake-resolve bin (+Cargo [[bin]], +stale artifact), the
  fetch-time resolve_cmake_deps + cmake-resolution.json report, pipeline
  export_transitive_cmake_deps, and the whole resolve_cmake_tree machinery
  (resolve_cmake_tree/ResolvedCmakeDep/CmakeResolutionReport/CmakeResolution/
  CmakeSource/SourceProvider/PkgsSourceProvider/run_resolver/detect_fetchcontent*).
  Kept in resolve/cmake.rs: scanner (detect_cmake_packages*), cmake_to_freight_name,
  is_installed_cmake_package, RegistryHit/RegistryResolver/ConfiguredRegistries
  (used by `init` harvest). Removed obsolete tests; NEW tests/cmake_provide.rs (2:
  cmake-provide prints prefix; provider satisfies find_package during a build).

BUG fixed en route: build_foreign_self passed root=source_dir.parent() (parent of
the project) → the cmake provider's `freight cmake-provide` ran with the wrong cwd
(couldn't find .pkgs) AND foreign-self outputs went to the parent's target/.
Fixed root=project_dir.

gRPC re-verified under the new architecture: provider passes zlib/ssl/c-ares
(installed) and stops at absl (absent) — same honest outcome, now fully
provider-driven (no executable, no report). 811 crate tests green, clippy clean,
no build warnings. Docs updated (on-demand provider section).

### 2026-06-23 — Claude — CMake dependency provider (intercept, don't scrape)

Max: inject a .cmake that redirects find_package/FetchContent to freight instead
of statically scraping the scripts. Implemented via CMake 3.24+ dependency
providers — the canonical hook.

- plugins/cmake/cmake.freight: Freight.cmake now registers
  `cmake_language(SET_DEPENDENCY_PROVIDER freight_provide_dependency
   SUPPORTED_METHODS FIND_PACKAGE FETCHCONTENT_MAKEAVAILABLE_SERIAL)`. The provider
  records each request to FREIGHT_REPORT (method + name) and, for FIND_PACKAGE,
  redirects to freight via `find_package(... BYPASS_PROVIDER)` with freight
  prefixes prepended. FetchContent: record only; TRY_FIND_PACKAGE_MODE=ALWAYS +
  CMAKE_PREFIX_PATH make MakeAvailable prefer freight's copy. Falls back to the
  find_package macro shim on CMake <3.24. (Built with `+=`, not a long `+` chain —
  Rhai has an expression-complexity limit.)
- This is CMake's real evaluation → covers conditionals / computed names /
  generated calls that the static text scan misses. Verified on grpc: the report
  dynamically captured FIND_PACKAGE Threads + absl (configure stopped at absl).
- Static scanner (detect_*) still used for AHEAD-of-build discovery (init/fetch),
  since the provider only runs during a configure (can't pre-fetch without it).
  The two are complementary; the provider is now the build-time redirect+record.
- Updated the run_build_system unit test (report format is now "FIND_PACKAGE X").
815 crate tests green.

### 2026-06-23 — Claude — build `source` deps + real gRPC test (4 bugs found & fixed)

Wired `source` (FetchContent-URL) deps into the build, then stress-tested the
whole flow by adopting real gRPC (shallow clone, v1.66.0). Found + fixed 4 bugs.

Build wiring:
- pipeline.rs: export_transitive_registry_deps → export_transitive_cmake_deps,
  split into build_registry_dep (freight-native build + generated .pc/Config.cmake)
  and build_source_dep (fetch git/URL → build via cmake plugin which installs its
  own real Config.cmake → feed install prefix). Takes tool_paths now.
- NEW tests/cmake_source_build.rs (real cmake): a `source` dep builds+installs via
  the plugin; its Config.cmake lands in the prefix.

gRPC findings → fixes:
1. find_package calls live in cmake/*.cmake, not top CMakeLists → NEW
   detect_cmake_packages_in_project / detect_fetchcontent_in_project (glob
   cmake/**/*.cmake). +PkgConfig to system pkgs. init + resolver + recursion use it.
2. init emitted `[cmake] build="<self>"` → freight NATIVELY compiled grpc's src/
   (wrong). Fixed: init now writes foreign self-build `[package] build = "cmake"`
   → build_foreign_self early-returns, delegates wholly to cmake (no native compile).
3. validate required [[bin]]/[lib] → rejected delegated builds. Relaxed: allow none
   when a build-system plugin (cmake/make/…) is declared. (Foreign self-build via
   [package].build was already exempt.)
4. `cmake = "0.1"` plugin dep collided with the cmake-TOOL version check (tool
   4.3.4 ≠ "0.1"). Fixed cmake_build_dep_constraint to ignore 0.x (plugin) versions.
- NEW [package].defines (alias cmake-args) → passed to run_build_system for
  foreign self-builds (e.g. gRPC_*_PROVIDER=package).

gRPC result (honest): freight adopts it, resolves its real tree (OpenSSL/ZLIB/
c-ares/systemd → installed; absl/protobuf/re2/benchmark/otel → external),
delegates to cmake, and with defines=[gRPC_*_PROVIDER=package] configure gets past
zlib/ssl/c-ares (installed) and stops at the FIRST genuinely-absent dep (abseil).
Full build is environment-blocked (absl/protobuf/re2 not installed, no registry
serving them, shallow clone has no submodules) — not a freight bug. 815 crate
tests green, clippy clean on changes. Docs updated (init = foreign self-build).

### 2026-06-22 — Claude — harvest FetchContent/ExternalProject URLs (Source resolution)

Max: "if there is a fetch content, we can use that URL as well." A CMakeLists's
FetchContent_Declare/ExternalProject_Add already carries the source URL — harvest
it so an otherwise-external dep becomes fetchable.

- resolve/cmake.rs: NEW CmakeSource{git_repository, git_tag, url} + detect_fetchcontent
  (call_bodies() paren-matches FetchContent_Declare/ExternalProject_Add, pulls
  GIT_REPOSITORY/GIT_TAG/URL). NEW CmakeResolution::Source{#[serde(flatten)]
  CmakeSource}. resolve_cmake_tree gained a fetch_sources map param; priority is
  installed → registry → source(FetchContent URL) → external. 2 tests.
- bin cmake_resolve.rs: harvests FetchContent, merges names into roots, passes the
  map. fetch.rs: Source arm prints "→ source <url> (from CMakeLists)" + count.
- Build orchestration intentionally does NOT build Source deps yet — the project's
  own FetchContent (with FETCHCONTENT_TRY_FIND_PACKAGE_MODE) still handles them;
  freight-managed fetch+build-from-harvested-URL is a future enhancement.

Live demo (FetchContent fmt+json, find_package ZLIB+absl, FREIGHT_HOME=/tmp/fh):
  ZLIB→installed, fmt→installed 12.2.0 (host wins over the FC url), absl→external,
  json→source https://…/json.tar.xz (harvested from FetchContent_Declare URL).
814 crate tests green, clippy clean. Docs updated (added the `source` kind).

### 2026-06-22 — Claude — real-library test (gRPC) → added `installed` resolution kind

Tested a realistic gRPC-consumer CMakeLists (find_package: Threads/Protobuf/gRPC/
OpenSSL/ZLIB/c-ares/absl/re2) on this Arch host. Found TWO real bugs:
1. c-ares was marked `external` though installed — find_package(c-ares) uses cmake
   name "c-ares" but pkg-config/system-registry know it as "libcares".
2. OpenSSL/ZLIB resolved as `registry` when they're really just installed on host.

Fix — distinguish "installed on host" from "downloadable registry":
- resolve/cmake.rs: RegistryResolver::lookup now returns RegistryHit{version,
  installed}. New CmakeResolution::Installed{version} (host-available, no freight
  action) vs Registry{version} (downloadable → build+export). NEW
  is_installed_cmake_package(name): searches /usr/lib{,64}/cmake/<Name>/ (+share,
  +CMAKE_PREFIX_PATH) for <name>config.cmake / <name>-config.cmake — catches
  cmake-name≠pkgconfig-name and config-only packages.
- resolve_cmake_tree: registry hit → installed?Installed:Registry; else if
  is_installed_cmake_package → Installed; else External+recurse.
- ConfiguredRegistries: system-registry hit → installed=true; network → false.
- Build orchestration only builds Registry (downloadable); Installed/System/External
  skipped (already correct via the if-let). fetch.rs prints installed count.
  new.rs hint + tests updated for RegistryHit.

After fix, gRPC resolves correctly: OpenSSL/ZLIB/c-ares → installed; Protobuf/gRPC/
absl/re2 → external (genuinely absent; would be `registry` if a freight registry
served them). 812 crate tests green, clippy clean. Docs updated (resolution kinds).
NOTE: init harvest (cmake_dependency_lines) still uses only pkg-config+registry, not
is_installed_cmake_package — so c-ares-style installed-but-pc-mismatched libs show
as commented "set a version" in `freight init` output (cosmetic; cmake finds them
anyway). Could align later.

### 2026-06-22 — Claude — `freight fetch` runs the cmake resolver (core triggers it)

- resolve/cmake.rs: NEW CmakeResolutionReport{project, dependencies} (moved out of
  the bin, shared). NEW run_resolver(project_dir, pkgs_dir) → locates
  freight-cmake-resolve (sibling of current_exe, or PATH), runs it, parses JSON;
  advisory (None on any failure, never breaks fetch).
- cli/commands/fetch.rs: resolve_cmake_deps() runs after deps are fetched —
  resolves the project + each on-disk external dep (registry-first), prints
  "resolve X → registry a@v", WARNS on transitive externals not in any registry
  ("bar (needed by foo) … left to foo's CMake"), and writes the merged report to
  .pkgs/cmake-resolution.json. No-op for non-CMake projects (no extra output).
- NEW tests/cmake_resolve.rs: offline e2e — temp $FREIGHT_HOME system registry
  (zlib stub) + project find_package(ZLIB)+foo, .pkgs/foo→find_package(bar); runs
  the real `freight fetch` binary; asserts ZLIB=registry@1.3.2, bar=external via
  foo, Threads dropped. Uses CARGO_BIN_EXE_freight + FREIGHT_HOME.
- docs/manifest-reference.md: "Registry-first transitive resolution" + "System
  registry" subsections (both executables documented).

Whole crate green (746 lib + all integration suites), clippy clean on changes.

NOT DONE (the remaining big piece — needs a design call): the BUILD consuming
.pkgs/cmake-resolution.json to actually fetch+build the registry-resolved
transitive deps and feed their prefixes to the cmake plugin (CFG.prefixes). This
auto-builds undeclared transitive deps → changes build semantics / lock; bring to
Max before implementing.

### 2026-06-22 — Claude — wire the system registry into resolution (directory-backed "system" repo)

NEW src/registry/directory.rs: DirectoryRegistry (impl PackageRepo) reads
[package] stub .toml files from a dir (lookup=<dir>/<name>.toml, search scans
stems). DirectoryRegistry::system() → $FREIGHT_HOME/registries/system. 1 test.
- registry/mod.rs exports it; repos.rs repo_by_name("system") returns it
  (implements the CLAUDE.md `repo = "system"`).
- resolve/system_registry.rs: system_registry_dir() helper (shared by the bin +
  the repo).
- resolve/cmake.rs ConfiguredRegistries: now holds the local system registry +
  network repos. System registry is checked FIRST and never short-circuits (local,
  no transport error) → offline-safe + prefers locally-installed libs; network
  repos follow with the existing first-error short-circuit.

E2E verified: `freight-system-registry --no-registry` generated 543 stubs into
/tmp/fh (descriptions from pacman), then `FREIGHT_HOME=/tmp/fh
freight-cmake-resolve` on find_package(ZLIB)+find_package(TotallyMadeUpLib) →
ZLIB resolves {"kind":"registry","version":"1.3.2"} (from the system registry),
made-up lib → external. Fully offline. 746 lib green, clippy clean on new files.

Note: scoped to the "system" repo + the cmake resolver path. Did NOT inject the
system registry into the global registries_in_order (would change `freight
add`/`search` for every lib — easy follow-up if wanted).

### 2026-06-22 — Claude — freight-system-registry executable (generate system-side registry)

NEW bin `freight-system-registry` (src/bin/system_registry.rs, [[bin]]):
`[--out DIR] [--force] [--no-registry] [--limit N]`. For every installed
pkg-config package, writes a `[package]` stub .toml to OUT (default
$FREIGHT_HOME/registries/system): registry metadata if the pkg is in a freight
registry, else synthesized (version from pkg-config, description from the system
package manager). Best-effort registries (first transport error short-circuits →
offline-safe, verified).

Supporting lib additions:
- resolve/pkg_config.rs: pkg_config_list_all() -> Vec<(name, description)> via
  `pkg-config --list-all` (pkgconf fallback).
- resolve/system_pm.rs: SystemPm::describe(pc_file) (apt→dpkg, dnf/zypper→rpm,
  pacman→pacman -Qi; brew/winget None) + pc_file_path(name) (via pkg-config
  pcfiledir) + testable parsers (parse_dpkg_owner/parse_pacman_owner/
  parse_pacman_description). 2 tests.
- resolve/system_registry.rs: render_package_stub(name,ver,desc,StubSource) +
  toml escaping. 2 tests (round-trip parse).

Live-verified on this host (Arch/pacman): generated stubs e.g. glesv2 3.2 "The GL
Vendor-Neutral Dispatch library" (desc from pacman via .pc owner). 745 lib green,
clippy clean on new files.

### 2026-06-22 — Claude — cmake resolver is now its own executable (Max: not the library's job)

Decisions w/ Max: ship the resolver as a bin in the freight crate (reuse
freight_core); it RESOLVES + REPORTS only (JSON), never builds; core triggers it
at fetch time (the plugin/.freight never resolves — it only runs tools).

DONE:
- NEW bin `freight-cmake-resolve` (src/bin/cmake_resolve.rs, [[bin]] in Cargo.toml):
  `--project DIR [--pkgs DIR] [--out FILE] [--pretty]`. Scans CMakeLists, runs
  resolve_cmake_tree, writes a JSON Report{project, dependencies:[ResolvedCmakeDep]}.
- resolve/cmake.rs: CmakeResolution/ResolvedCmakeDep are serde (CmakeResolution is
  #[serde(tag="kind", rename_all="lowercase")] → {"kind":"registry","version":..} /
  external / system). NEW PkgsSourceProvider (only returns .pkgs/<name> dirs that
  exist on disk — a non-registry transitive dep has no fetch location, so it's a
  leaf; matches the KEY CONSTRAINT noted earlier).
- Live-verified: project with .pkgs/foo → resolver recursed into foo, found bar
  (via: foo), filtered Threads. Offline → all external (registry unreachable).
- 741 lib green, clippy clean on new files.

NOTE: resolve_cmake_tree algorithm still lives in freight_core (reused by the
bin); what changed is it's invoked as a SEPARATE EXECUTABLE, never wired into the
core build pipeline. init still uses the lib's scanner+hint directly (that's
harvest, not resolution).

NOT DONE (next): core runs `freight-cmake-resolve` during `freight fetch`,
consumes the JSON, builds the registry-resolved deps (needs a 'build a package by
name' path), feeds their prefixes via the existing CFG.prefixes orchestration;
warn on unresolvable transitive externals.

### 2026-06-22 — Claude — registry-first CMake dep resolver — ENGINE + init hint (branch adaptors-as-plugins)

Decisions w/ Max: transitive resolution runs at FETCH time (manifest keeps direct
deps; resolved tree → lock); registry-first applies to TRANSITIVE deps only
(direct stay external); direct deps get a registry-availability HINT comment.

DONE this pass (the engine + the hint):
- NEW src/resolve/cmake.rs: moved the CMake find_package scanner + name maps out
  of new.rs (detect_cmake_packages / cmake_to_freight_name / is_system_pkg).
  Added the recursive resolver: resolve_cmake_tree(roots, &dyn RegistryResolver,
  &mut dyn SourceProvider) -> Vec<ResolvedCmakeDep{cmake_name, freight_name,
  resolution: Registry{version}|External|System, via}>. BFS, registry-first
  (hit terminates → freight builds natively), external → fetch+scan+recurse,
  system terminates, cycle-safe. ConfiguredRegistries impl (best-effort: first
  transport error flips it off so init never hangs). 3 tests.
- new.rs init: cmake_dependency_lines now takes &dyn RegistryResolver. Direct deps
  stay external=true; version from pkg-config else registry; if in registry, append
  "— also in registry (drop `external`...)" hint. init builds ConfiguredRegistries.
  2 tests. 741 lib green, clippy clean, live init verified offline.

KEY CONSTRAINT found for the wiring: a transitive find_package name NOT in the
registry can't be fetched (we only know the name, no URL) → it stays the parent
cmake's concern (leaf; warn). Only registry-resolvable transitive deps (+ the
direct external deps already on disk) can be pulled in. So SourceProvider only
returns dirs for things freight can actually obtain.

NOT DONE (next, fetch/lock/build wiring): real SourceProvider that fetches into
.pkgs; call resolve_cmake_tree during `freight fetch`; record resolved registry
deps so the build builds them + Freight.cmake/prefix-orchestration steers the
external parent to them; warn on unresolvable transitive externals.

### 2026-06-22 — Claude — build-system plugins live in [build-dependencies] (Max correction)

A build-system plugin (cmake/make/…) is a build-time tool that links nothing, so
it belongs in [build-dependencies], not [dependencies]. (A plugin that also ships
a linked runtime, like proto, stays in [dependencies].)

- plugin.rs plugin_packages: now also discovers build-dependency **path** deps
  (source_package_dirs only walks runtime/dev path deps; version build-deps were
  already covered by the .pkgs scan). This is what makes a path-dep cmake plugin
  in [build-dependencies] discoverable.
- new.rs (init): emits `[build-dependencies] cmake = "0.1"`; harvested find_package
  libs (linked) stay under [dependencies], which is now only emitted when non-empty.
- dep_cmds.rs manifest_add_foreign_build (used by `add`): plugin → [build-dependencies].
- plugin_cmake.rs both tests now declare cmake in [build-dependencies] (validates
  the new path-dep discovery). Docs updated + placement note.

Known nuance: the `cmake` key in [build-dependencies] also feeds the legacy
cmake-tool version check (cmake_build_dep_constraint). `cmake = "0.1"` (plugin
version) is read as a tool floor → trivially passes, harmless. A real tool floor
(`cmake >= 3.20` AND the plugin) can't share one key yet — follow-up if needed.

739 lib + cmake integration green, clippy clean.

### 2026-06-22 — Claude — cross-plugin prefix orchestration (branch adaptors-as-plugins)

Foreign deps built by plugins can now find_package() / pkg-config each other and
core-resolved freight deps, across separate plugin invocations.

- build/plugin.rs: RawOutput/PluginBuildOutput/PluginCache gain `prefixes`; new
  `add_prefix(path)` plugin fn registers a produced install prefix. `run_plugins`
  takes `seed_prefixes`, keeps an `acc_prefixes` (seed + each plugin's add_prefix
  output), and injects it into every plugin's `CFG.prefixes` via new
  `inject_prefixes()` (explicit manifest `prefixes` wins). `run_build_system`
  gains a `prefixes` param → synthesized `CFG.prefixes`.
- pipeline.rs Stage 6b: seeds run_plugins with core dep prefixes (parent of each
  dep include dir — same heuristic as foreign-self). build_foreign_self now
  actually passes prefix_paths through (was a `let _ = prefix_paths` TODO).
- cmake.freight: reads CFG.prefixes as an array (joins `;` for CMAKE_PREFIX_PATH),
  accumulates within a `build=[...]` list AND calls add_prefix(prefix) per dep.

Tests: inject_prefixes (explicit-wins), seed_prefixes_reach_cfg_and_add_prefix
(unit), and **plugin_cmake::cmake_plugin_resolves_an_earlier_dep_via_find_package**
— real E2E: build=["liba","libb"], libb's CMakeLists find_package(liba) resolves
liba's freight-built prefix. 738 lib + cmake integration green, clippy clean.
Docs: CFG.prefixes + add_prefix in manifest-reference.

UNCOMMITTED on branch adaptors-as-plugins. Next: meson/autotools Freight.cmake
analogs (meson native/cross file + pkg-config; autotools PKG_CONFIG_PATH/--with) —
they don't honour CFG.prefixes yet, so cross-dep resolution is cmake-only for now;
archive-url adoption (only git-url adopts); vcpkg-converter still emits `type`.

### 2026-06-21 — Claude — LSP labels plugin-generated headers as generated

Deeper plugin awareness in the HeaderIndex. New HeaderOrigin::Generated; new
build/plugin.rs `plugin_generated_dirs()` returns (out_dir, plugin_name,
section) with provenance (plugin_include_dirs now projects from it). The LSP
refresh feeds those into build_source_indexes via the new GeneratedDirSpec, so
headers under target/<profile>/plugin-gen/<section> are indexed and credited to
the plugin:

  - #include hover tooltip: "**proto** — generated by build plugin"
  - inlay hint: "← generated (proto)"  (was "← <project>")
  - include-hint line: "**proto** (generated)/foo.pb.h"
  - include completion detail: "generated by proto"

build_source_indexes gained a `generated_dirs: &[GeneratedDirSpec]` param
(test call sites updated). New unit test
(generated_headers_are_indexed_with_plugin_provenance). Full suite green (836,
0 failures); clippy clean on changed files. CHANGELOG updated. Not committed.

### 2026-06-21 — Claude — plugin distribution from .pkgs + [plugin.schema] completion

Two plugin follow-ups.

1) Distribution. New build/plugin.rs `plugin_packages(project_dir)` enumerates
plugin packages from BOTH path deps and `.pkgs/` (registry/git/url fetches),
deduped by canonical path. run_plugins + plugin_generated_dirs now route
through it, so a plugin no longer has to be a path dep — `proto = "0.1.0"`
fetched into `.pkgs/proto` is discovered and run. The build already skips
linking plugin-only deps; fetched plugins run automatically like a Cargo build
script (same tools allow-list + project confinement). New unit test
fetched_plugin_in_pkgs_is_discovered_and_run.

2) [plugin.schema]. PluginManifest gained `schema: BTreeMap<String,String>`
(key→description, advisory). New plugin.rs PluginSchema + plugin_schemas();
section_matches is now pub(crate). The LSP caches plugin_schemas alongside the
workspace inventory (refresh_workspace_inventory) and threads them into
completion_result + hover_result:
  - editing inside a plugin section (e.g. [proto]) completes the schema keys,
    each labelled "plugin: <name>" with its doc (and no longer falls through to
    the top-level section list)
  - hovering a schema key shows its description + "provided by plugin `<name>`";
    hovering the [proto] header explains it's plugin-handled
proto example ships a [plugin.schema] for proto_path. New tests
plugin_section_completion_offers_schema_keys + plugin_section_hover_describes_
key_and_section.

Full suite green (839 lib, 0 failures); clippy clean on changed files.
manifest-reference.md + CHANGELOG updated. Not committed.

### 2026-06-21 — Claude — plugin scripts get HOST / TARGET objects

Plugins can now branch on platform. run_script pushes two Rhai object-map
constants built from crate::environment::Environment::for_project (computed once
per run_plugins call, shared across scripts):
  HOST   = #{ os, arch, family, pointer_width }
  TARGET = #{ os, arch, family, pointer_width, triple }
os/arch use the manifest's [os.*]/[arch.*] vocabulary (linux/windows/macos,
x86_64/aarch64); family is "windows"/"unix"/"wasm" (CMake WIN32/UNIX spirit);
TARGET.triple is "" for a native build, the full triple when cross. New private
PluginEnv struct + os_family/pointer_width/platform_map helpers. run_script
gained an `env: &PluginEnv` arg (test sites pass a deterministic test_env()).

Design notes from this thread (for the record):
- configure_file is a CMake-ism → it should live in a *cmake plugin* on top of
  generic file primitives, NOT as a core .rhai function. Not adding it to core.
- Rhai ships no file IO; the only off-the-shelf option (rhai-fs) is unsandboxed
  (raw absolute paths) → must NOT be enabled. Safe file IO = our own
  read_file/write_file/etc. routed through contained() (still TODO, not done
  this pass). Also TODO: route Rhai print/debug off stdout, set engine resource
  limits.

New tests host_and_target_objects_expose_platform + native_target_triple_is_
empty. Fixed stale module doc (listed a nonexistent out_dir() fn). Full suite
green (841 lib, 0 failures); clippy clean. manifest-reference.md + CHANGELOG
updated. Not committed.

### 2026-06-21 — Claude — Python-like sandboxed file IO + path helpers for plugins

Made the plugin script engine easier to read/write with Python-flavoured
helpers, all registered in register_fns (split into register_io_fns +
register_path_fns).

Filesystem (project-confined via contained(); writes auto-create parent dirs;
reads of a missing file raise like Python):
  read_text, write_text, append_text, copy, makedirs, listdir,
  exists, is_file, is_dir
Pure path strings (os.path-style, no fs/containment):
  join (str,str and array), basename, dirname, stem, ext

Hardening (from the earlier discussion):
  - on_print → tracing::info, on_debug → tracing::debug (never stdout, so the
    LSP stdio stream stays clean). `print(...)` is now safe Python-style logging;
    `throw "msg"` aborts (Rhai's raise).
  - set_max_operations(100M) + set_max_call_levels(256) bound runaway/malicious
    scripts. Did NOT add rhai-fs (unsandboxed) — we register our own confined fns.

Cleanup: bison.rhai/flex.rhai dropped their hand-rolled stem() (now built-in).
Fixed module doc again. New tests python_like_io_and_path_helpers +
io_outside_project_is_rejected (16 plugin tests total). Full suite green (843
lib, 0 failures); clippy clean; bison e2e still passes with built-in stem.
manifest-reference.md (functions split into Build outputs / Filesystem / Path
tables) + CHANGELOG updated. Not committed.

### 2026-06-21 — Claude — plugins: add_flag(tool, flag) + TOOLS constant

Plugins can now inject compiler/linker/archiver flags at a specific tool.
- New add_flag(tool, flag) Rhai fn; TOOLS constant lists every target
  (#{name,family,kind} from toolchain::builtin::all_compiler_templates + the
  "linker"/"archiver" roles).
- ToolFlag{tool,flag} added to RawOutput/PluginBuildOutput (+ persisted in the
  incremental PluginCache). Matching helpers compiler_tool_flags (name/alias/
  family/"compiler") and role_tool_flags ("linker"/"archiver").
- Threaded tool_flags through the whole compile/link path: build_sources →
  compile_sources / compile_sources_unity / compile_module_sources (+ compile_miu/
  compile_non_miu); applied via settings.extra_flags after the per-source
  compiler is chosen. config_fingerprint now hashes tool_flags so a flag change
  forces recompile. link_targets merges "linker" flags into the link command and
  "archiver" flags into link_static (ar).
- Scope: applies to the main build goal. Dep source-builds and test/bench/example
  compiles pass &[] (deps run their own plugins). Module up-to-date is mtime-based
  like header_unit_flags, so a tool_flag change on a pure-module TU needs a touch/
  clean (same existing limitation).

New tests add_flag_records_tool_targeted_flags +
tool_flag_matching_by_name_alias_family_and_role; cache roundtrip extended.
Full suite green (845 lib, 0 failures); clippy clean on changed files.
manifest-reference.md + CHANGELOG updated. Not committed.

Open question from user: should CUDA become a plugin too? (answering next)

### 2026-06-21 — Claude — plugins: capture(tool, args) + strip()

Added capture(tool, args) → #{ code, stdout, stderr }: like run() but returns
output instead of aborting on non-zero exit (same tools allow-list, cwd=project
root). Completes the codegen toolkit for build stamping / version / pkg-config
probes.

Gotcha found + fixed: Rhai's String.trim() mutates in place and returns () — so
`capture(...).stdout.trim()` yields "". Added a pure `strip(s)` helper (Python
str.strip name) that returns a trimmed copy; documented the footgun. Use
`strip(r.stdout)`.

Docs note: a stamping plugin should omit `inputs` so it re-runs every build
(else the incremental cache serves a stale stamp). New tests
capture_returns_output_and_respects_allowlist + capture_disallowed_tool_is_
rejected (20 plugin tests). Full suite green (847 lib, 0 failures); clippy clean.
manifest-reference.md + CHANGELOG updated. Not committed.

### 2026-06-21 — Claude — plugin tool output in build output + regex helpers

Two asks.

1) See tool stdout/stderr when building. `run` now captures output (.output()
instead of .status()) and streams it via a new BuildEvent::ScriptOutput
{source,text,is_err} through the progress sink — CLI prints it (print_script_output
in cli/output.rs, wired into both build.rs formatters; stderr tinted), LSP passes
silent() so the JSON-RPC stream stays clean. On failure, run's error now includes
the tool's stderr. `print()` also routes through ScriptOutput now (shows when
building, silent under LSP) instead of tracing. CtxState/run_script gained a
`progress` arg (threaded from run_plugins; tests pass silent()).

2) Regex for parsing output. Python re-flavoured (regex crate, already a dep):
re_test / re_find ([whole,g1,..]) / re_find_all (array of group-arrays) /
re_replace (pattern first; invalid pattern raises). Plus a lines(s) helper.

New tests run_surfaces_tool_output_via_progress + regex_helpers_extract_and_replace
(22 plugin tests). Gotcha: r#"..."# raw string can't contain "# — used "N" not
"#" as the replace char in the test. Full suite green (849 lib, 0 failures);
clippy clean. manifest-reference.md + CHANGELOG updated. Not committed.

### 2026-06-21 — Claude — plugins: LIB and BIN target objects

Plugin scripts can now introspect the consuming project's targets:
  LIB = #{ name, type, hdrs, srcs, link }  (or () when no [lib])
  BIN = [#{ name, src, required_features }, …]
Folded into PluginEnv (built in PluginEnv::for_project from the project manifest)
so run_script stays a single env param. New LibInfo/BinInfo owned structs +
lib_object/bin_array Dynamic builders; LIB/BIN pushed as scope constants.
test_env() + the host-test inline env updated with pkg_name/lib/bins.

New test lib_and_bin_objects_expose_project_targets (via run_plugins with a
shared-lib + 2-bin project). 23 plugin tests; full suite green (850 lib, 0
failures); clippy clean. manifest-reference.md + CHANGELOG updated. Not committed.

### 2026-06-21 — Claude — plugin constant renames: cfg→CFG, BIN→BINS

Consistency pass on the script scope constants. `cfg` is now `CFG` (and pushed as
a constant, not push_dynamic — read-only like the others). `BIN` → `BINS` (it's
an array). Updated proto.rhai, plugin_codegen.rs test script, the unit tests,
manifest-reference.md, plugins/README.md, CHANGELOG. Full suite green (850 lib);
clippy clean; bison + plugin_codegen e2e pass.

Open: user weighing BINS-as-array vs BIN["name"]-as-map. Recommending array
(iteration-first, ordered) + an optional bin("name") lookup fn — awaiting call.

### 2026-06-21 — Claude — [branch: adaptors-as-plugins] PKGS keyword

Started a branch (adaptors-as-plugins, off master) to explore replacing the
builtin foreign-build adaptors (cmake/make/meson/autotools/scons/bazel) with
build plugins. First, foundational + additive piece landed on the branch:

PKGS — a new .rhai map constant keyed by dependency name, each value
#{ name, dir, version }. dir = a path dep's directory, else .pkgs/<name> (may
not exist yet → exists()-check). Built in PluginEnv::for_project from
manifest.effective_dependencies(); pkgs_map() pushes it like LIB/BINS. New test
pkgs_map_exposes_dependencies. 24 plugin tests; suite green (850, 1 unrelated DAP
flake passes in isolation); clippy clean on plugin.rs. Not committed.

Target design (from user): foreign build systems become plugins handling [cmake]
etc.; a project does `[cmake] build = "libfoo"`, the plugin reads
PKGS["libfoo"].dir and builds it. Deps get `external = true` (replacing
type="cmake") so core doesn't try to build/link them. STILL TODO on branch:
external flag; a link_lib/add_library plugin fn (plugins can't contribute a
foreign-built .a/.so to the link yet — only add_flag("linker",...)); reference
cmake/make plugins; migrate migration/ + detection; then delete src/adaptors/
(keep pkg_config/system_pm — that's resolution, not building). Big parity effort;
adaptors stays working until the plugin path proves out.

### 2026-06-21 — Claude — PROFILE constant; branch folded back (no adaptor changes)

- Folded the throwaway adaptors-as-plugins branch back to master and deleted it
  (it had no unique commits — only the additive PKGS work, which is a general
  plugin feature, not an adaptor conversion). IMPORTANT: src/adaptors/ is
  UNTOUCHED — no foreign builder was ever turned into a script. The cmake plugin
  remained a design sketch only. The actual adaptors→plugins refactor will live
  on its own branch if/when built; it is NOT on master.
- Added PROFILE constant to the plugin scope (derived from target/<profile>),
  so scripts can branch on debug vs release: `if PROFILE == "release" {...}`.
  Values: "debug"/"release"/custom. New test profile_constant_reflects_the_
  build_profile (25 plugin tests). Suite green; clippy clean. docs + CHANGELOG
  updated. Not committed.

### 2026-06-21 — Claude — [branch: adaptors-as-plugins] external deps + cmake plugin

Started the real adaptors→plugins work on branch adaptors-as-plugins (off
master). Vertical slice working end-to-end:

- DetailedDep gains `external: bool` (additive; serde skip when false). Marks a
  dep as built by a plugin, not core.
- adaptors::build_foreign_deps now skips `external` deps (no auto-detect/build).
  Source is still fetched into .pkgs by `freight fetch` (unchanged).
- PKGS entries gain `.external`.
- New plugin fns link_lib(name_or_path) / link_dir(path) — sugar emitting
  `linker` tool-flags (reuses existing link plumbing; no pipeline changes).
- plugins/cmake/ reference plugin: [cmake] build="pkg" → reads PKGS["pkg"].dir,
  runs cmake configure/build/install into OUT_DIR, add_include_dir + link_dir +
  link_lib the installed libs. Respects PROFILE (Release/Debug).
- tests/plugin_cmake.rs: vendored CMake lib (app/vendor/mylib) built via the
  plugin, linked into app → "answer=42". Gated on cmake availability. PASSES.

Note: external deps / vendored sources must be INSIDE the project (the script
sandbox boundary). A sibling ../dep is rejected — needs the future
project-granted external-access opt-in. Fetched .pkgs deps and vendored deps both
work.

Full suite green (853 lib + cmake/bison/codegen e2e); clippy clean. docs +
CHANGELOG + plugins/README updated. NOT committed (entangled with the session's
general plugin work in the working tree — see below).

TODO on branch: make/meson/autotools plugins; migrate migration/ + detection;
then remove the builder half of src/adaptors/ (keep pkg_config/system_pm).
Commit/separation: general plugin work (CFG/BINS/capture/PKGS/PROFILE/...) and
this adaptor work are intermixed in plugin.rs, so master-vs-branch split needs a
decision before committing.

### 2026-06-21 — Claude — committed split + make/meson/autotools plugins

Did the (b) commit split:
- master @ dd684bf: all general plugin/scripting work (49 files), chat.md excluded.
- branch adaptors-as-plugins @ 53181c4 (external deps + cmake plugin), then
  da97609 (make/meson/autotools). Branch = master + 2 adaptor commits.

make/meson/autotools reference plugins (plugins/{make,meson,autotools}), same
external-dep + PKGS pattern as cmake:
- make: in-tree `make -C src`, expose include + glob produced *.a/*.so.
- meson: setup/compile/install, forces --libdir=lib for predictable lib path.
- autotools: out-of-source ./configure && make && make install (VPATH).
- New run(tool, args, cwd) 3-arg variant (project-confined cwd) for autotools.
- cmake plugin: forces CMAKE_INSTALL_LIBDIR=lib, checks lib + lib64.
e2e tests for all three (gated; autotools uses a hand-written configure → no
autoconf needed). run_with_cwd unit test added. Full suite green (854 lib + 4
build-system e2e); clippy clean.

Remaining to finish the adaptors→plugins migration (branch): migrate the
migration/ converters + detection to emit external+[cmake]/etc; then remove the
builder half of src/adaptors/ (keep pkg_config/system_pm). scons/bazel still
builtin (could get plugins too). Not pushed.

### 2026-06-21 — Claude — [branch] examples build cmake/make/meson via plugins

Updated examples/deps/{cmake,make,meson} from the builtin-adaptor path-dep style
to the new external=true + build-system-plugin pattern:
- freight.toml: plugin path dep (../../../plugins/<bs>) + dep marked external +
  [cmake]/[make]/[meson] build="...".
- Added install() rules to cmake (CMakeLists install TARGETS+DIRECTORY) and meson
  (install:true + install_headers) vendored libs so the install-based plugins
  stage headers+lib. make builds in-tree (no change to vendor).
- meson README updated.
- New gated build_examples tests deps_cmake/make/meson_plugin_example_runs
  (cmake→42, make→word count:5, meson→49). All pass in-place (artifacts
  gitignored under examples/**).
Verified all three run via the freight binary. Full suite green (15 binaries);
clippy clean. Branch commit e434428. (Accidentally rm'd a few tracked
.freight/config.toml + make/freight.lock during cleanup; restored via git
checkout.) Not pushed.

Branch adaptors-as-plugins now: dd684bf(master base) ← 53181c4 ← da97609 ←
e434428. Still TODO: migrate migration/ converters + detection to emit
external+[cmake]/etc; remove builder half of src/adaptors/; scons/bazel plugins.

### 2026-06-21 — Claude — [branch] (b2) stages 1-2: all 6 plugins + resolve/ split

Committing to (b2): foreign build systems become plugins; core never invokes
make/cmake/etc; resolution moves to its own folder.
- 611a5c4: scons + bazel plugins (plugins/ now covers all 6 build systems).
- 7958c8e: moved pkg_config / pkg_config_cache / system_pm out of adaptors/ into
  new src/resolve/ (they shell out to pkg-config, not make/cmake → core keeps
  them). adaptors/ re-exports during migration; only header_ownership needed a
  path fix. Suite green (15 bins).

Remaining (b2) — and two architectural forks before the breaking parts:
core invokes build tools at 4 invoke_build_system sites:
  410, 529 — foreign DEP builds  → remove (plugins cover these now).
  188      — build-DEP source build (a tool with CMakeLists) → FORK A.
  596      — build_foreign_self (freight building a package that *is* a cmake/
             meson project, the vcpkg-scraper stub shape) → FORK B.
FORK A: drop source-building build-deps (prebuilt/binary build-deps only)? or
  keep core building tools?
FORK B: foreign-self packages — drop, or reframe as "thin freight wrapper that
  depends on its own source as external + [cmake]"? Affects vcpkg-converter.
After those: migration/ must emit external+[cmake]/etc; then delete cmake.rs/
  make.rs/meson.rs/autotools.rs/scons.rs/bazel.rs + invoke_build_system; fix the
  remaining adaptors::-via-resolve consumers + tests. Not pushed.

### 2026-06-22 — Claude — [branch] source=true dep flag

Added `source = true` dep field (6a7f652): force-build a freight package from
source even when a prebuilt exists (bootstrapping default prefers prebuilt).
Additive — exposed in PKGS alongside external; resolver consumption comes with
the builder-removal/bootstrap-resolver stage. Decision: `type` stays functional
via adaptors for now; it'll be converted to a deprecated alias (→ external +
[section]) in the same stage that deletes the builders (auto-mapping earlier
would break type-deps for lack of the plugin). Suite green (DAP flake passes in
isolation); clippy clean.

Branch adaptors-as-plugins commits since master:
  53181c4 external+cmake → da97609 make/meson/autotools → e434428 examples →
  611a5c4 scons/bazel → 7958c8e resolve/ split → 6a7f652 source flag.
Remaining big piece: the version-keyed bootstrapping build-dep resolver
(prebuilt/system leaves, source=true override, cycle/termination errors, foreign
builds via plugins), then remove the 4 invoke_build_system sites + delete the 6
builder files, convert type→external, update migration emit + vcpkg + tests.

### 2026-06-22 — Claude — [branch] debug=true dep flag

Added `debug = true` dep field (3245df2): in a debug-profile build, fetch the
dep's debug prebuilt instead of the default release one (default: always link
release prebuilts even in debug). Additive; exposed in PKGS alongside
external/source. Resolver consumption with the prebuilt/bootstrap stage. Plugin
scripts can already pair PKGS[name].debug with the PROFILE constant.
Dep fetch-preference flags now: external, source, debug.

### 2026-06-22 — Claude — [branch] dep os/arch: target for regular, host for build-deps

Per design correction: `[dependencies]` os/arch (field + [os.*]/[arch.*]
sections) gate on the build TARGET (linked into the artifact); `[build-deps]`
os/arch gate on the HOST (tools run on host). Was host-for-all before; build-deps
were unfiltered. f323f92:
- platforms_for(os) generalizes host_platforms() to any os.
- dep_matches_env(dep, platforms, arch, current_target) — explicit platform/arch.
- effective_dependencies uses target os/arch (vendor::resolve_target on
  compiler.target, fallback host); effective_build_dependencies (new) uses host.
- adaptors build-dep pass uses effective_build_dependencies.
- Native builds unchanged (target==host) → existing tests stable. 2 new tests
  (regular→target, build-dep→host). targets field kept (exact-triple). Suite
  green (15 bins); clippy clean.

Still TODO (b2 core): version-keyed bootstrapping build-dep resolver consuming
external/source/debug; remove 4 invoke_build_system sites + delete 6 builder
files; type→external alias; migration emit + vcpkg + tests.

### 2026-06-22 — Claude — [branch] bootstrapping build-dep resolver (algorithm)

b4faf21: new src/resolve/build_deps.rs — pure resolver. resolve_build_deps(roots,
&ToolEnv) → leaves-first Vec<PlannedTool>. Per tool: source=true→FromSource;
else Prebuilt; else System; else FromSource. FromSource recurses into deps_of
(the tool's own build-deps), terminating at prebuilt/system leaves. (name,
version) cycle detection; Unresolvable error. All env knowledge behind ToolEnv
trait → pure + 6 unit tests (fake env): prefer prebuilt/system, source override,
bootstrap-via-older-prebuilt, cycle, unresolvable, dedup. Clippy clean.

Next: real ToolEnv impl (registry prebuilt query / system PATH / fetched
manifest deps_of) + wire resolve_build_deps into the build-dep pass (replace the
adaptors loop), with from-source tools built via the build-system plugins. Then
remove the 4 invoke_build_system sites + delete the 6 builder files + type→
external alias + migration emit + vcpkg + tests.

### 2026-06-22 — Claude — [branch] real HostToolEnv + system short-circuit wiring

05da170: HostToolEnv impl of ToolEnv (resolve/build_deps.rs) — system() via
toolchain::detect::which (PATH), deps_of() via a fetched pkg's
effective_build_dependencies, source() from fetched/declared version,
prebuilt()=None (no prebuilt-binary registry index yet → policy is "system tool
if present, else build from source"). Wired into the adaptors build-dep pass: a
build-dep with a satisfying tool on PATH is used directly (no fetch/build) unless
source=true. Suite green (error_examples flake passes in isolation); clippy clean.

Note: full plan-driven recursive orchestration (resolve_build_deps driving the
whole pass, incl bootstrapping a from-source tool via the build-system plugins)
needs the from-source build to run through plugins (the build_foreign_self
reframe) — that's the next piece, together with removing the 4
invoke_build_system sites + 6 builder files + type→external alias + migration/
vcpkg/tests. prebuilt() stays None until a prebuilt-binary registry index exists.

### 2026-06-22 — Claude — [branch] run_build_system: build foreign source via bundled plugins

53ae602: the build_foreign_self reframe ENABLER. Embedded the 6 build-system
plugin scripts (include_str! from plugins/<bs>/<bs>.rhai) + run_build_system(
backend, name, source_dir, out_dir, root, profile, defines, tool_paths, progress)
→ runs the bundled plugin with a synthesized scope (PKGS = just this package,
CFG.build=name, LIB=(), BINS={}), returns include_dirs + tool_flags(linker) +
sources. Factored engine setup out of run_script into run_engine (shared).
Containment: root must contain source_dir + out_dir (is_dir/add_include_dir are
contained). Tested programmatically vs a real CMake project (28 plugin tests).
clippy clean.

So core can now build foreign source through the plugins. Remaining to finish
(b2): rewire the 4 invoke_build_system sites to run_build_system (foreign-dep
410/529, build-dep source 188, foreign-self 596), delete the 6 builder .rs +
invoke_build_system, type→external alias, migration emit + vcpkg + tests.

### 2026-06-22 — Claude — [branch] foreign-self via run_build_system + deletion findings

d4f5eda: build_foreign_self now builds via run_build_system (bundled plugin)
instead of invoke_build_system — extracts libs from the plugin's linker flags,
places them in target/<profile>. New e2e foreign_self_cmake_package_builds_via_
plugin (first coverage of a core foreign-build path). Suite green (16 bins);
clippy clean.

FULL builder deletion is NOT mechanical — flagged to user, did the safe isolated
piece only. Blockers for deleting the 6 builder .rs + invoke_build_system:
- invoke_build_system still used at 2 more sites: parallel foreign-DEP pass (~421)
  and build_foreign_member_closure (~540); plus build-dep source build (~199).
- The foreign-dep job loop is INTERLEAVED with pkg-config resolution in the
  1900-line build_foreign_deps — must keep pkg-config, remove only foreign-build.
- Those foreign-dep paths have NO test coverage (examples use the external+plugin
  path), so rewiring is unverifiable without new tests.
- Parity gaps: transitive CMAKE_PREFIX_PATH (member_closure's purpose); build-deps
  produce binaries not libs (run_build_system is lib-oriented → drop source
  build-deps, Fork A1).
- type→external alias, migration/ emit, vcpkg-converter (separate crate) still
  assume the old shape.
Recommended next: (a) gut the foreign-DEP auto-build (unused → error for
non-external, keep pkg-config branch) + member_closure; (b) drop build-dep
source-build; (c) then delete cmake/make/meson/autotools/scons/bazel.rs +
invoke_build_system; (d) type→external + deprecation; (e) migration emit; vcpkg
as a separate-crate follow-up. Each with new test coverage.

### 2026-06-22 — Claude — [branch] DELETED the builtin foreign builders (b2 core)

12941c9 (BREAKING): removed src/adaptors/{cmake,make,meson,autotools,scons,
bazel}.rs + invoke_build_system + build_foreign_member_closure + BuildJob/Node/
topo_order/foreign_dep_dir/install_prefix/validate_backend/run/find_libs.
adaptors/ is now a single mod.rs (~1190 lines, was ~2560 across 7 files).

Behavior now:
- non-external foreign dep (type=X, or auto-detected cmake/make/...) → hard error
  foreign_needs_external() pointing at external=true + the [cmake]/etc plugin.
- registry/source dep needing a foreign build → same error.
- build_foreign_self → run_build_system (bundled plugin).
- build-deps: prebuilt/system only; source build-deps rejected (lib-plugins can't
  produce a tool binary). System short-circuit via HostToolEnv.
- pkg-config / system_pm (in resolve/) + detect_build_system stay; cmake
  version-check reimplemented inline.
All 16 test binaries green; clippy: only 3 pre-existing nits in untouched helpers.

REMAINING (b2 polish):
- type → external: currently type=X ERRORS with guidance (not a silent alias).
  Decide: keep error, or auto-map type→external (needs the plugin too).
- migration/ still emits type="cmake" → produces erroring manifests; update the
  cmake/make/autotools converters to emit external=true + [cmake] build + the
  plugin dep.
- vcpkg-converter (separate submodule) emits type=cmake stubs — separate follow-up.

### 2026-06-22 — Claude — [branch] Freight.cmake build-side override + dep recording

74e270d: the cmake plugin now generates+injects a build-side Freight.cmake via
-DCMAKE_PROJECT_TOP_LEVEL_INCLUDES (CMake ≥3.24). It:
- list(PREPEND CMAKE_PREFIX_PATH ${FREIGHT_PREFIXES}) → find_package/config/vcpkg
  resolve freight's copy first (override).
- PKG_CONFIG_PATH ← freight .pc dirs (override pkg-config).
- FETCHCONTENT_TRY_FIND_PACKAGE_MODE=ALWAYS → FetchContent prefers find_package
  (→freight) over git fetch.
- macro(find_package) override (builtin → reliable, unlike module cmds) records
  every resolved pkg to FREIGHT_REPORT; plugin reads it back + prints.
Covers user's list: cmake's own pkgs, vcpkg, pkg-config, git-links/FetchContent.
Prefixes/.pc plumbed via CFG.prefixes/pc_path (pipeline supplies transitive
prefixes later). run_build_system test asserts report records find_package(Threads).
Suite green (13 test bins, 0 fail); clippy clean.

Notes/limits: module-defined cmds (FetchContent_Declare, pkg_check_modules) can't
be macro-overridden from the top-level include (the module redefines them later)
→ rely on the variables/env + find_package instead. ExternalProject(git) is
record-only/best-effort. Other build systems (meson machine files, autotools
config-site/PKG_CONFIG_PATH) need their own analogs later.

REMAINING (b2 direction the user set): remove `type`/dep_type fully + remove
`freight migrate` (migration/ module) → replace with init/add foreign detection
that auto-wires the plugin and consumes the Freight.cmake report to populate deps;
then cross-dep prefix orchestration (pass built deps' install prefixes as
FREIGHT_PREFIXES); vcpkg-converter separate.

### 2026-06-22 — Claude — [branch] removed `type` dep field + `freight migrate`

4562f84 (BREAKING): deleted DetailedDep.type (foreign builds go via external +
plugin; type had no consumer). Updated adaptors loop, build/deps, install, fetch,
dep_cmds, LSP completion/params/signature (now external/source/debug). Deleted
`freight migrate` + the whole src/migration/ module + the cmake-lossless dep.
Docs updated (manifest-reference, cargo-vs-freight, architecture). ~130 migration
tests removed with the module; suite green (732 lib + integration), clippy clean.

Branch adaptors-as-plugins commits (off master dd684bf):
  53181c4 external+cmake → da97609 make/meson/autotools → e434428 examples →
  611a5c4 scons/bazel → 7958c8e resolve/ split → 6a7f652 source → 3245df2 debug →
  61c7b80 -unity → f323f92 os/arch target-vs-host → b4faf21 resolver algo →
  05da170 HostToolEnv+wiring → 53ae602 run_build_system → d4f5eda foreign_self →
  12941c9 DELETE builders → 74e270d Freight.cmake → 4562f84 -type/-migrate.

NEXT (step 2: init/add foreign detection) — hit decisions, see below.
