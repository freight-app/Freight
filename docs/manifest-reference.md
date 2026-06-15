# freight.toml — Manifest Reference

A `freight.toml` file at the root of a project describes everything freight needs to build, test, and
package it. Every field is optional unless noted as required.

---

## `[package]`

Required. Identifies the project.

```toml
[package]
name        = "myproject"          # required — lowercase, hyphens allowed
version     = "0.1.0"             # required — semver string
authors     = ["Alice <a@b.com>"]  # optional — shown in freight info
description = "Short description"  # optional
license     = "MIT"                # optional — SPDX identifier
default-run = "myproject"          # optional — default [[bin]] for `freight run`
supports    = "(windows & !uwp & (x86 | x64)) | (!windows & !osx)"
```

`default-run` names the `[[bin]]` that `freight run` builds and runs when the
project has more than one binary and `--bin` is not given. It must match a
declared `[[bin]]` name.

`supports` is optional. When present, it is a boolean platform expression that
controls whether the package is buildable on the current host/target. Freight supports
identifiers such as `windows`, `linux`, `macos`/`osx`, `unix`, `uwp`, `x86`, `x64`/`x86_64`,
`arm`, and `arm64`/`aarch64`, plus the operators `!`, `&`, `|`, and parentheses.

---

## `[language.<key>]`

Languages are **detected automatically** from source file extensions — no manifest declaration
needed. A `.cu` file activates the `cuda` language key; a `.asm` file activates `asm`; and so on.

`[language.<key>]` sections are **optional configuration** applied only when source files of that
language are actually present. A declared section for an absent language is silently ignored.

Each compiler template supplies a default standard (e.g. `c++17` for C++). Use `[language.*]` to
override it or to set language-specific options.

```toml
[language.cpp]
std = "c++20"   # override the template default; values come from [standards] in the template

[language.c]
std = "c17"

[language.fortran]
std = "f2018"   # optional; template default applies if omitted
```

Available standard strings depend on the loaded template. Common values:

| Key | Default | Typical overrides |
|---|---|---|
| `cpp` | `c++17` | `c++20`, `c++23` |
| `c` | `c11` | `c17`, `c23` |
| `fortran` | `f2018` | `f2003`, `f2008` |

### `[language.proto]` — protobuf code generation

`[language.proto]` is a special language key that triggers **protobuf code generation** via
`protoc`. When declared, freight discovers all `.proto` files under `src/`, runs `protoc
--cpp_out=<dest>` on each, and injects the generated `.pb.cc` files into the normal C++
compilation step. The generated header directory is added to the include path automatically
so `#include "foo.pb.h"` works without any manual flags.

```toml
[language.proto]
# Directory for generated C++ files.  Default: target/<profile>/proto-gen/
# dest = "src/generated"

# Extra --proto_path roots beyond src/ and the project root (comma-separated).
# srcs = "proto/"

# Extra flags forwarded verbatim to protoc (whitespace-separated).
# extra_flags = "--experimental_allow_proto3_optional"
```

The `protoc` binary is resolved from **tool_paths** (populated by `[build-dependencies]`) first,
then from the system PATH. To pin to a specific protoc version:

```toml
[build-dependencies]
# Prebuilt protoc binary — freight extracts it and uses the protoc from its bin/.
protoc = { url = "https://github.com/protocolbuffers/protobuf/releases/download/v27.0/protoc-27.0-linux-x86_64.zip", type = "none" }

[dependencies]
# Link the protobuf runtime library (resolved via pkg-config or the registry).
protobuf = "3"
```

Incremental builds: protoc is only re-invoked for `.proto` files that are newer than their
corresponding `.pb.cc` / `.pb.h` outputs. Other files are skipped with a `Fresh` event.

---

## `[lib]`

Declares a library output. Omit this section for binary-only projects.

```toml
[lib]
type = "static"                          # required — static | shared | header-only
srcs = ["src/mathlib.cpp", "src/vec.cpp"] # source files (string or list; globs allowed)
hdrs = ["include/mathlib.h"]             # public headers exposed to dependents
```

`srcs` accepts either a single string or a list. Glob patterns are expanded relative to the
project root.

`hdrs` lists the public API headers. Freight infers the public include directories from the
parent paths of the listed files — e.g. `["include/mathlib.h", "include/vec.h"]` automatically
exposes `include/` to dependents. When `hdrs` is empty, freight falls back to auto-detecting
`include/` or `inc/` directories.

