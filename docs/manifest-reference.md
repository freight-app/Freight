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

`auto-discover` (default `true`) controls the zero-config `src/` walk. When `false`,
freight compiles **only** the files explicitly listed in `[lib].srcs` / `[[bin]].src`
(plus `[os.*]`/`[arch.*]` sources) — the `src/` tree is not auto-walked. This is the
blunt "I list every source" switch; for dropping just a few files while keeping the
walk, prefer a `!` negation in `[lib].srcs` (above).

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

> **Built-in protobuf codegen was removed.** It's now expressed as a **build
> plugin** (see below) rather than a special `[language.proto]` language key — so
> protobuf, Qt `moc`/`uic`, FlatBuffers, shader compilers, `config.h`, etc. all
> use one mechanism instead of being baked into the core.

---

## `[plugin]` and build plugins

A **plugin** is an ordinary dependency that runs a script during your build. A
package becomes a plugin by declaring `[plugin]`:

```toml
# the plugin package's freight.toml
[package]
name    = "proto"
version = "0.1.1"

[plugin]
entry   = "proto.freight"     # Rhai script, relative to the package root
handles = ["proto"]        # sections it activates on (defaults to the package name)
tools   = ["protoc"]       # external tools the script may run (allow-list)

# Activation conditions (all optional):
goals    = ["build", "test"]   # build goals that trigger it (default: all)
profiles = ["debug"]           # profiles that trigger it (default: all)
inputs   = ["src/**/*.proto"]  # re-run only when these change (else reuse output)

# Advisory editor schema (optional): keys the consumer's section understands.
[plugin.schema]
proto_path = "Import root passed to protoc (default: src/)."
grpc       = "Generate gRPC service stubs (bool)."
```

The plugin runs when the consumer declares a matched `handles` section **and**
every activation condition holds:

- **`goals`** — only on these build goals (`build` / `test` / `bench` /
  `examples`). e.g. a fixture generator that runs only on `test`.
- **`profiles`** — only on these profiles. e.g. `profiles = ["debug"]` to enable
  something only in debug builds.
