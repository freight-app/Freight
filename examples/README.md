# Freight Examples

Each subdirectory is a self-contained freight project.  Run any of them with:

```sh
cd examples/<group>/<name>
freight build
freight run
```

## Layout

| Group | What it covers |
|---|---|
| `c/` | Plain C projects |
| `cpp/` | C++ — modules, multi-binary, static libs, features |
| `fortran/` | Fortran 90/2003 |
| `assembly/` | NASM / GAS assembly mixed with C |
| `mixed/` | Multiple languages in one project |
| `gpu/` | CUDA, HIP, OpenCL, ISPC — require compatible hardware |
| `exotic/` | Ada, D, Objective-C, Zig (as host or as compiler frontend) |
| `deps/` | Dependency management: git, CMake, Make, registry, external, per-dep defines |
| `misc/` | Features, build scripts, docs, workspace, migration |
| `broken/` | **Intentionally broken** — shows freight's error output |

## Feature examples

| Example | What it shows |
|---|---|
| `cpp/features` | `[features]` → `-D<NAME>` conditional compilation; `--features` / `--no-default-features` |
| `deps/dep-defines` | Forwarding a `-D` into a dependency's build via `<dep>/define:NAME`; defines are per-package |
| `c/simd` | `[arch.*] features` enable CPU/ISA extensions (`avx2` → `-mavx2`) with a scalar fallback |
| `c/required-features` | `[[bin]] required-features` gating + `[package] default-run` |
| `misc/examples-target` | `[[example]]` targets + `examples/` auto-discovery; `freight run --example` |
| `deps/patch` | `[patch]` overrides a dependency's source with a local checkout |
| `misc/workspace-inherit` | `[workspace.dependencies]` / `[workspace.package]` inheritance via `{ workspace = true }` |
| `misc/aliases` | `[alias]` command shortcuts in `.freight/config.toml` (`freight b` → `build`) |

## Broken examples

The `broken/` group contains projects that are **expected to fail**.
Each has a `README.md` explaining what error freight will produce and why.

| Example | Failure mode |
|---|---|
| `broken/compile-error` | C++ syntax errors caught at compile time |
| `broken/link-error` | Undefined symbol caught at link time |
| `broken/bad-dep` | Non-existent dependency caught at resolution time |
| `broken/runtime-crash` | Null dereference / UB — builds fine, crashes at runtime |
