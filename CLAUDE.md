# Freight — Build Tool & Package Manager

## What is freight?

Freight is a Cargo-inspired build tool and package manager for compiled languages that target GCC or Clang: C, C++, Fortran, assembly, CUDA, HIP, OpenCL, and others. It aims to be the single tool you need to build, test, and publish native code — no Makefile, no CMake, no Ninja required.

The project is written in Rust.

---

## Core philosophy

- **No external build system** — freight owns the entire build graph internally. No Ninja, no Make underneath.
- **Declarative compiler templates** — each compiler (gcc, clang, nvcc, gfortran, nasm…) is described in a `.rhai` file that maps abstract settings to real flags. Adding a new compiler = writing a Rhai script, not writing Rust.
- **One tool, many languages** — file extension routes to the right compiler automatically. A single project can mix `.cpp`, `.c`, `.f90`, `.asm`, `.cu` files.
- **Incremental by default** — mtime dirty checking via Makefile `.d` dep files (source + all included headers), parallel compilation via rayon.
- **C++20 modules supported** — scanner detects `export module` / `import` declarations, builds a dependency DAG, compiles MIUs in topological order (parallel within each level), then compiles the rest in parallel with `-fmodule-file=` flags injected per import.

---

## Naming conventions

| Name | Meaning |
|---|---|
| `freight` | The CLI binary |
| `freight.toml` | Project manifest |
| `freight.lock` | Auto-generated lockfile (commit this) |
| `build.freight` | Optional pre-build hook script |
| `~/.freight/` | Global cache directory |
| `freight.dev` | The package registry — not yet implemented |

---

## Repository layout

```
crane/                              # repo root (git)
├── Cargo.toml                      # workspace root
├── CLAUDE.md                       # this file
├── vendors/                        # runtime arch/os/compiler token database
│   ├── x86_64.toml                 # kind = "arch", aliases = ["amd64", ...]
│   ├── linux.toml                  # kind = "os"
│   ├── gnu.toml                    # kind = "compiler", aliases = ["gnueabi", ...]
│   └── ...                         # one .toml per arch/os/compiler family
├── toolchains/                     # bundled .rhai compiler templates
│   ├── gcc.rhai
│   ├── clang.rhai
│   ├── gfortran.rhai
│   ├── nasm.rhai
│   ├── nvcc.rhai
│   ├── msvc.rhai
│   └── ...                         # one per compiler
├── crates/
│   ├── freight/                    # binary crate — CLI shells + clap dispatch
│   │   └── src/
│   │       ├── main.rs             # clap parse → commands::* dispatch
│   │       ├── output.rs           # coloured print helpers (CLI-only)
│   │       └── commands/           # one cmd_* shell per command, calls into freight-core
│   │           ├── mod.rs
│   │           ├── build.rs        # cmd_build, cmd_run, cmd_test, cmd_clean, cmd_watch
│   │           ├── check.rs        # cmd_check + manifest summary printer
│   │           ├── compile_commands.rs  # cmd_compile_commands
│   │           ├── debug.rs        # cmd_debug
│   │           ├── deps.rs         # cmd_add, remove, update, fetch, tree, search, info, login, publish, yank
│   │           ├── doc.rs          # cmd_doc, cmd_man
│   │           ├── install.rs      # cmd_install, cmd_package
│   │           ├── migrate.rs      # cmd_migrate
│   │           ├── new.rs          # cmd_new, cmd_init
│   │           └── toolchain.rs    # cmd_toolchain_list, cmd_toolchain_add
│   ├── freight-core/               # library crate — all build logic, no CLI / no printing of results
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs
│   │       ├── new.rs              # scaffold_project / init_project (returns ScaffoldOutcome)
│   │       ├── dep_cmds.rs         # manifest_add_dep, manifest_remove_dep, regen_lock, locate_project
│   │       ├── install.rs          # install_project, package_project
│   │       ├── lock.rs             # freight.lock read/write
│   │       ├── vendor.rs           # VendorDb, parse_triple, global_db()
│   │       ├── manifest/           # freight.toml parsing + validation
│   │       │   ├── mod.rs
│   │       │   ├── types.rs
│   │       │   ├── find.rs
│   │       │   └── validate.rs
│   │       ├── toolchain/          # compiler detection + templates
│   │       │   ├── mod.rs
│   │       │   ├── template.rs
│   │       │   ├── detect.rs
│   │       │   └── cache.rs
│   │       └── build/              # compilation + linking orchestration
│   │           ├── mod.rs          # build_project, clean_project, test_project (pub functions)
│   │           ├── compile.rs      # source → object, parallel via rayon
│   │           ├── deps.rs         # dep graph resolution + topo sort + slot conflict check
│   │           ├── discover.rs     # walkdir source discovery
│   │           ├── features.rs     # feature resolution, dep: activation, to_defines()
│   │           ├── foreign.rs      # CMake/Make/Meson/SCons/Autotools foreign dep builds
│   │           ├── header_units.rs # C++20 header unit precompilation
│   │           ├── http.rs         # URL dep download + sha256 verify
│   │           ├── link.rs         # object → binary / .a / .so
│   │           ├── modules.rs      # C++20 module scanner, DAG, phased compilation
│   │           └── script.rs       # build.freight script runner
│   ├── freight-doc/                # doc extraction and site generation
│   ├── freight-migrator/           # library crate — freight migrate (CMake/Makefile/Meson → freight.toml)
│   │   └── src/
│   │       ├── lib.rs              # run_migrate → MigrateOutcome, ImportedProject IR
│   │       ├── detect.rs           # pick format from files present
│   │       ├── emit.rs             # ImportedProject → freight.toml string
│   │       ├── cmake.rs            # CMakeLists.txt parser
│   │       ├── makefile.rs         # Makefile parser
│   │       └── meson.rs            # meson.build parser
│   └── freight-lsp/                # Language Server for freight.toml
│       └── src/
│           ├── lib.rs
│           ├── completion.rs
│           ├── docs.rs
│           └── position.rs
└── examples/                       # every example is buildable — `cd <dir> && freight build`
    ├── hello-cpp/
    ├── multi-lang/
    ├── with-deps/
    ├── c-simple/
    ├── multi-bin/
    ├── cpp-modules/
    ├── tri-lang/
    ├── with-build-script/
    ├── with-cmake-dep/
    ├── with-git-dep/
    └── migrated-from-cmake/
```

