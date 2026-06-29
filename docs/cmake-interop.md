# CMake interoperability

Freight talks to CMake in **both directions**, and the two halves compose:

1. **Building a CMake project with freight** — a `build = "cmake"` self-build runs
   the real `cmake` tool through the cmake plugin, while freight steers compilers,
   flags, and dependency resolution from the outside. This is the safe, supported
   way to bring an existing CMake project into a freight build or registry without
   rewriting its build system.
2. **Exposing a freight package to CMake** — every freight-built package is wrapped
   in a generated `<Name>Config.cmake` + `.pc`, so a downstream `find_package()` /
   `pkg_check_modules()` finds it like any system library.

Three mechanisms carry this, each owning one job:

| Mechanism | File | Job |
|---|---|---|
| **Toolchain file** | `src/build/cmake_toolchain.rs` | *What to build with, and where to look* — compilers, flags, and freight-first dependency search. |
| **Dependency provider** | `plugins/cmake/Freight.cmake` + `freight cmake-provide` | *Provide on demand* — intercept each `find_package` / `FetchContent` and hand back freight's copy. |
| **Package export** | `src/build/cmake_export.rs` | *Be findable* — emit `<Name>Config.cmake` + `.pc` for freight-built libraries. |

The rest of this document describes each, then how they fit together.

---

## 1. The generated toolchain file

When the cmake plugin builds a CMake project, freight writes
`Freight.toolchain.cmake` into the build directory and passes it via
`-DCMAKE_TOOLCHAIN_FILE=…` (the plugin reads the `FREIGHT_TOOLCHAIN` script
constant and only adds the flag when it is non-empty — see
`plugins/cmake/cmake.freight`).

CMake reads a toolchain file **before `project()`**, which makes it the earliest,
most universal place to install freight's environment — earlier than any
`find_package`, and honoured by sub-builds and `FetchContent` children too. The
file does three things.

### a. Compilers

```cmake
set(CMAKE_C_COMPILER "/usr/lib/llvm/bin/clang")
set(CMAKE_CXX_COMPILER "/usr/lib/llvm/bin/clang++")
```

The foreign build uses the **same toolchain freight selected for native builds**.
freight resolves these with `select_compiler` (which picks the toolchain *entry*,
consistent across languages by link capability) followed by
`resolve_compile_binary` (which yields the *correct per-language* binary — `clang`
for C, `clang++` for C++, not the C++ driver for both). Languages with no detected
compiler are simply omitted. C, C++, Fortran, and CUDA are emitted when present.

### b. Host-compatibility flags

```cmake
set(CMAKE_CXX_FLAGS_INIT "-include cstdint")
```

Machine-local flags from `cmake-c-flags` / `cmake-cxx-flags` in
`~/.freight/config.toml` land in `CMAKE_<LANG>_FLAGS_INIT` (space-joined). This is
the home for host-compat shims — e.g. `-include cstdint` to paper over older C++
code that misses a transitive include under a newer libstdc++ — applied to **every**
`build = "cmake"` build on the machine, set once, without per-project patches.
`_INIT` is used (not a hard `CMAKE_<LANG>_FLAGS` override) so the project's own flags
still append.

### c. Dependency redirection (freight-first)

```cmake
list(PREPEND CMAKE_PREFIX_PATH "/…/.pkgs/zlib/…/install" "/…/.pkgs/png/…/install")
set(CMAKE_FIND_ROOT_PATH "/…/zlib/install" "/…/png/install")   # freight first
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM BOTH)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY BOTH)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE BOTH)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE BOTH)
```

freight's package prefixes are **prepended** to `CMAKE_PREFIX_PATH` and placed
first in `CMAKE_FIND_ROOT_PATH`. Ordering is search priority: when both freight and
the system provide a library, `find_package` / `find_library` resolve freight's copy,
with the host as a genuine fallback. For a project with many dependencies this is
what keeps the per-project diff small — instead of editing each dependency, freight
redirects the whole search at the toolchain level.

### Native overlay vs. cross

The find-root modes change with whether the build targets a foreign triple:

- **Native overlay** (no `[target]` triple) — mode `BOTH`: freight first, host as
  fallback. No `CMAKE_SYSTEM_NAME` is set.
- **Cross** (a `target` triple in config) — freight additionally emits
  `CMAKE_SYSTEM_NAME` / `CMAKE_SYSTEM_PROCESSOR` / `CMAKE_SYSROOT`, and uses mode
  `ONLY` for libraries/includes/packages so the host environment can never leak into
  a foreign target. `PROGRAM` stays `BOTH` (compilers and tools still come from the
  host). The sysroot is appended **after** the freight prefixes in
  `CMAKE_FIND_ROOT_PATH`, so freight's copies still win, with the sysroot as the
  real fallback.

### When it is skipped

If the project already declares its own `CMAKE_TOOLCHAIN_FILE` (e.g. a vcpkg
toolchain in `[…cmake] defines`), freight does **not** generate or inject one — it
must not clobber a toolchain the project relies on. Detection is a prefix match on
`CMAKE_TOOLCHAIN_FILE` across the configured defines (`plugin.rs::has_user_toolchain`).

