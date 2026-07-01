# Cargo vs Freight

Freight is deliberately modelled after Cargo. If you know Cargo, most concepts map directly.

---

## Manifest

| Concept | Cargo (`Cargo.toml`) | Freight (`freight.toml`) |
|---|---|---|
| Package metadata | `[package]` | `[package]` — same fields |
| Binary target | `[[bin]]` | `[[bin]]` — same shape; supports `required-features` |
| Example target | `[[example]]` + `examples/` | `[[example]]` + `examples/` — auto-discovered; `--example`/`--examples` |
| Default binary | `[package] default-run` | `[package] default-run` — same |
| Library target | `[lib]` | `[lib]` — adds `type = "static \| shared \| header-only"` |
| Features | `[features]` | `[features]` — same syntax; active features emit `-D<NAME>` |
| Build profiles | `[profile.debug]`, `[profile.release]` | `[profile.debug]`, `[profile.release]` — same keys |
| Dependencies | `[dependencies]` | `[dependencies]` — extended (see below) |
| Dev dependencies | `[dev-dependencies]` | `[dev-dependencies]` — debug builds + tests only |
| Build-time tools | `[build-dependencies]` (for build.rs) | `[build-dependencies]` — executables needed during compilation; `bin/` prepended to PATH |
| Build script | `build.rs` | Not supported |
| Workspaces | `[workspace]` in root `Cargo.toml` | Same — `[workspace.dependencies]` / `[workspace.package]` inheritance via `{ workspace = true }` |
| Dependency source override | `[patch]` / `[replace]` | `[patch]` — path overrides (across the whole graph, incl. transitive) |

### Differences in `[lib]`

Freight requires `type = "static" | "shared" | "header-only"` — Cargo infers this from the crate type. `header-only` skips compilation and just exposes include paths to dependents.

### No `[package] edition`

Freight has no language edition concept. Language standards are set per-language under `[language.<key>]`:

```toml
[language.cpp]
std = "c++20"

[language.c]
std = "c17"
```

### Platform-conditional sections

Freight adds `[os.<key>]` and `[arch.<key>]` sections that Cargo handles with `cfg()` attributes in code. Files listed in these sections are completely excluded from the build on non-matching platforms — they are never compiled, not just conditionally skipped.

```toml
[os.linux]
srcs     = ["src/os/linux/**"]
defines  = ["PLATFORM_LINUX"]
features = ["pthread"]

[os.windows]
srcs = ["src/os/windows/**"]
```

---

## Dependencies

### What's the same

| Kind | Cargo | Freight |
|---|---|---|
| Path dep | `{ path = "../mylib" }` | `{ path = "../mylib" }` |
| Git dep | `{ git = "...", tag = "v1" }` | `{ url = "....git", tag = "v1" }` — git URL + ref |
| Dev dep | `[dev-dependencies]` | `[dev-dependencies]` |
| Build-time tool | `[build-dependencies]` | `[build-dependencies]` — different semantics: no build.rs, tools go on PATH |
| Feature selection | `{ features = ["tls"] }` | `{ features = ["tls"] }` |
| Default features | `{ default-features = false }` | `{ default-features = false }` |

### What's different

Cargo resolves Rust crates from crates.io. Freight resolves C/C++ libraries through a chain:

1. **pkg-config** — `pkg-config --modversion <name>`
2. **System-lib stub** — bundled stubs for common OS libraries (pthread, zlib, OpenSSL, …)
3. **Registry** — `.deps/` cache populated by `freight fetch`

Pin a specific resolver with `repo` (`"system"` for stubs only, or a named registry):
```toml
zlib = { version = "1.3", repo = "system" }
```

### What Freight adds

**URL archive dependency** — not in Cargo:
```toml
json = { url = "https://github.com/nlohmann/json/archive/refs/tags/v3.11.3.tar.gz", sha256 = "..." }
```

**Foreign build system deps** — CMake, Meson, Autotools, Make, Bazel, SCons —
are built by build-system plugins: mark the dep `external = true` and add the
matching plugin + section.
```toml
SDL2          = { path = "../SDL2", external = true }
cmake = "0.1"
[cmake]
build   = "SDL2"
defines = ["SDL_STATIC=ON"]
```

**System libraries** — resolved via pkg-config → stub → registry from a bare version:
```toml
openssl = "3.0"
```

**Versionless system libraries** — linked via `[os.*] features`, not a dep entry:
```toml
[os.unix]
features = ["pthread"]   # -lpthread on Unix

[os.windows]
features = ["ws2_32"]    # -lws2_32 on Windows
```

---

## Commands

