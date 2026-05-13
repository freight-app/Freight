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
supports    = "(windows & !uwp & (x86 | x64)) | (!windows & !osx)"
```

`supports` is optional. When present, it is a vcpkg-style boolean platform expression that
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

---

## `[lib]`

Declares a library output. Omit this section for binary-only projects.

```toml
[lib]
type    = "static"     # required — static | shared | header-only
src     = "src/"       # directory containing source files (default: "src/")
include = "include/"   # public include directory exposed to dependents
```

`header-only` libraries skip compilation entirely — freight records the include path so dependents
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
`[dev-dependencies]` are only linked when running `freight test`.

### Version dependency (registry)

```toml
mylib = "1.2"          # resolved from freight.dev; exact pinning via freight.lock
mylib = ">=1.0, <2.0"  # semver range
```

> Registry resolution is not yet implemented. Version deps produce a clear "registry not yet available" message.

### Path dependency

```toml
myutils = { path = "../myutils" }
```

Compiles the freight project at the given path and links its library archive. The dep's `include/`
directory is added to the include path automatically. Path deps with a `freight.toml` are treated as
freight projects; those without are treated as foreign build systems (see below).

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
or package not found), freight falls back to `-l{system}` and prints a warning.

### Git dependency

```toml
easyloggingpp = { git = "https://github.com/amrayn/easyloggingpp" }
easyloggingpp = { git = "https://...", tag = "v9.97.1" }   # pin to tag
easyloggingpp = { git = "https://...", branch = "main" }   # track branch
easyloggingpp = { git = "https://...", rev = "abc1234" }   # pin to commit
```

Clones the repo into `.deps/<name>/`, then treats it exactly like a path dep — foreign build
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
curl handles), optionally verifies SHA-256, extracts to `.deps/<name>/` with `--strip-components=1`,
then auto-detects the build system or treats as header-only if no source files are found. The
sentinel `.deps/<name>/.freight-fetched` prevents re-downloading; `freight update <name>` invalidates it.

For GitHub repos specifically: if you need to track a branch or make incremental updates, prefer
`git = "https://github.com/..."` instead. `url` is for pinned release tarballs.

### Foreign build system options

Any dep with a source (path, git, http, github) supports these additional keys:

```toml
dep = {
    path       = "../dep",
    backend    = "cmake",               # cmake | make | meson | autotools | scons | bazel | none
    cmake_args = ["-DBUILD_TESTS=OFF"], # extra args forwarded to cmake configure step
    include    = ["include/", "src/"],  # explicit include dirs (skips auto-detection)
}
```

`backend` is optional — freight auto-detects the build system from marker files in the dep directory
(`CMakeLists.txt` → cmake, `meson.build` → meson, `configure.ac` → autotools, `Makefile` → make, etc.).
Specifying an explicit `backend` when the required marker file is absent is an error.

`backend = "none"` skips the build entirely — useful when you want to explicitly declare a
header-only dep. Freight also auto-detects header-only deps: if no compilable source files are found
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

`target` passes the active target triple to compilers that support a target flag.
`sysroot` passes `--sysroot={path}` to compilers that support it.

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
cpu_extensions = ["avx2", "fma"]    # → -mavx2 -mfma  (template: cpu_extension = "-m{name}")
```

`arch` defaults to `std::env::consts::ARCH`. It is used by assembler templates to select the
correct output format (e.g. NASM `-f elf64` vs `-f macho64` vs `-f win64`).

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
| `sources` | Glob patterns relative to the project root. Matched files are added to the build; files listed in any `[os.*]`/`[arch.*]` section are excluded from the unconditional `src/` walk. |
| `defines` | Extra `-D` flags applied only on this platform. |
| `flags` | Extra compiler flags applied only on this platform. |
| `includes` | Extra include paths (`-I`) applied only on this platform. |
| `dependencies` | Inline dependency table — same syntax as `[dependencies]`. |
| `language` | Per-language overrides — same keys as `[language.<key>]`. |

```toml
[os.linux]
sources      = ["src/os/linux/**"]
defines      = ["PLATFORM_LINUX", "POSIX_BUILD"]
flags        = ["-fvisibility=hidden"]
includes     = ["/usr/local/include"]
dependencies = { m = { system = "m" }, pthread = { system = "pthread" } }

[os.windows]
sources      = ["src/os/windows/**"]
defines      = ["WIN32_LEAN_AND_MEAN", "PLATFORM_WINDOWS"]
dependencies = { ws2_32 = { system = "ws2_32" } }

[os.unix]
defines = ["POSIX_BUILD"]

[arch.x86_64]
sources = ["src/arch/x86_64/**"]
defines = ["HAVE_SSE2"]

[arch.aarch64]
sources = ["src/arch/aarch64/**"]
defines = ["HAVE_NEON"]
```

Files matched by `sources` globs in any `[os.*]` or `[arch.*]` section are automatically
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