`header-only` libraries skip compilation entirely — freight records the include paths so
dependents can use them, but no `.a` or `.so` is produced.

---

## `[[bin]]`

Declares a binary target. Repeat the section for multiple binaries.

```toml
[[bin]]
name = "mytool"       # required — output binary name
src  = "src/main.cpp" # entry-point source file (default: "src/main.cpp")
required-features = ["cli"]  # optional — only built when all listed features are active
```

When multiple `[[bin]]` sections are present, each binary is compiled from its own entry-point
source plus any shared sources discovered in the project's source tree. Linker deduplication
ensures that `main()` from one binary is not linked into another.

`required-features` gates the target: the binary is only linked when **all** the
listed features are active for the build (mirrors Cargo). A target whose
requirements aren't met is silently skipped rather than erroring. Each name must
be a declared `[features]` key. Pair it with `[package] default-run` to keep
`freight run` unambiguous when optional binaries come and go.

---

## `[[example]]`

Example programs. Files under `examples/` are **auto-discovered** (the example
name is the file stem), so most projects need no `[[example]]` section at all.
Declare one only to set a custom name or `required-features`:

```toml
[[example]]
name = "demo"
src  = "examples/demo_main.cpp"
required-features = ["gui"]
```

Examples are not built by a normal `freight build`. Build them explicitly:

```sh
freight build --examples          # build all
freight build --example demo      # build one
freight run   --example demo      # build + run one
```

Each example is compiled and linked against the project's library/non-entry
objects (like a test or benchmark) into `target/<profile>/examples/`. A declared
`[[example]]` whose `src` is under `examples/` overrides the auto-discovered
entry for that file. `required-features` gates an example the same way as
`[[bin]]`.

---

## `[dependencies]`, `[build-dependencies]`, and `[dev-dependencies]`

| Section | When it applies | What freight does with it |
|---|---|---|
| `[dependencies]` | All builds | Compiled and linked into every artifact |
| `[build-dependencies]` | All builds | Fetched and built **before** regular deps; any `bin/` in the installed output is prepended to PATH for all subsequent build steps |
| `[dev-dependencies]` | Debug builds and `freight test` | Compiled and linked only when `--profile dev` (the default) or during test builds |

`[build-dependencies]` is the right place for tools like `cmake`, `ninja`, `protoc`, `flex`, `bison`, and any other executables that are invoked *during* compilation but do not end up linked into your final binary. Freight builds and installs them first, then uses whatever binaries they produced (e.g. `.pkgs/cmake/bin/cmake`) for every cmake/make/meson/autotools dep build in the same project — so you can pin away from a system cmake that breaks an older library.

```toml
[build-dependencies]
cmake = ">=3.20, <4"   # use any cmake 3.x; avoids cmake 4 breaking old CMakeLists.txt
ninja = ">=1.10"       # prefer locally-installed ninja over system one
```

### Version dependency (automatic resolver chain)

```toml
zlib    = "1.3.1"       # require at least this version
openssl = ">=3.0"       # comparator-style constraint
libpng  = "1.6"         # concrete version or range — a bare `*` is rejected

# Equivalent detailed form, useful with optional/features/default-features keys:
zstd = { version = "1.5" }
```

A concrete version or range is **required** — a bare `*` is rejected at validation, because
C/C++ libraries change their API between versions. Freight uses the version installed on the
system if present, and downloads it from the registry otherwise.

For version-only dependencies, Freight tries each resolver in order and uses the first that succeeds:

1. **pkg-config** — checks `pkg-config --modversion <name>` against the version constraint and
   injects the resulting `-I`/`-L`/`-l` flags.
2. **System-lib stub** — matches the name against the bundled stubs in `toolchains/system-libs/`
   (see below). If a stub matches, freight injects `-l{link_name}` directly. No package manager required.
3. **Registry** — downloads the package source from the configured registry into `.pkgs/` and builds it.

To restrict resolution, use the `repo` key:

```toml
pthread = { version = "0", repo = "system" }       # stubs only — never hit the registry
zlib    = { version = "1.3", repo = "my-registry" } # use a named registry from config
```

### Path dependency

```toml
myutils = { path = "../myutils" }
```

Compiles the freight project at the given path and links its library archive. The dep's `include/`
directory is added to the include path automatically. Path deps with a `freight.toml` are treated as
freight projects; those without are treated as foreign build systems (see below).

### System dependency