| Cargo | Freight | Notes |
|---|---|---|
| `cargo new` | `freight new` | Same; `--lang` selects C, C++, Fortran, etc. |
| `cargo init` | `freight init` | Same |
| `cargo build` | `freight build` | Same flags: `--release`, `--features`, `--package` |
| `cargo run` | `freight run` | Same |
| `cargo test` | `freight test` | Same |
| `cargo bench` | `freight bench` | Same |
| `cargo check` | `freight check` | Validates `freight.toml`; no type-checking (no compiler) |
| `cargo clean` | `freight clean` | Same — wipes `target/` |
| `cargo add` | `freight add` | Same flags; adds git/path/system/URL deps |
| `cargo remove` | `freight remove` | Same |
| `cargo update` | `freight update` | Same |
| `cargo fetch` | `freight fetch` | Same |
| `cargo tree` | `freight tree` | Shows all dep kinds (normal/build/dev); `--depth N`; adds `--sources` for the `#include` graph |
| `cargo doc` | `freight doc` | Generates Markdown API docs via `docify`; no HTML |
| `cargo fmt` | `freight fmt` | Wraps clang-format (or astyle, uncrustify, fprettify) |
| `cargo clippy` | `freight lint` | Wraps clang-tidy (or cppcheck, cpplint, flawfinder) |
| `cargo install` | `freight install` | Installs to a prefix; supports `--destdir` for packaging |
| `cargo publish` | `freight publish` | Same; adds `--prebuilt` for binary releases |
| `cargo login` | `freight login` | Same |
| `cargo search` | `freight search` | Same |
| — | `freight debug` | Launches GDB or LLDB; generates `.vscode/launch.json` |
| — | `freight watch` | Rebuilds on file changes |
| — | `freight compile-commands` | Generates `compile_commands.json` for clangd and other source tools |
| — | `freight toolchain list` | Shows detected compilers and debuggers |
| — | `freight toolchain use` | Sets the default compiler backend |
| `cargo outdated` (plugin) | `freight outdated` | Built-in |
| `cargo metadata` | `freight metadata` | JSON of the resolved package + dep graph (`--no-deps`, `--compact`) |
| — | `freight workspace graph` | Visualises inter-member path-dep edges (text / mermaid / dot) |

### Build flags

| Cargo | Freight | Notes |
|---|---|---|
| `--offline` | `--offline` | No network access; use only deps already in `.pkgs/` |
| `--locked` | `--locked` | Require `freight.lock` to be up to date; never rewrite it |
| `--frozen` | `--frozen` | `--offline` + `--locked` |

Available on `freight build` / `run` / `test` and other build-engine commands.

### Command aliases

| Cargo | Freight |
|---|---|
| `[alias]` in `.cargo/config.toml` | `[alias]` in `~/.freight/config.toml` or `<project>/.freight/config.toml` |

```toml
[alias]
b  = "build"
br = ["build", "--release"]
```

An alias may not shadow a built-in subcommand; local entries override global.

---

## Build output

| | Cargo | Freight |
|---|---|---|
| Output directory | `target/debug/` or `target/release/` | `target/debug/` or `target/release/` |
| Incremental builds | Rustc-native | mtime + `.d` dep files |
| Parallel compilation | Always on | `rayon`-parallel; C++20 modules use a DAG-ordered Kahn batch |
| LTO | `lto = true` in profile | `lto = true` in profile |
| Sanitizers | Not in manifest | `sanitize = ["address", "undefined"]` in profile or `--sanitize` flag |
| Strip | Not in manifest | `strip = true` in profile |
| Emit ASM | `--emit=asm` | `--emit asm` |
| Build graph | — | `--graph` (text / mermaid / dot) |

---

## Toolchain

| | Cargo | Freight |
|---|---|---|
| Toolchain spec | `rust-toolchain.toml` | `[compiler] backend = "clang"` in manifest or `~/.freight/config.toml` |
| Cross-compilation | `--target <triple>` | `[compiler] target = "<triple>"` or `~/.freight/config.toml` |
| Sysroot | Handled by rustup | `[compiler] sysroot` or `FREIGHT_SYSROOT` env var |
| Multiple compilers | No (one Rust compiler) | Yes — GCC, Clang, MSVC, Intel, NVHPC, TCC, DMD, MSVC, and more |
| Guest compilers | No | NVCC, HIPCC, NASM, YASM auto-activate alongside a host toolchain |

---

## What Cargo has that Freight does not

- **`build.rs`** — pre-build Rust scripts. Freight has no equivalent; platform-conditional logic belongs in `[os.*]`/`[arch.*]` sections.
- **Procedural macros** — not applicable to C/C++.
- **`cargo fix`** — Freight's `freight lint --fix` is the closest equivalent.
- **Workspaces with virtual manifests** — Freight workspaces require each member to have its own `freight.toml`; virtual root manifests are not supported yet. (Dependency and package-field inheritance via `{ workspace = true }` *is* supported.)
- **Edition system** — no concept of language editions; standards are set explicitly.

## What Freight has that Cargo does not

- **Multi-language builds** — C, C++, Fortran, CUDA, HIP, D, OpenCL, and assembly in one project.
- **Foreign build system integration** — pull in CMake/Meson/Autotools deps natively.
- **`[os.*]` / `[arch.*]` source routing** — platform-specific source files declared in the manifest.
- **`freight debug`** — integrated GDB/LLDB launcher and VS Code `launch.json` generator.
- **`freight compile-commands`** — `compile_commands.json` for language server integration.
- **Prebuilt binary publishing** — `freight publish --prebuilt` uploads a binary tarball alongside source.
- **CPU extension flags** — `[target] cpu-extensions = ["avx2", "fma"]` emits `-mavx2 -mfma`.