- **`inputs`** — incremental: freight fingerprints these files (plus `CFG` and
  the script); if nothing changed since the last run, the previously generated
  output is reused instead of re-running the tool (so editing a `.proto`
  re-runs `protoc`, but an unrelated edit doesn't).

`handles` entries are matched against the **dotted path** of every section the
consumer declares, so a plugin can target nested config too:

| Pattern | Matches |
|---|---|
| `proto` | top-level `[proto]` |
| `compiler.clang` | exactly `[compiler.clang]` |
| `compiler.*` | any one-level `[compiler.<x>]` |
| `language.**` | `[language.<x>]` and deeper (one or more segments) |

The plugin runs **once per matched section**; the matched path is available in
the script as `section` (e.g. `"compiler.clang"`).

You **consume** it like any dependency, and add the section it handles:

```toml
# your project's freight.toml
[dependencies]
proto = "0.1.1"

[proto]                    # plain config data, handed to the plugin as `CFG`
out = "src/generated"
```

During the build, the plugin's script runs with these **constants** in scope:

| Constant | Value |
|---|---|
| `SECTION` | the matched section's dotted path (e.g. `"proto"`, `"compiler.clang"`) |
| `PROJECT_DIR` | the project root |
| `SRC_DIR` | `<project>/src` |
| `INCLUDE_DIR` | `<project>/include` |
| `TARGET_DIR` | `<project>/target/<profile>` |
| `OUT_DIR` | this plugin's output dir, `TARGET_DIR/plugin-gen/<section>` (created for you) |
| `PROFILE` | the build profile (`"debug"`, `"release"`, or a custom name) — branch on `PROFILE == "release"` |
| `HOST` | host characteristics: `HOST.os`, `HOST.arch`, `HOST.family` (`"windows"`/`"unix"`/`"wasm"`), `HOST.pointer_width` |
| `TARGET` | target characteristics: `TARGET.os`, `TARGET.arch`, `TARGET.family`, `TARGET.pointer_width`, `TARGET.triple` (`""` when building for the host, the full triple when cross-compiling) |
| `LIB` | the consuming project's library, `#{ name, type, hdrs, srcs, link }` — or `()` when it builds no `[lib]` |
| `BINS` | the project's executables as a map keyed by name (`BINS["cli"]`), each `#{ name, src, required_features }` |
| `PKGS` | the project's dependencies as a map keyed by name (`PKGS["libfoo"].dir`), each `#{ name, dir, version, external, source, debug }` |
| `CFG` | the matched section's config data (`CFG.out`, `CFG.enabled`, …). `CFG.prefixes` is an array of install prefixes of everything built before this plugin (core-resolved deps + earlier foreign-build plugins), so a build-system plugin can point `find_package` / pkg-config at them |

`BINS` is keyed by executable name (names are unique). Look one up with
`BINS["cli"]`; iterate with `for b in BINS.values() { … b.name … }` or over
`BINS.keys()`. (Rhai has no `for (key, value)` form.)

`os`/`arch` use the same names as the manifest's `[os.*]` / `[arch.*]` sections
(`linux`, `windows`, `macos`; `x86_64`, `aarch64`, …), so a plugin branches on the
same vocabulary the consumer writes. For a native build `TARGET` mirrors `HOST`.

…and these **functions**:

**Build outputs** — what the plugin feeds back into the build:

| Function | Returns | Effect |
|---|---|---|
| `glob(pattern)` | array | project files matching a glob |
| `run(tool, [args])` / `run(tool, [args], cwd)` | — | run a tool from the plugin's `tools` allow-list (anything else aborts the build); cwd = project root, or a project-confined `cwd`. Its stdout/stderr stream into the build output, and stderr is included in the error if it fails |
| `capture(tool, [args])` | `#{ code, stdout, stderr }` | like `run`, but returns output instead of aborting on a non-zero exit (build stamping, version/`pkg-config` probes) |
| `add_source(path)` / `add_sources([…])` | — | compile generated source(s) |
| `add_include_dir(path)` | — | expose a generated header directory |
| `define(name)` / `define(name, value)` | — | inject a `-D` define |
| `add_flag(tool, flag)` | — | inject a flag into one tool's invocations (see below) |
| `add_prefix(path)` | — | register an install prefix this plugin produced; freight threads it into later plugins' `CFG.prefixes` so a foreign dep built afterwards can resolve it |

`add_flag` targets a specific tool. The `tool` is matched against an invoked
compiler by its template `name`, `alias`, or `family`, the catch-all
`"compiler"`, or a role keyword (`"linker"` / `"archiver"`). The `TOOLS` constant
lists every valid target — each `#{ name, family, kind }` — so a script can
discover what's available:

```rhai
add_flag("clang", "-fno-rtti");           // only clang/clang++ compiles
add_flag("compiler", "-ffast-math");      // every compiler
add_flag("linker", "-Wl,--gc-sections");  // the link step
add_flag("archiver", "-D");               // static-archive (ar) step
// TOOLS = [#{name:"gcc", family:"gnu", kind:"compiler"}, …, #{name:"linker", …}]
```

**Filesystem** — Python-flavoured helpers, all **confined to the project**
(writes create missing parent dirs; reads of a missing file raise):

| Function | Returns | Effect |
|---|---|---|
| `read_text(path)` | string | file contents (raises if missing) |
| `write_text(path, s)` | — | write a file (creates parent dirs) |
| `append_text(path, s)` | — | append to a file (creates it) |
| `copy(src, dst)` | — | copy a file (creates dst's parent dirs) |
| `makedirs(path)` | — | create a directory tree |
| `listdir(path)` | array | entry names in a directory |
| `exists(path)` / `is_file(path)` / `is_dir(path)` | bool | existence / type checks |

**Path strings** — pure helpers (no filesystem), named like Python's `os.path`:

| Function | Returns | Example |
|---|---|---|
| `join(a, b)` / `join([…])` | string | `join("a", "b")` → `"a/b"` |
| `basename(path)` | string | `"a/calc.y"` → `"calc.y"` |
| `dirname(path)` | string | `"a/calc.y"` → `"a"` |
| `stem(path)` | string | `"a/calc.y"` → `"calc"` |
| `ext(path)` | string | `"a/calc.y"` → `"y"` |
| `strip(s)` | string | trimmed *copy* (Rhai's `.trim()` mutates in place and returns `()`) |
| `lines(s)` | array | split text into lines |

**Regex** — Python `re`-flavoured (pattern first); an invalid pattern raises:

| Function | Returns | Effect |
|---|---|---|
| `re_test(pattern, text)` | bool | does `pattern` match anywhere? |
| `re_find(pattern, text)` | array | first match as `[whole, group1, …]`; `[]` if none |
| `re_find_all(pattern, text)` | array | every match, each an array of groups |
| `re_replace(pattern, text, repl)` | string | replace all (`$1` / `${name}` in `repl`) |

Pulling errors out of a tool's output (capture + regex + lines):

```rhai
let r = capture("mytool", glob("src/*.in"));
for line in lines(r.stderr) {
    let m = re_find("(\\S+):(\\d+): error: (.*)", line);   // file:line: error: msg
    if m.len() > 0 { print(m[1] + ":" + m[2] + " — " + m[3]); }
}
```

A build-info stamp, putting it all together:

```rhai
let r = capture("git", ["rev-parse", "--short", "HEAD"]);   // git in [plugin] tools
if r.code == 0 { define("GIT_SHA", strip(r.stdout)); }
write_text("gen/version.h", "#define APP_TARGET \"" + TARGET.triple + "\"\n");
add_include_dir("gen");
```

> A plugin that stamps build info with `capture` should **omit `inputs`** so it
> re-runs every build (otherwise the incremental cache serves a stale stamp).

`print(msg)` writes to the build log (never stdout). To abort with a message,
`throw "reason"` (surfaces as a build-script failure).

(Plus the full Rhai language: `let`/`if`/`for`, user `fn`s, strings, arrays,
maps, and Rhai's standard library. The filesystem helpers stay inside the
project; the only way to reach outside it is an allow-listed `run` tool, which is
the real trust boundary. Scripts are also bounded by operation/recursion limits.)

```rhai
// proto.freight
for f in glob("src/**/*.proto") {
    run("protoc", ["--proto_path=" + SRC_DIR, "--cpp_out=" + OUT_DIR, f]);
}
add_include_dir(OUT_DIR);
for g in glob("target/*/plugin-gen/" + SECTION + "/**/*.pb.cc") { add_source(g); }
```

### `[plugin.schema]` — editor key docs

A plugin may document the keys its handled section accepts as a table of
`key = "one-line description"`. This is **purely advisory** — freight never
validates it — but `freight lsp` uses it to power completion and hover **inside
the consumer's section**: typing in `[proto]` offers `proto_path`, `grpc`, … each
labelled `plugin: proto` with its description, and hovering a key (or the
`[proto]` header) explains it and names the providing plugin. Without a schema,
the plugin still works; the editor just has nothing extra to suggest.

Notes:
- A plugin package may **also** ship a library (e.g. the protobuf runtime) — then
  `proto = "0.1.1"` provides both the codegen and the link library. A
  *plugin-only* package (no `[lib]`) is never built or linked; it only runs.
- **Placement:** a plugin-only build tool (e.g. the `cmake`/`make`/`meson`
  build-system plugins, which run a tool but link nothing) belongs in
  `[build-dependencies]`. A plugin that also ships a linked runtime (like `proto`)
  goes in `[dependencies]`. Both are discovered the same way.
- The manifest stays declarative — `[proto]` is data, and the plugin is a normal,
  lock-pinned dependency.
- **Security:**
  - The script can only run tools the plugin declares in `tools`; arbitrary
    commands are rejected.
  - The file functions (`glob` / `add_source` / `add_include_dir`) are **confined
    to the project directory** — a plugin can't read or inject files from outside
    it (e.g. `glob("/etc/*")` returns nothing; `add_source("../x")` aborts the
    build).
  - Note the real trust boundary is `run`: an allow-listed external tool still
    runs with your privileges and can touch anything you can. So plugin packages
    must be vetted like crates / npm packages — they're versioned, lock-pinned,
    and checksummed. An opt-in for project-granted external file access is
    planned for the cases that need it.
  - **Distribution:** a plugin works whether it's a `path` dependency or fetched
    into `.pkgs/` from a registry, git, or archive URL — freight discovers
    plugins from both. A fetched plugin runs automatically during the build (and
    the LSP refresh), exactly like a path-dep plugin and like a Cargo build
    script: the same `tools` allow-list and project-confinement apply, so vet
    the package before depending on it. A finer-grained capability policy is in
    progress.

---

## `[lib]`

Declares a library output. Omit this section for binary-only projects.

```toml
[lib]
type    = "static"                       # required — static | shared | header-only
srcs    = ["src/mathlib.cpp", "src/vec.cpp"] # source files (string or list; globs allowed)
hdrs    = ["include/mathlib.h"]          # public headers exposed to dependents
defines = ["SPDLOG_COMPILED_LIB"]        # exported defines — see below
```

`srcs` accepts either a single string or a list. Glob patterns are expanded relative to the
project root. These entries are **additive** to the zero-config `src/` walk (useful for sources
outside `src/`).

An entry prefixed with `!` is a **negation** — a glob that *removes* matching files from the
discovered set (applied after additions). Negation globs use gitignore-like semantics:

- patterns are relative to the project root; a leading `/` or `./` is ignored, so `!/src/x.c`,
  `!./src/x.c`, and `!src/x.c` are equivalent;
- `*` matches within a single path segment and does **not** cross `/`;
- `**` spans directories, so a whole subtree is `!src/windows/**`.

Examples: `"!src/fmt.cc"` drops one module unit the walk would otherwise compile;
`"!src/windows/**"` drops a platform subtree; `"!src/*.generated.c"` drops generated files in
one directory.

`defines` lists **exported (public/interface) preprocessor defines**. They are applied to this
library's *own* compilation **and** propagated to every dependent — so a consumer compiles in the
same configuration the library was built with, without restating them (mirrors CMake's
`target_compile_definitions(... PUBLIC ...)`). Use it for macros that are part of the library's
public API/ABI: e.g. a spdlog port built with the external-fmt option declares
`defines = ["SPDLOG_COMPILED_LIB", "SPDLOG_FMT_EXTERNAL"]`, and anyone who depends on it picks
those up automatically. On a `header-only` library there is no own compilation, so `defines`
are interface-only (dependents only). Private, non-exported defines go in `[compiler].defines`.
For a define that should be exported only when a feature is active, use a `pub-define:` entry in
`[features]` (see `[features]`).

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
    external    = true,                  # built by a build-system plugin, not core (see below)
    defines     = ["BUILD_TESTS=OFF"],   # configure defines forwarded to the plugin's section
    include     = ["include/", "src/"],  # explicit include dirs (header-only deps)
    source      = false,                 # true → build this freight package from source
                                         #   even if a prebuilt binary exists
    debug       = false,                 # true → in a debug build, fetch this dep's
                                         #   debug prebuilt (default: always release)
}
```

> Foreign builds are no longer driven by a `type` field — freight builds them
> through **build-system plugins**. Mark the dep `external = true` and add the
> matching plugin + section (e.g. `[cmake]`); see below.

**`external = true`** marks a dependency as built by a **build-system plugin**
rather than freight's core. The source is still fetched into `.pkgs/<name>` (or a
`path` points at it), but freight does not auto-detect or run a build for it.
A plugin that handles a section like `[cmake]` then reads `PKGS["<name>"].dir`
and builds it — see the `cmake` reference plugin under `plugins/`:

The build-system plugin is a build-time tool (it runs cmake; it is not linked
into the artifact), so it goes in `[build-dependencies]`. The library it builds
is linked, so it stays in `[dependencies]`:

```toml
[build-dependencies]
cmake = "0.1"          # plugin handling [cmake] (a build-time tool)

[dependencies]
zlib  = { version = "1.3", external = true }   # linked into the artifact

[cmake]
build = "zlib"
```

Foreign builds (cmake/make/meson/autotools/scons/bazel) are performed by the
**build-system plugins** under `plugins/`, not by freight's core. Declare the dep
`external = true` and add the matching plugin + section — the plugin runs the
tool, installs the result, and wires the headers + libraries back in. A
header-only dep needs no plugin: with no compilable sources, freight skips the
build and collects its include dirs. See [`[plugin]` and build
plugins](#plugin-and-build-plugins).

**Adopting a CMake project.** `freight init` only scaffolds a freight-native project.
To build an existing CMake project with freight, use a **foreign self-build** manifest
(`[package] build = "cmake"`) — CMake configures and builds the whole project via the
cmake plugin, with `find_package` / FetchContent steered to freight/installed packages
by the dependency provider (below). Such a manifest can be written by hand, or generated
by the separate **`freight-migrate`** tool:

```sh
freight-migrate <dir>            # write a build = "cmake" self-build (harvest find_package
                                 # deps, convert vendored submodules/FetchContent/add_subdirectory)
freight-migrate --native <dir>   # instead extract real build data via CMake's File API and
                                 # write a freight-native manifest (best-effort, library-focused)
```

Auto-generating a native manifest from an arbitrary C++ project is best-effort and easy
to get subtly wrong, so it lives in `freight-migrate` rather than `freight` itself; the
safe, supported path is the `build = "cmake"` self-build built by the real cmake tool.

- **`freight add <git-url>`** fetches the repo and, if it ships no `freight.toml`,
  marks the dep `external = true`; when a build system is recognised it also adds
  the matching plugin to `[build-dependencies]` and a `[<backend>] build` entry.

**On-demand dependency provider.** When the cmake plugin builds a CMake project,
the injected `Freight.cmake` registers a [CMake dependency
provider](https://cmake.org/cmake/help/latest/command/cmake_language.html#dependency-providers)
(CMake 3.24+) that intercepts every `find_package` and `FetchContent_MakeAvailable`
at configure time — using CMake's own evaluation, not text scraping. For each one
it calls `freight cmake-provide <name>`, which makes freight's copy available and
prints an install prefix to add to `CMAKE_PREFIX_PATH`. The dep resolves to:

- **installed** — already on the host (pkg-config, *or* an installed
  `<Name>Config.cmake` — so `find_package(c-ares)` is matched even though
  pkg-config calls it `libcares`). Freight provides nothing; CMake finds it.
- a **freight package** fetched under `.pkgs/` — built natively and wrapped in a
  generated `.pc` + `<Name>Config.cmake`.
- a **foreign CMake project** fetched under `.pkgs/` — built via the cmake plugin
  (which runs its own `install`, yielding its real `<Name>Config.cmake`).
- otherwise freight provides nothing and CMake's normal search runs (system
  package, or a `FetchContent` download — freight sets
  `FETCHCONTENT_TRY_FIND_PACKAGE_MODE=ALWAYS` so a freight/installed copy still
  wins when present).

This is dynamic and self-contained: no separate resolver binary, no resolution
file — the cmake script calls `freight` directly, on demand. `freight init` still
harvests `find_package` names statically (from `CMakeLists.txt` + `cmake/*.cmake`)
to seed `[dependencies]`.

**Generated toolchain file.** Alongside the provider, freight writes a
`Freight.toolchain.cmake` (passed via `-DCMAKE_TOOLCHAIN_FILE`) that sets freight's
compilers, applies machine-local host-compat flags (`cmake-c-flags` /
`cmake-cxx-flags`, see [Developer config](#developer-config--outside-freighttoml)),
and prepends freight's package prefixes to `CMAKE_PREFIX_PATH` /
`CMAKE_FIND_ROOT_PATH` so dependency resolution is freight-first. It is skipped when
the project supplies its own `CMAKE_TOOLCHAIN_FILE` (e.g. vcpkg). The provider says
*provide on demand*; the toolchain file says *what to build with and where to look*.
The full story — including cross-compilation, the package export side
(`<Name>Config.cmake`), and how the pieces compose — is in
[cmake-interop.md](cmake-interop.md).

**System registry.** `freight-system-registry` builds a local directory of
`[package]` stubs — one per pkg-config package installed on the host — so freight
can resolve locally-installed libraries offline:

```
freight-system-registry [--out DIR] [--force] [--no-registry] [--limit N]
```

For each installed pkg-config package it writes `<name>.toml` (default
`$FREIGHT_HOME/registries/system/`): a registry's metadata when the package is
published, otherwise a stub whose `version` comes from pkg-config and whose
`description` comes from the system package manager (apt/dnf/pacman/…). The result
is consulted as the `system` repo (`repo = "system"`) and checked first by the
CMake resolver, so a `find_package` for anything already on the host resolves
without a network round-trip.

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
fmt-ext = ["dep:fmt", "pub-define:USE_EXTERNAL_FMT"]  # EXPORTED define: this package + its dependents
```

- `dep:name` — activate optional dependency `name` (no define), mirroring Cargo's `dep:` syntax.
- `define:NAME` / `define:NAME=value` — inject an explicit `-DNAME` / `-DNAME=value` into the
  current package (the `=value` is optional). **Private** — not seen by dependents.
- `pub-define:NAME[=value]` — inject an **exported** define: it lands in this package's own
  compilation **and** propagates to every dependent (the feature-gated counterpart of
  `[lib].defines`). Use it when a feature toggles a macro that is part of the library's public
  API/ABI, so consumers automatically build in the same configuration. Because freight builds each
  package once with its unified resolved feature set, a flipped feature flips the exported define
  for the library and all its consumers in lockstep.
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
[profile.debug]
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
# Extra flags injected into foreign CMake builds via the generated toolchain file
# (CMAKE_<LANG>_FLAGS_INIT). The home for host-compat shims, applied to every
# `build = "cmake"` build on this machine without per-project edits.
# See docs/cmake-interop.md for the full toolchain-file story.
cmake-cxx-flags = ["-include", "cstdint"]
cmake-c-flags   = []

[debugger.gdb]
args  = ["--tui"]   # raw extra flags before the program separator
tui   = true        # resolved via gdb.rhai's settings map → --tui
quiet = true

[debugger.lldb]
no_use_colors = true

# Command aliases (mirrors Cargo's [alias]). A string is split on whitespace;
# an array is taken verbatim. An alias may not shadow a built-in subcommand.
[alias]
b  = "build"
br = ["build", "--release"]
t  = "test"
```

Run an alias like any subcommand: `freight br` expands to `freight build --release`.
Local `[alias]` entries override global ones of the same name.