> The dependency *provider* (next section) is **not** registered from the toolchain
> file — CMake forbids that. It lives in `Freight.cmake`, injected via
> `CMAKE_PROJECT_TOP_LEVEL_INCLUDES`. The toolchain says *where to look*; the
> provider *provides on demand*.

---

## 2. The on-demand dependency provider

The injected `Freight.cmake` registers a [CMake dependency
provider](https://cmake.org/cmake/help/latest/command/cmake_language.html#dependency-providers)
(CMake 3.24+) that intercepts every `find_package` and `FetchContent_MakeAvailable`
at configure time, using CMake's own evaluation rather than text scraping. For each
one it calls `freight cmake-provide <name>`, which makes freight's copy available and
prints an install prefix to add to `CMAKE_PREFIX_PATH`. A request resolves to:

- **installed** — already on the host (pkg-config, *or* an installed
  `<Name>Config.cmake`, so `find_package(c-ares)` matches even though pkg-config
  calls it `libcares` — see `resolve/cmake.rs::is_installed_cmake_package`). freight
  provides nothing; CMake finds it.
- a **freight package** under `.pkgs/` — built natively and wrapped in a generated
  `.pc` + `<Name>Config.cmake` (see export, below).
- a **foreign CMake project** under `.pkgs/` — built via the cmake plugin, which
  runs its own `install`, yielding the project's real `<Name>Config.cmake`.
- **nothing** — freight stays out of the way and CMake's normal search runs;
  `FETCHCONTENT_TRY_FIND_PACKAGE_MODE=ALWAYS` is set so a freight/installed copy still
  wins over a network `FetchContent` download when present.

This is dynamic and self-contained: no separate resolver binary and no resolution
file — the script calls `freight` directly, on demand, only for the packages a
configure actually asks for.

---

## 3. Exposing freight packages to CMake (export)

For a downstream CMake `find_package(Foo)` to succeed, the dependency must look like
a normal installed package. `src/build/cmake_export.rs` writes, into a freight
package's install prefix:

```
<prefix>/lib/pkgconfig/<pc_name>.pc
<prefix>/lib/cmake/<CMakeName>/<CMakeName>Config.cmake
<prefix>/lib/cmake/<CMakeName>/<CMakeName>ConfigVersion.cmake
```

`export_cmake_package` (or `assemble_export_prefix`, which also copies the built
`.a`/`.so` into `<prefix>/lib/` first) produces a prefix ready to drop onto
`CMAKE_PREFIX_PATH`. The pkg-config name and the CMake config name are tracked
separately on the `ExportSpec` because they frequently differ (e.g. `libcares`
vs. `c-ares`).

Because the provider already knows the **exact** `find_package` name CMake asked
for, freight exports *reactively* (when a request comes in) rather than eagerly for
every package — eager export would have to guess the CMake-side casing (`ZLIB` vs
`zlib`) and could publish a config under the wrong name.

---

## How it composes — end to end

A foreign CMake `app` that does `find_package(greet)`, where `greet` is a freight
dependency:

1. freight builds `greet` natively, then `cmake_export` writes
   `…/greet/install/lib/cmake/greet/greetConfig.cmake` (+ `.pc`).
2. freight generates `Freight.toolchain.cmake` for `app`: `clang`/`clang++`,
   `-include cstdint`, and `greet`'s install prefix prepended to
   `CMAKE_PREFIX_PATH` / `CMAKE_FIND_ROOT_PATH`.
3. The cmake plugin configures `app` with both
   `-DCMAKE_TOOLCHAIN_FILE=Freight.toolchain.cmake` and the provider injected via
   `CMAKE_PROJECT_TOP_LEVEL_INCLUDES`.
4. `app`'s `find_package(greet)` hits the provider → `freight cmake-provide greet`
   → freight points `greet_DIR` at the exported config → CMake resolves freight's
   `greet`, compiled with freight's toolchain.

No edits to `app`'s `CMakeLists.txt`; the redirection is entirely external.

---

## Adopting a CMake project

`freight init` only scaffolds a freight-native project. To build an *existing* CMake
project with freight, write a **foreign self-build** manifest:

```toml
[package]
name  = "thing"
build = "cmake"
```

CMake then configures and builds the whole project through the cmake plugin, with
everything above applied. Such a manifest can be written by hand or generated by the
separate **`freight-migrate`** tool (`freight-migrate <dir>`). Auto-generating a
*native* manifest from an arbitrary C++ project is best-effort and easy to get subtly
wrong, so it lives in `freight-migrate`, not `freight` — the supported path is the
`build = "cmake"` self-build run by the real cmake tool. See
[manifest-reference.md](manifest-reference.md#foreign-build-system-options) for the
manifest fields and the `freight-system-registry` helper.

---

## Configuration knobs

```toml
# ~/.freight/config.toml  (machine-wide)
cmake-cxx-flags = ["-include", "cstdint"]   # → CMAKE_CXX_FLAGS_INIT in the toolchain
cmake-c-flags   = []                         # → CMAKE_C_FLAGS_INIT
target          = "aarch64-linux-gnu"        # presence flips the toolchain to cross mode
sysroot         = "/opt/sysroot"             # → CMAKE_SYSROOT, appended after freight prefixes
```

Compilers come from freight's normal toolchain detection / `freight toolchain use`;
they are not configured here. See [manifest-reference.md](manifest-reference.md) for
the full config-file reference.