---

## freight.toml — manifest format

See **`docs/manifest-reference.md`** for the complete field reference. Minimal example:

```toml
[package]
name    = "myproject"
version = "0.1.0"

[language.cpp]
std = "c++20"

[[bin]]
name = "myproject"
src  = "src/main.cpp"

[dependencies]
myutils = { path = "../myutils" }   # path dep
openssl = { system = "openssl" }    # system dep
pthread = { system = "pthread", os = "linux" }  # OS-filtered

[profile.release]
opt-level = 3
lto       = true
strip     = true
```

---

## Build engine — internal pipeline

```
freight build
  │
  ├── 1. Parse + validate freight.toml
  ├── 2. Detect toolchain (probe $PATH, load compiler templates, version cache)
  ├── 3. Resolve features (dep: entries activate optional deps, profile features merged)
  ├── 4. Resolve dependency graph (topo sort, flat .deps/ pool, slot conflict check)
  │       ├── compile each dep → archive (.a)
  │       └── collect dep include dirs
  ├── 5. Walk src/ — discover sources by file extension → language key
  ├── 6. Scan C++ sources for `export module` / `import` declarations
  │       ├── [no modules] → flat parallel compile (step 7a)
  │       └── [modules found] → module-aware pipeline (step 7b)
  ├── 7a. Flat: dirty-check + compile all sources in parallel (rayon)
  ├── 7b. Module-aware:
  │       ├── topo-sort MIUs into batches (Kahn's algorithm)
  │       ├── for each batch: compile MIUs in parallel → produce .pcm + .o
  │       │     GCC: one pass with -fmodule-output=
  │       │     Clang: --precompile → .pcm, then -c → .o
  │       └── compile MImplUs + regular TUs in parallel with -fmodule-file= per import
  └── 8. Link all .o + dep .a files → binary / .a / .so
          (each [[bin]] only links its own entry-point .o, not other bins')
```

---

## Dependency model

| Kind | freight.toml syntax | How it works |
|---|---|---|
| Path | `{ path = "../mylib" }` | Compiles the dep project, links its `.a` archive |
| System | `{ system = "openssl" }` | Passes `-l{name}` to the linker |
| Version | `"0.3"` | Fetched from freight.dev (not yet implemented) |
| Git | `{ git = "..." }` | Cloned to `.deps/{name}/` by `freight fetch` |
| URL | `{ url = "https://..." }` | Downloaded + extracted to `.deps/{name}/` |
| Foreign | `{ path = "...", build_system = "cmake" }` | Delegates to CMake/Make/Meson/SCons/Autotools |
| Optional | `{ path = "...", optional = true }` | Only compiled when activated via a `dep:name` feature |

All deps — including transitive ones — live in the **root project's flat `.deps/` pool**.
Version/git deps always resolve to `{root}/.deps/{name}/`. Path deps are relative to the
manifest that declares them. The topo sort ensures deps are compiled in the right order.

### Slot conflict detection

A package can declare `provides = ["blas"]` in its `[package]`. If two active deps fill the
same slot, freight errors before compilation:

```
error: slot conflict — 'openblas' and 'mkl' both provide 'blas'
       only one provider per slot may be active
```

Use optional deps + features to select one provider at a time.

---

## CLI commands

