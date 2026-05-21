# Cargo vs Freight

Freight is deliberately modelled after Cargo. If you know Cargo, most concepts map directly.

---

## Manifest

| Concept | Cargo (`Cargo.toml`) | Freight (`freight.toml`) |
|---|---|---|
| Package metadata | `[package]` | `[package]` — same fields |
| Binary target | `[[bin]]` | `[[bin]]` — same shape |
| Library target | `[lib]` | `[lib]` — adds `type = "static \| shared \| header-only"` |
| Features | `[features]` | `[features]` — same syntax; active features emit `-D<NAME>` |
| Build profiles | `[profile.dev]`, `[profile.release]` | `[profile.dev]`, `[profile.release]` — same keys |
| Dependencies | `[dependencies]` | `[dependencies]` — extended (see below) |
| Dev dependencies | `[dev-dependencies]` | `[dev-dependencies]` |
| Build script | `build.rs` | Not supported |
| Workspaces | `[workspace]` in root `Cargo.toml` | Same |

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
srcs         = ["src/os/linux/**"]
defines      = ["PLATFORM_LINUX"]
dependencies = { pthread = { system = "pthread" } }

[os.windows]
srcs = ["src/os/windows/**"]
```

---

## Dependencies

### What's the same

| Kind | Cargo | Freight |
|---|---|---|
| Path dep | `{ path = "../mylib" }` | `{ path = "../mylib" }` |
| Git dep | `{ git = "...", tag = "v1" }` | `{ git = "...", tag = "v1" }` — same keys |
| Dev dep | `[dev-dependencies]` | `[dev-dependencies]` |
| Feature selection | `{ features = ["tls"] }` | `{ features = ["tls"] }` |
| Default features | `{ default-features = false }` | `{ default-features = false }` |

### What's different

Cargo resolves Rust crates from crates.io. Freight resolves C/C++ libraries through a chain:

1. **pkg-config** — `pkg-config --modversion <name>`
2. **Conan** — `conan install <name>/<version>`
3. **vcpkg** — `vcpkg install <name>`
4. **System-lib stub** — bundled stubs for common OS libraries (pthread, zlib, OpenSSL, …)

Pin a specific resolver with `repo`:
```toml
zlib = { version = "1.3", repo = "vcpkg" }
```

### What Freight adds

**URL archive dependency** — not in Cargo:
```toml
json = { url = "https://github.com/nlohmann/json/archive/refs/tags/v3.11.3.tar.gz", sha256 = "..." }
```

**Foreign build system deps** — CMake, Meson, Autotools, Make, Bazel, SCons:
```toml
SDL2 = { path = "../SDL2", backend = "cmake", cmake-args = ["-DSDL_STATIC=ON"] }
```

**System link deps** — explicit linker flags, no package manager:
```toml
openssl = { system = "ssl" }
```

**Platform filters on deps**:
```toml
pthread = { system = "pthread", os = "linux" }
ws2_32  = { system = "ws2_32", os = "windows" }
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
| `cargo tree` | `freight tree` | Adds `--sources` for the `#include` graph |
| `cargo doc` | `freight doc` | Generates Markdown API docs via `docify`; no HTML |
| `cargo fmt` | `freight fmt` | Wraps clang-format (or astyle, uncrustify, fprettify) |
| `cargo clippy` | `freight lint` | Wraps clang-tidy (or cppcheck, cpplint, flawfinder) |
| `cargo install` | `freight install` | Installs to a prefix; supports `--destdir` for packaging |
| `cargo publish` | `freight publish` | Same; adds `--prebuilt` for binary releases |
| `cargo login` | `freight login` | Same |
| `cargo search` | `freight search` | Same |
| — | `freight debug` | Launches GDB or LLDB; generates `.vscode/launch.json` |
| — | `freight watch` | Rebuilds on file changes |
| — | `freight compile-commands` | Generates `compile_commands.json` for clangd/fortls |
| — | `freight toolchain list` | Shows detected compilers and debuggers |
| — | `freight toolchain use` | Sets the default compiler backend |
| `cargo outdated` (plugin) | `freight outdated` | Built-in |

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
- **Workspaces with virtual manifests** — Freight workspaces require each member to have its own `freight.toml`; virtual root manifests are not supported yet.
- **Edition system** — no concept of language editions; standards are set explicitly.

## What Freight has that Cargo does not

- **Multi-language builds** — C, C++, Fortran, CUDA, HIP, D, OpenCL, and assembly in one project.
- **Foreign build system integration** — pull in CMake/Meson/Autotools deps natively.
- **`[os.*]` / `[arch.*]` source routing** — platform-specific source files declared in the manifest.
- **`freight debug`** — integrated GDB/LLDB launcher and VS Code `launch.json` generator.
- **`freight compile-commands`** — `compile_commands.json` for language server integration.
- **Prebuilt binary publishing** — `freight publish --prebuilt` uploads a binary tarball alongside source.
- **CPU extension flags** — `[target] cpu-extensions = ["avx2", "fma"]` emits `-mavx2 -mfma`.
