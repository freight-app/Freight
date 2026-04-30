# crane.toml — Manifest Reference

A `crane.toml` file at the root of a project describes everything crane needs to build, test, and
package it. Every field is optional unless noted as required.

---

## `[package]`

Required. Identifies the project.

```toml
[package]
name        = "myproject"          # required — lowercase, hyphens allowed
version     = "0.1.0"             # required — semver string
authors     = ["Alice <a@b.com>"]  # optional — shown in crane info
description = "Short description"  # optional
license     = "MIT"                # optional — SPDX identifier
```

---

## `[language.<key>]`

Per-language compiler settings. The key must match a `[linking.<key>]` name in a loaded compiler
template (e.g. `cpp`, `c`, `fortran`, `asm`).

```toml
[language.cpp]
std = "c++20"   # compiler standard passed as -std=... ; values come from [standards] in the template

[language.c]
std = "c17"

[language.fortran]
# std is optional for Fortran
```

Available standard strings depend on the loaded template. Common values:

| Key | Typical values |
|---|---|
| `cpp` | `c++11`, `c++14`, `c++17`, `c++20`, `c++23` |
| `c` | `c11`, `c17`, `c23` |
| `fortran` | `f2003`, `f2008`, `f2018` |

---

## `[lib]`

Declares a library output. Omit this section for binary-only projects.

```toml
[lib]
type    = "static"     # required — static | shared | header-only
src     = "src/"       # directory containing source files (default: "src/")
include = "include/"   # public include directory exposed to dependents
```

`header-only` libraries skip compilation entirely — crane records the include path so dependents
can use it, but no `.a` or `.so` is produced.

---

## `[[bin]]`

Declares a binary target. Repeat the section for multiple binaries.

```toml
[[bin]]
name = "mytool"       # required — output binary name
src  = "src/main.cpp" # entry-point source file (default: "src/main.cpp")
```

When multiple `[[bin]]` sections are present, each binary is compiled from its own entry-point
source plus any shared sources discovered in the project's source tree. Linker deduplication
ensures that `main()` from one binary is not linked into another.

---

## `[dependencies]` and `[dev-dependencies]`

Dependencies listed under `[dependencies]` are always compiled and linked. Those under
`[dev-dependencies]` are only linked when running `crane test`.

### Version dependency (registry)

```toml
mylib = "1.2"          # resolved from crane.dev; exact pinning via crane.lock
mylib = ">=1.0, <2.0"  # semver range
```

> Registry resolution is not yet implemented. Version deps produce a clear "registry not yet available" message.

### Path dependency

```toml
myutils = { path = "../myutils" }
```

Compiles the crane project at the given path and links its library archive. The dep's `include/`
directory is added to the include path automatically. Path deps with a `crane.toml` are treated as
crane projects; those without are treated as foreign build systems (see below).

### System dependency

```toml
openssl = { system = "ssl" }            # → -lssl
pthread = { system = "pthread" }
```

Passes `-l{name}` to the linker. No source build; assumes the library is installed on the system.

### System + pkg-config

```toml
# pkg-config only — error if pkg-config is not found
zlib = { pkg_config = "zlib" }

# Combined — pkg-config first, bare -l{name} fallback if pkg-config fails
zlib = { system = "z", pkg_config = "zlib" }
```

`pkg_config` runs `pkg-config --cflags --libs <query>` and injects the resulting include dirs
(`-I`) into compilation and link flags (`-L`, `-l`, `-pthread` etc.) verbatim into the linker
command. The query string is passed as-is to pkg-config, so version constraints work:
`"glib-2.0 >= 2.56"`.

When both `system` and `pkg_config` are set, pkg-config is tried first. If it fails (not installed
or package not found), crane falls back to `-l{system}` and prints a warning.

### Git dependency

```toml
easyloggingpp = { git = "https://github.com/amrayn/easyloggingpp" }
easyloggingpp = { git = "https://...", tag = "v9.97.1" }   # pin to tag
easyloggingpp = { git = "https://...", branch = "main" }   # track branch
easyloggingpp = { git = "https://...", rev = "abc1234" }   # pin to commit
```

Clones the repo into `.deps/<name>/`, then treats it exactly like a path dep — foreign build
system detection applies. Run `crane fetch` to clone before building.

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
curl handles), optionally verifies SHA-256, extracts to `.deps/<name>/` with `--strip-components=1`,
then auto-detects the build system or treats as header-only if no source files are found. The
sentinel `.deps/<name>/.crane-fetched` prevents re-downloading; `crane update <name>` invalidates it.

For GitHub repos specifically: if you need to track a branch or make incremental updates, prefer
`git = "https://github.com/..."` instead. `url` is for pinned release tarballs.

### Foreign build system options

Any dep with a source (path, git, http, github) supports these additional keys:

```toml
dep = {
    path         = "../dep",
    build_system = "cmake",              # cmake | make | meson | autotools | scons | none | auto
    cmake_args   = ["-DBUILD_TESTS=OFF"], # extra args forwarded to cmake configure step
    include      = ["include/", "src/"], # explicit include dirs (skips auto-detection)
}
```

`build_system = "none"` skips the build entirely — useful when you want to explicitly declare a
header-only dep. Crane also auto-detects header-only deps: if no compilable source files are found
after fetching, the build step is skipped and include dirs are collected automatically.

### Dependency filters

Any dep can be gated by target triple, OS, or CPU architecture. Deps that do not match the current
build context are excluded from compilation and linking.

```toml
# Only included when cross-compiling to this target triple
arm-hal = { path = "../arm-hal", targets = ["aarch64-linux-gnu"] }

# Only linked on matching host OS
# Accepted values: linux, windows, macos, freebsd, openbsd, netbsd, dragonfly,
#                  android, ios, solaris, illumos, unix (family), bsd (family)
pthread = { system = "pthread", os = "linux" }
libm    = { system = "m",       os = ["linux", "macos"] }

# Only linked on matching CPU architecture (std::env::consts::ARCH)
sse-util = { path = "../sse-util", arch = "x86_64" }

# Combine OS + arch (both must match)
avx-opt = { system = "avx-opt", os = "linux", arch = ["x86_64", "aarch64"] }
```

---

## `[features]`

Cargo-style conditional compilation. Active features produce `-D<NAME_UPPER>` flags for all
compiled sources.

```toml
[features]
default = ["logging"]  # active unless overridden; "default" itself never produces -DDEFAULT
logging = []           # → -DLOGGING
tls     = ["net"]      # → -DTLS, also activates "net"
net     = []           # → -DNET
```

Consumers of a library dep can select features:

```toml
mylib = { path = "../mylib", features = ["tls"] }
mylib = { path = "../mylib", default-features = false, features = ["net"] }
```

Features are transitively expanded (BFS). Cycles are a validation error.

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
target    = "aarch64-linux-gnu"   # cross-compilation target triple
sysroot   = "/opt/sysroot"        # sysroot path for cross-compilation

[compiler.includes]
paths = ["include/", "third_party/include/"]  # extra -I directories
```

`backend = "auto"` selects the first detected compiler whose template handles the project's source
languages. Override with an explicit name (e.g. `"clang"`) to pin a specific toolchain.

### Cross-compilation

`target` passes `--target={triple}` to compilers that support it (clang, gfortran, hipcc, icpx).
GCC requires a dedicated cross-compilation binary (e.g. `aarch64-linux-gnu-gcc`) — set
`backend = "aarch64-linux-gnu-gcc"` and leave `target` empty for GCC.

`sysroot` passes `--sysroot={path}` to compilers that support it.

Deps with `targets = [...]` lists are filtered: a dep is included only when its target list
contains the active `compiler.target` value (or always when no target list is set).

---

## `[target]`

Hardware-specific flags. Drives `[arch_flags]` lookups and `-m<ext>` flag generation.

```toml
[target]
arch           = "x86_64"           # overrides host arch for template [arch_flags] lookup
cpu_extensions = ["avx2", "fma"]    # → -mavx2 -mfma  (template: cpu_extension = "-m{name}")
```

`arch` defaults to `std::env::consts::ARCH`. It is used by assembler templates to select the
correct output format (e.g. NASM `-f elf64` vs `-f macho64` vs `-f win64`).

---

## `[profile.<name>]`

Named build profiles override `[compiler]` settings. Two profiles are always available: `dev`
(default for `crane build`) and `release` (selected with `--release`). Custom profile names are
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

## `[platform.<os>]`

Per-OS overlays merged into the build at runtime. Use for OS-specific dependencies and compiler
flags without cluttering the main sections.

Recognized keys: `linux`, `windows`, `macos`, `freebsd`, `openbsd`, `netbsd`, `dragonfly`,
`android`, `ios`, `solaris`, `illumos`, plus the family aliases `unix` (everything except
Windows) and `bsd` (all BSDs). Family overlays are applied before the specific OS, so a Linux
build picks up `[platform.unix]` then `[platform.linux]`.

```toml
[platform.linux.dependencies]
dl      = { system = "dl" }
pthread = { system = "pthread" }

[platform.windows.dependencies]
ws2_32  = { system = "ws2_32" }

[platform.windows.compiler]
defines = ["WIN32_LEAN_AND_MEAN"]

[platform.unix.compiler]
defines = ["POSIX_BUILD"]
flags   = ["-fvisibility=hidden"]

[platform.linux.compiler.includes]
paths = ["/usr/local/include"]
```

Overlayable fields: `dependencies`, `compiler.defines`, `compiler.flags`, `compiler.includes.paths`.
`[[bin]]`, `[language]`, `[lib]`, profiles, and sanitizers are not overlayable in v1.