```
freight new <name> [--lang <lang>]         scaffold a new project              ✓ implemented
freight init [--lang <lang>]               init freight in current directory   ✓ implemented
freight build [--release] [--features F]   build the project                   ✓ implemented
freight run [--release] [-- <args>]        build and run default binary        ✓ implemented
freight test [<name>] [--release]          build and run tests                 ✓ implemented
freight clean                              wipe target/                        ✓ implemented
freight check                              validate freight.toml               ✓ implemented
freight watch [--release]                  rebuild on file changes             ✓ implemented
freight debug [<binary>] [--debugger D]    launch interactive debugger         ✓ implemented
freight compile-commands [--release]       generate compile_commands.json      ✓ implemented
freight doc [--format html|md|latex|pdf]   generate documentation site         ✓ implemented
freight man [--out-dir DIR]                generate man pages                  ✓ implemented

freight add <name>[@ver] [--path P] [--system] [--dev]  add a dependency      ✓ implemented
freight remove <package>                   remove a dependency                 ✓ implemented
freight update [<package>]                 refresh lockfile for path deps      ✓ implemented (registry pending)
freight fetch                              verify/download deps                ✓ implemented (registry pending)
freight tree                               print dependency tree               ✓ implemented
freight info <package>                     show package metadata               ✗ Phase 12 (registry server)
freight search <query>                     search freight.dev                  ✗ Phase 12 (registry server)
freight migrate [--from <format>] [--dry-run] [--force]  import existing build system  ✓ implemented
freight install [--prefix P] [--destdir D] [--target T]  install to system    ✓ implemented
freight package [--target TRIPLES]         build redistributable tar.gz        ✓ implemented
freight login                              authenticate with freight.dev       ✗ Phase 12 (registry server)
freight publish                            upload package to registry          ✗ Phase 12 (registry server)
freight yank <version>                     yank a published version            ✗ Phase 12 (registry server)
freight toolchain list                     show detected compilers             ✓ implemented
freight toolchain add <path>               install a compiler template         ✓ implemented
freight toolchain use <name>               set default compiler backend        ✓ implemented
freight lsp                                run language server on stdio        ✓ implemented
```

---

## Development roadmap

See **`docs/roadmap.md`** for full per-phase checklists. Status summary:

| Phase | Topic | Status |
|---|---|---|
| 1 | CLI skeleton | ✓ complete |
| 2 | Manifest parsing | ✓ complete |
| 3 | Compiler detection (family grouping, guest extensions) | ✓ complete |
| 4 | Build engine | ✓ complete |
| 5 | Dependencies | ✓ complete |
| 6 | Assembly + target config | ✓ complete |
| 7 | Examples | ✓ complete |
| 8 | C++20 modules | ✓ complete |
| 9 | Registry client + lockfile | partial — registry server unblocks remainder |
| 10 | Cross-compilation | ✓ complete |
| 11 | Build system migrator | ✓ complete |
| 12 | Features + optional deps | ✓ complete |
| 13 | Registry server (`crates/freight-registry/`) | planned |
| 14 | Language server (`freight lsp`) | in progress — VS Code ext + inlay hints pending |

---

## Backburner (deferred, not forgotten)

- **Slot-based substitution** — `provides` currently only detects conflicts; auto-routing a dep request to a compatible provider (e.g. root has `mkl`, sub-dep requests `openblas`, both provide `blas`) is complex and deferred to Phase 13+
- **Progress callbacks** — build output currently goes to stdout via `println!`; routing through a callback for GUI/TUI frontends is future work
- **Per-language `[platform]` overlays** — `[platform.linux.language.cpp]` deliberately excluded from v1
- **JWT/OAuth for registry** — v1 uses static bearer tokens only
- **Git dep recursive fetch** — freight intentionally does not fetch transitively; user manages `.deps/` manually

---

## Architecture rules

1. **`freight` crate owns the CLI** — clap parsing, `commands/` shells, and `output.rs` colour helpers. Each `cmd_*` reads cwd, calls a pure function in `freight-core`, prints the outcome.
2. **`freight-core` is a library, no CLI knowledge** — pure functions return `Result<T, FreightError>`. It must not depend on `output.rs` or call `print_*`. Inline `println!` for build-engine progress is the one exception, pending a future progress-callback abstraction.
3. **`freight-migrator` is a separate library** — depends on `freight-core` for `FreightError`, exposes `run_migrate → MigrateOutcome`.
4. **Compiler templates are runtime data** — loaded from `toolchains/` directory as `.rhai` files, not hardcoded in Rust.
5. **Vendor database is runtime data** — arch/os/compiler tokens loaded from `vendors/*.toml` at startup via `VendorDb`; adding a new target = writing a TOML.
6. **One template per toolchain, not per language** — `gcc.rhai` handles both C and C++; `compile_binary` in `[linking.c]` overrides which binary compiles that language.
7. **DAG cycles = hard error** — report the full cycle path (both dep cycles and module cycles).
8. **Flat dep pool** — all deps resolve from the root project's `.deps/`; no nested `.deps/` inside deps.
9. **`CompilerTemplate::assemble_flags()` is pure** — no side effects, unit-tested.
10. **Never shell out to Make / Ninja / CMake during a build** — freight owns the build graph entirely (foreign deps are the explicit exception).
11. **Errors use `thiserror` in freight-core, surface at the CLI boundary.**
12. **Feature branches** — each new feature gets its own `feature/<name>` branch off `master`.
13. **Module detection is transparent** — `build_sources()` scans automatically; projects without `export module` take the unchanged fast path.