There is no dedicated `system`/`pkg-config` dependency field. A library installed on the system is
resolved through the normal chain (pkg-config → stub → registry) from a bare version constraint:

```toml
openssl = "3.0"   # pkg-config `openssl` → stub → registry
zlib    = "1.3"   # pkg-config `zlib`   → `z` stub → registry
```

For well-known OS libraries (pthread, ws2_32, libm, dl, rt, d3d11, …), Freight ships a built-in
stub table (bundled `system-libs.toml`). A stub carries the correct `-l` name, header list, and a
`supports` expression (e.g. `supports = "unix"`) so it is only applied on matching platforms. The
table is data-driven: add or override entries by dropping `.toml` files (same format) into
`$FREIGHT_HOME/toolchains/system-libs/` (default `~/.freight/toolchains/system-libs/`) — a user
entry with the same name replaces the built-in. Stub file format:

```toml
# ~/.freight/toolchains/system-libs/mylib.toml
[mylib]
link     = "mylib"          # optional; defaults to the table name → -lmylib
supports = "linux | macos"  # host platforms this stub applies to
headers  = ["mylib.h"]      # headers it provides (include-hygiene / browser)
```

**Versionless system libraries** (pthread, m, the OpenCL loader, …) have no meaningful version and
are linked via *platform features* under the relevant `[os.*]` / `[arch.*]` section, not as a
dependency entry — see [`[os.*] features`](#osplatform-features) below:

```toml
[os.unix]
features = ["pthread", "m"]   # -lpthread -lm on Unix

[os.windows]
features = ["ws2_32"]         # -lws2_32 on Windows
```

To force the stub path and skip the registry, pin the resolver with `repo = "system"`:

```toml
pthread = { version = "0", repo = "system" }
```

### pkg-config resolution

pkg-config is the first step of the resolver chain for any bare-version dependency — no special
field is needed. When `pkg-config --modversion <name>` satisfies the constraint, freight runs
`pkg-config --cflags --libs <name>` and injects the resulting include dirs
(`-I`) into compilation and link flags (`-L`, `-l`, `-pthread` etc.) verbatim into the linker
command. The query string is passed as-is to pkg-config, so version constraints work:
`"glib-2.0 >= 2.56"`.

When both `system` and `pkg-config` are set, pkg-config is tried first. If it fails (not installed
or package not found), freight falls back to `-l{system}` and prints a warning.

**pkgconf fallback** — if `pkg-config` is not found on `$PATH`, freight automatically retries with
`pkgconf` (a compatible alternative common on Alpine, Void, and other distributions).

**Cross-compilation** — when `[compiler] target` is set, freight resolves `PKG_CONFIG_PATH`,
`PKG_CONFIG_LIBDIR`, and `PKG_CONFIG_SYSROOT_DIR` with the following priority order so each can
be configured independently per target:

1. `<VAR>_<target>` — e.g. `PKG_CONFIG_PATH_aarch64-linux-gnu`
2. `<VAR>_<target_underscored>` — e.g. `PKG_CONFIG_PATH_aarch64_linux_gnu`
3. `TARGET_<VAR>` — e.g. `TARGET_PKG_CONFIG_PATH`
4. `<VAR>` — the plain variable

**Static linking** — set `PKG_CONFIG_ALL_STATIC=1` in your environment to pass `--static` to
every pkg-config invocation, requesting static-link flags from all `.pc` files.

### Git dependency

A git dependency is a `url` ending in `.git` (or any `url` combined with a `branch`/`tag`/`rev`).
There is no separate `git` field.

```toml
easyloggingpp = { url = "https://github.com/amrayn/easyloggingpp.git" }
easyloggingpp = { url = "https://github.com/amrayn/easyloggingpp.git", tag = "v9.97.1" }    # pin to tag
easyloggingpp = { url = "https://github.com/amrayn/easyloggingpp.git", branch = "main" }    # track branch
easyloggingpp = { url = "https://github.com/amrayn/easyloggingpp.git", rev = "abc1234" }    # pin to commit
```

`branch`, `tag`, and `rev` are mutually exclusive; `rev` pins the commit and blocks `freight update`.
Clones the repo into `.pkgs/<name>/`, then treats it exactly like a path dep — foreign build
system detection applies. Run `freight fetch` to clone before building.

### URL archive dependency

```toml
zlib = { url = "https://zlib.net/zlib-1.3.1.tar.gz" }
# with SHA-256 verification (recommended):
zlib = { url = "https://zlib.net/zlib-1.3.1.tar.gz", sha256 = "9a93b2b7..." }
# any scheme curl supports — ftp works too:
mylib = { url = "ftp://ftp.example.com/pub/mylib-2.0.tar.gz" }
# GitHub release archives are just URLs:
json = { url = "https://github.com/nlohmann/json/archive/refs/tags/v3.11.3.tar.gz" }
```

Downloads the archive using `curl` (supports `https://`, `http://`, `ftp://`, and any other scheme
curl handles), optionally verifies SHA-256, extracts to `.pkgs/<name>/` with `--strip-components=1`,
then auto-detects the build system or treats as header-only if no source files are found. The
sentinel `.pkgs/<name>/.freight-fetched` prevents re-downloading; `freight update <name>` invalidates it.

For GitHub repos specifically: if you need to track a branch or make incremental updates, prefer a
`.git` URL with a `branch`/`tag`/`rev` (see above). A plain `url` is for pinned release tarballs.

### Foreign build system options

Any dep with a source (path, git, or archive url) supports these additional keys:

```toml
dep = {
    path        = "../dep",
    type        = "cmake",               # cmake | make | meson | autotools | scons | bazel | none
    defines     = ["BUILD_TESTS=OFF"],   # configure defines, applied per builder (cmake/meson -D,
                                         #   make KEY=VALUE); a leading -D is accepted.
                                         #   aliases: cmake-args / cmake_args
    include     = ["include/", "src/"],  # explicit include dirs (skips auto-detection)
}
```

`type` is optional — freight auto-detects the build system from marker files in the dep directory
(`CMakeLists.txt` → cmake, `meson.build` → meson, `configure.ac` → autotools, `Makefile` → make, etc.).
Specifying an explicit `type` when the required marker file is absent is an error.

`type = "none"` skips the build entirely — useful when you want to explicitly declare a
header-only dep or a prebuilt binary tarball. Freight also auto-detects header-only deps: if no
compilable source files are found after fetching, the build step is skipped and include dirs are
collected automatically.

**CMake build details** — when `type = "cmake"` (or auto-detected):
- Ninja is used as the generator when `ninja` is on `$PATH`; otherwise CMake's default (Unix Makefiles) is used.
- When `[compiler] target` is set, `-DCMAKE_SYSTEM_NAME` and `-DCMAKE_SYSTEM_PROCESSOR` are injected automatically from the target triple.
- Parallel builds via `cmake --build --parallel N` on CMake ≥ 3.12.
- `cmake --install` installs built artifacts to `.freight-build/install/` so headers and archives are always found at a predictable path.
- Additional configure defines from `defines` are forwarded to the configure step as `-D<KEY=VALUE>`.

**Autotools build details** — when `type = "autotools"` (or auto-detected):
- When `[compiler] target` is set, `--host=<triple>` is passed to `configure` automatically.
- Configure is skipped when `config.status` and `Makefile` are already present and `configure` has not been modified since the last configure run (fast-build).
- `--enable-static --disable-shared` is always passed for predictable static archive output.
- `make -j{N}` runs with all available CPU cores.
- For wasm/Emscripten targets, `emconfigure` and `emmake` are used in place of `configure` and `make`.

### Dependency filters

Any dep can be gated by target triple, OS, or CPU architecture. Deps that do not match the current
build context are excluded from compilation and linking.

```toml
# Only included when cross-compiling to this target triple
arm-hal = { path = "../arm-hal", targets = ["aarch64-linux-gnu"] }

# Versionless system libraries are linked via `[os.*] features`, not a dep entry
# (see the [os.*] / [arch.*] section below).

# Only included when cross-compiling to this target triple / matching CPU arch
sse-util = { path = "../sse-util", arch = "x86_64" }

# Combine OS + arch (both must match)
avx-opt = { path = "../avx-opt", os = "linux", arch = ["x86_64", "aarch64"] }
```

---

## `[patch]`

Override where a dependency's source comes from — anywhere in the dependency graph,
including **transitive** deps. Useful for testing a local fix to an upstream library
without editing the dep that pulls it in.

```toml
[dependencies]
app-core = { path = "../app-core" }   # app-core itself depends on "json"

[patch]
# Build against a local checkout of "json" instead of the version app-core
# (or any dep) declares. Paths are relative to *this* manifest's directory.
json = { path = "../json-fork" }
```

A matching dep name resolves to the patched source instead of its original
location. Patches are read from the **root** project's manifest only — a `[patch]`
in a dependency's own manifest is ignored. Each entry must be a **path** override;
version, git, and archive overrides are rejected at validation. Patched deps are
skipped by `freight fetch` (the source is already local).

---

## `[workspace]`

A workspace-root `freight.toml` contains **only** a `[workspace]` section (no
`[package]`). Members are ordinary freight projects listed by relative path.

```toml
[workspace]
members = ["app", "libfoo", "libbar"]

# Shared dependency definitions. Members opt in per-dep with `{ workspace = true }`.
[workspace.dependencies]
fmt  = ">=10.0"
spdlog = { version = "1.13", features = ["std-format"] }

# Shared [package] field defaults. Members opt in per-field with `field.workspace = true`.
[workspace.package]
version = "1.4.0"
license = "Apache-2.0"
authors = ["ACME <dev@acme.example>"]
```

### Inheritance from a member

A member pulls shared values in with the `workspace = true` marker:

```toml
[package]
name             = "app"
version.workspace = true      # ← from [workspace.package].version
license.workspace = true

[dependencies]
fmt    = { workspace = true }                       # inherit verbatim
spdlog = { workspace = true, features = ["async"] } # inherit + add a feature
```

Rules:
- **Package fields** — any `[package]` field can be `field.workspace = true`; the
  value is taken from `[workspace.package].<field>`. Missing there → error.
- **Dependencies** — `name = { workspace = true }` in `[dependencies]`,
  `[build-dependencies]`, or `[dev-dependencies]` inherits from
  `[workspace.dependencies].<name>`. The member may add `features` (unioned with
  the workspace entry's) and override `optional` / `default-features`. Missing in
  `[workspace.dependencies]` → error.
- Inheritance is resolved against the nearest ancestor directory whose
  `freight.toml` has a `[workspace]` section. A marker with no such ancestor is an
  error.

---

## `[features]`

Cargo-style conditional compilation. Active features produce `-D<NAME_UPPER>` flags for all
compiled sources. Defines are **per-package** — they only ever land in one package's
compilation, never globally.

```toml
[features]
default = ["logging"]  # active unless overridden; "default" itself never produces -DDEFAULT
logging = []           # → -DLOGGING
tls     = ["net"]      # → -DTLS, also activates "net"
net     = []           # → -DNET
```

Besides another feature name, a feature-list entry can be one of:

```toml
[features]
spdlog = ["dep:spdlog"]                       # activate optional dependency "spdlog" (no define)
fast   = ["define:NDEBUG", "define:LEVEL=3"]  # inject -DNDEBUG / -DLEVEL=3 into THIS package
crypto = ["openssl/define:WITH_TLS"]          # inject -DWITH_TLS into openssl's build (and
                                              #   activate openssl if it is optional)
extra  = ["openssl?/define:WITH_EXTRA"]       # weak: inject into openssl only if it is already active
```

- `dep:name` — activate optional dependency `name` (no define), mirroring Cargo's `dep:` syntax.
- `define:NAME` / `define:NAME=value` — inject an explicit `-DNAME` / `-DNAME=value` into the
  current package (the `=value` is optional).
- `<dep>/define:NAME[=value]` — forward the explicit define into **dependency `<dep>`'s** build
  instead of this package, mirroring Cargo's `<dep>/<feature>` syntax. Activates `<dep>` if optional.
- `<dep>?/define:NAME[=value]` — the weak form: forwards only when `<dep>` is already activated by
  something else; never activates it itself.

Consumers of a library dep can select features:

```toml
mylib = { path = "../mylib", features = ["tls"] }
mylib = { path = "../mylib", default-features = false, features = ["net"] }
```

Features are transitively expanded (BFS). Cycles are a validation error.

Changing the active feature set (or any compile flag) invalidates the affected package's object
cache, so incremental builds recompile exactly the packages whose flags changed.

---

## `[compiler]`

Global compiler settings. All fields are optional.

```toml
[compiler]
backend   = "auto"                # compiler to use: auto | gcc | clang | gfortran | nasm | …
opt-level = 2                     # 0 | 1 | 2 | 3
debug     = false                 # emit debug symbols (-g)
warnings  = "all"                 # none | default | all | error
defines   = ["USE_BLAS", "FOO=1"] # extra -D flags
flags     = ["-march=native"]     # verbatim extra flags appended to every compile invocation
includes  = ["include/", "third-party/include/"]  # extra -I directories
target    = "aarch64-linux-gnu"   # cross-compilation target triple
sysroot   = "/opt/sysroot"        # sysroot path for cross-compilation
```

`backend = "auto"` selects the first detected compiler whose template handles the project's source
languages. Override with an explicit name (e.g. `"clang"`) to pin a specific toolchain.

### Cross-compilation

`target` passes the active target triple to compilers that support a target flag.
`sysroot` passes `--sysroot={path}` to compilers that support it. If the `FREIGHT_SYSROOT` environment variable is set, it supplies the same sysroot automatically and takes precedence over the global config value for the current invocation.

When both `target` and `sysroot` are configured, freight also derives conservative CPU tuning flags
(such as `-march=`, `-mcpu=`, or `-mtune=`) from the target/sysroot pair unless auto CPU tuning is disabled in `~/.freight/config.toml`.

Deps with `targets = [...]` lists are filtered: a dep is included only when its target list
contains the active `compiler.target` value (or always when no target list is set).

---

## `[target]`

Hardware-specific flags. Drives `[arch_flags]` lookups and `-m<ext>` flag generation.

```toml
[target]
arch           = "x86_64"           # overrides host arch for template [arch_flags] lookup
cpu-extensions = ["avx2", "fma"]    # → -mavx2 -mfma  (template: cpu_extension = "-m{name}")
```

`arch` defaults to `std::env::consts::ARCH`. It is used by assembler templates to select the
correct output format (e.g. NASM `-f elf64` vs `-f macho64` vs `-f win64`).

**`cpu-extensions` vs `[arch.*] features`** — both enable CPU/ISA extensions, but:

- **`[arch.<arch>] features`** (preferred) is *arch-gated*, *data-driven* (resolved through
  `cpu-features.toml`: e.g. `sve` → `-march=armv8-a+sve`, same-base `-march` flags merged), and
  *header-aware* (unlocks the feature's intrinsic headers for include hygiene). Use this for SIMD.
- **`[target] cpu-extensions`** is the *unconditional* (all-arch) form, applied through the active
  **compiler template's** `cpu_extension` pattern — so it is per-compiler (`-m{name}`, empty for
  assemblers, etc.). Kept for the global case; reach for `[arch.*] features` otherwise.

---

## `[profile.<name>]`

Named build profiles override `[compiler]` settings. Two profiles are always available: `dev`
(default for `freight build`) and `release` (selected with `--release`). Custom profile names are
reserved for future use.

```toml
[profile.dev]
opt-level = 0
debug     = true
sanitize  = ["address", "undefined"]  # comma-separated list of sanitizers

[profile.release]
opt-level = 3
lto       = true   # link-time optimisation (-flto)
strip     = true   # strip debug symbols from output (-s)
debug     = false
```

Sanitizers are passed as `-fsanitize=address,undefined`. Supported values depend on the compiler;
common choices: `address`, `undefined`, `thread`, `memory`, `leak`.

---

## `[os.<key>]` and `[arch.<key>]`

Platform-conditional sections. Each section is applied only when the host OS or CPU architecture
matches the key. Non-matching sections are completely ignored — their source files are excluded
from the build, not just skipped at compile time.

### OS keys

Recognized OS keys: `linux`, `windows`, `macos`, `freebsd`, `openbsd`, `netbsd`, `dragonfly`,
`android`, `ios`, `solaris`, `illumos`. Family aliases: `unix` (everything except Windows) and
`bsd` (all BSDs). When both a family and a specific OS match (e.g. Linux), both sections are
applied — family first, then specific OS.

### Arch keys

Recognized arch keys match `std::env::consts::ARCH`: `x86_64`, `aarch64`, `x86`, `arm`,
`riscv64`, `powerpc64`, `s390x`, `wasm32`, and others.

### Fields

All fields are optional within each section:

| Field | Description |
|---|---|
| `srcs` | Glob patterns relative to the project root. Matched files are added to the build; files listed in any `[os.*]`/`[arch.*]` section are excluded from the unconditional `src/` walk. |
| `defines` | Extra `-D` flags applied only on this platform. |
| `flags` | Extra compiler flags applied only on this platform. |
| `includes` | Extra include paths (`-I`) applied only on this platform. |
| `features` | <a id="osplatform-features"></a>Context-dependent. In `[os.*]`: **system libraries** to link — each name resolves through the system-lib stub table to `-l<lib>` (macOS frameworks → `-framework`, MSVC → `<name>.lib`); unknown names fall back to `-l<name>`. In `[arch.*]`: **CPU/ISA features** to enable — each resolves through the cpu-features table to a compiler flag (`avx2` → `-mavx2`, `sve` → `-march=armv8-a+sve`; unknown → `-m<name>`) and unlocks that feature's intrinsic headers for include hygiene. |
| `version` | Minimum target OS / SDK version. On Apple targets → `-mmacosx-version-min` / `-miphoneos-version-min`; always exposed to source as `-DFREIGHT_OS_VERSION="<v>"`. |
| `dependencies` | Inline dependency table — same syntax as `[dependencies]`. |
| `language` | Per-language overrides — same keys as `[language.<key>]`. |

```toml
[os.linux]
srcs     = ["src/os/linux/**"]
defines  = ["PLATFORM_LINUX", "POSIX_BUILD"]
flags    = ["-fvisibility=hidden"]
includes = ["/usr/local/include"]
features = ["m", "pthread"]                      # -lm -lpthread

[os.windows]
srcs     = ["src/os/windows/**"]
defines  = ["WIN32_LEAN_AND_MEAN", "PLATFORM_WINDOWS"]
features = ["ws2_32"]                            # -lws2_32

[os.macos]
version  = "11.0"                                # -mmacosx-version-min=11.0
features = ["Foundation"]                        # -framework Foundation

[os.unix]
defines = ["POSIX_BUILD"]

[arch.x86_64]
srcs     = ["src/arch/x86_64/**"]
defines  = ["HAVE_SSE2"]
features = ["avx2", "fma"]                       # -mavx2 -mfma; unlocks <immintrin.h>

[arch.aarch64]
srcs     = ["src/arch/aarch64/**"]
defines  = ["HAVE_NEON"]
features = ["sve"]                               # -march=armv8-a+sve; unlocks <arm_sve.h>
```

CPU-feature names come from a bundled `cpu-features.toml` (link name/flag, arch, and the
intrinsic headers each unlocks); add or override entries with `.toml` files in
`$FREIGHT_HOME/toolchains/cpu-features/`. A feature declared under an `[arch.*]` section it
doesn't belong to (e.g. `avx2` under `[arch.aarch64]`) is a validation error.

Additive `-m<ext>` flags (AVX, SSE, FMA, …) simply stack. `-march=<base>+<ext>` features that
share a base are merged into one flag (`sve` + `sve2` → `-march=armv8-a+sve+sve2`) so they don't
clobber each other under the compiler's last-`-march`-wins rule; `-march`/`-mcpu`/`-mtune`/… flags
with genuinely different values that can't be merged are kept as-is and reported as a build warning.

Files matched by `srcs` globs in any `[os.*]` or `[arch.*]` section are automatically
excluded from the unconditional source walk — they will never be compiled on a non-matching
platform, even if they live under `src/`.

---

## `[formatter]`

Optional. Declares the project's preferred formatter and its settings. When absent, `freight fmt`
auto-selects the first formatter found on PATH.

```toml
[formatter]
name   = "clang-format"   # optional — pin a specific formatter; auto-detected when absent
style  = "Google"         # tool setting → --style=Google
# config = ".clang-format" # alternative: point to a config file → --style=file:.clang-format
```

The setting keys (`style`, `config`, …) come from the selected formatter's template. Unknown keys
are silently ignored. On first run without any config, `freight fmt` prints a hint listing the
available settings and valid values for the detected tool.

```sh
freight fmt           # format all source files in-place
freight fmt --check   # exit non-zero if any file needs reformatting (CI use)
```

| Formatter | Settings |
|---|---|
| `clang-format` | `style` (Google \| LLVM \| Mozilla \| WebKit \| Chromium \| Microsoft \| GNU \| file), `config` |
| `astyle` | `style` (allman \| java \| kr \| stroustrup \| google \| mozilla \| otbs \| vtk \| ratliff \| lisp), `indent` |
| `uncrustify` | `config` |
| `fprettify` | `indent`, `config` |

---

## IDE / LSP

`freight lsp` runs a stdio language server for editors. It owns `freight.toml`
diagnostics, completion, and hover, and can start source-file language servers
as passthroughs:

| Language | Passthrough |
|---|---|
| C / C++ / CUDA / HIP / Objective-C / Objective-C++ | `clangd` |
| Fortran | `fortls` |
| Assembly (`.asm`, `.nasm`, `.s`) | `asm-lsp` |

On initialize, when `freight.toml` is saved, and when the editor reports an
external `freight.toml` file change, the server refreshes a source LSP compile
database at `.freight/lsp/<profile>/compile_commands.json`. `clangd` is launched
with `--compile-commands-dir=.freight/lsp/<profile>`, so the editor does not need
a project-root `compile_commands.json` in the explorer. The explicit
`freight compile-commands` command still writes the project-root file for users
and tools that ask for it.

The generated database is manifest-aware. Source language servers should see
only include paths and sources reachable from packages explicitly declared in
`freight.toml` and active for the current OS, architecture, target triple,
profile, and feature set. Installed system packages that are not listed in the
manifest are not added to the source LSP search surface.

```sh
freight lsp                    # freight.toml helper + source LSP passthroughs
freight lsp --no-clangd        # disable C-family passthrough
freight lsp --clangd /path/to/clangd
freight lsp --fortls /path/to/fortls
freight lsp --asm-lsp /path/to/asm-lsp
freight lsp --no-clangd --no-fortls --no-asm-lsp  # manifest-only mode
freight lsp --profile release  # generate release LSP compile database
```

---

## `[linter]`

Optional. Declares the project's preferred linter and its settings. When absent, `freight lint`
auto-selects the first linter found on PATH.

```toml
[linter]
name   = "clang-tidy"              # optional — pin a specific linter; auto-detected when absent
checks = "-*,modernize-*,bugprone-*"  # tool setting → --checks=...
# config = ".clang-tidy"            # alternative: point to a config file
```

```sh
freight lint          # run static analysis, report issues
freight lint --fix    # apply safe auto-fixes where the tool supports them
```

| Linter | Settings |
|---|---|
| `clang-tidy` | `checks`, `config` |
| `cppcheck` | `enable` (all \| warning \| style \| performance \| portability \| information), `std`, `suppress`, `jobs` |
| `cpplint` | `filter`, `linelength`, `root` |
| `flawfinder` | `minlevel` (0–5) |

Tools without a native auto-fix mode (`cppcheck`, `cpplint`, `flawfinder`) treat `freight lint --fix`
identically to `freight lint`.

---

## `[lints]`

Optional. Configures Freight's own lints. See
[`include-hygiene.md`](include-hygiene.md) for the include-hygiene design.

```toml
[lints]
undeclared-include = "warn"   # "allow" | "warn" | "deny"  (default: "warn")
```

| Lint | Levels | Meaning |
|---|---|---|
| `undeclared-include` | `allow` \| `warn` \| `deny` | Report an `#include` that resolves to a header provided by **no declared package**. |

`undeclared-include` flags any `#include` that resolves outside the project and
its declared dependencies and is **not** a language standard-library header. The
standard library is recognised by header name and so passes on every platform;
POSIX/OS headers (`<unistd.h>`, `<pthread.h>`, `<windows.h>`, …) are *not* part of
it and must be provided by a declared dependency (e.g. a `system` dep or an
`[os.*]` section). In the editor the warning appears inline on the `#include`;
`deny` raises it to an error. Defaults to `warn` even when `[lints]` is absent.

---

## Developer config — outside `freight.toml`

Toolchain selection, debugger preferences, and cross-compilation settings are **developer
concerns**, not project concerns. They live outside `freight.toml` and are not committed as
part of the project:

| File | Scope | Description |
|---|---|---|
| `~/.freight/config.toml` | global | Machine-wide developer defaults (toolchain, debugger, sysroot) |
| `<project>/.freight/config.toml` | local | Project-specific developer overrides; can be committed or gitignored |

Local config overrides global. Both share the same format:

```toml
# ~/.freight/config.toml  or  <project>/.freight/config.toml
default_backend = "clang"       # preferred compiler family
default_debugger = "lldb"       # preferred debugger backend
target          = "aarch64-linux-gnu"  # cross-compilation target triple
sysroot         = "/opt/sysroot"
auto-cpu-tuning = true          # set false to suppress derived -march/-mcpu/-mtune flags

[debugger.gdb]
args  = ["--tui"]   # raw extra flags before the program separator
tui   = true        # resolved via gdb.rhai's settings map → --tui
quiet = true

[debugger.lldb]
no_use_colors = true
```
