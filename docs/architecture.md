# Freight — Architecture

Internal documentation for contributors. Covers the repository layout, build engine
pipeline, architecture rules, and the key Rust dependencies.

---

## Repository layout

```
freight/
├── Cargo.toml                  # workspace root
├── README.md
├── crates/
│   ├── freight/                  # binary crate — CLI shells + clap dispatch
│   │   └── src/
│   │       ├── main.rs         # clap parse → commands::* dispatch
│   │       ├── output.rs       # coloured print helpers (CLI-only)
│   │       └── commands/       # one cmd_* shell per command, calls into freight-core
│   │           ├── mod.rs
│   │           ├── build.rs    # cmd_build, cmd_run, cmd_test, cmd_clean
│   │           ├── check.rs    # cmd_check + manifest summary printer
│   │           ├── deps.rs     # cmd_add, remove, update, fetch, tree, search, info, login, publish, yank
│   │           ├── doc.rs      # cmd_doc, cmd_man
│   │           ├── migrate.rs  # cmd_migrate
│   │           ├── new.rs      # cmd_new, cmd_init
│   │           └── toolchain.rs # cmd_toolchain_list, cmd_toolchain_add
│   ├── freight-core/             # library crate — all build logic, no CLI / no printing of results
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs
│   │       ├── new.rs          # scaffold_project / init_project (returns ScaffoldOutcome)
│   │       ├── dep_cmds.rs     # manifest_add_dep, manifest_remove_dep, regen_lock, locate_project
│   │       ├── lock.rs         # freight.lock read/write
│   │       ├── manifest/       # freight.toml parsing + validation
│   │       │   ├── mod.rs
│   │       │   ├── types.rs
│   │       │   ├── find.rs
│   │       │   └── validate.rs
│   │       ├── toolchain/      # compiler detection + templates
│   │       │   ├── mod.rs
│   │       │   ├── template.rs
│   │       │   ├── detect.rs
│   │       │   ├── cache.rs
│   │       │   ├── engine.rs   # Rhai engine + registered API
│   │       │   └── debugger.rs # DebuggerTemplate + detect_debuggers()
│   │       ├── doc/            # documentation extraction and rendering
│   │       │   ├── mod.rs      # OutputFormat enum + render() dispatch
│   │       │   ├── extract.rs  # multi-language doc comment extractor
│   │       │   ├── markdown.rs # math protection + MD→HTML + MD→LaTeX via pulldown-cmark
│   │       │   ├── render.rs   # HTML renderer (self-contained, MathJax)
│   │       │   ├── render_md.rs  # Markdown renderer (GFM, cross-document links)
│   │       │   └── render_latex.rs # LaTeX renderer + PDF compilation
│   │       └── build/          # compilation + linking orchestration
│   │           ├── mod.rs      # build_project, clean_project, test_project (pub functions)
│   │           ├── compile.rs  # source → object, parallel via rayon
│   │           ├── link.rs     # object → binary / .a / .so
│   │           ├── discover.rs # walkdir source discovery
│   │           ├── deps.rs     # dep graph resolution + topo sort
│   │           ├── features.rs # Cargo-style [features] resolve + define generation
│   │           ├── foreign.rs  # foreign build system integration (cmake/make/meson/autotools/scons)
│   │           └── modules.rs  # C++20 module scanner, DAG, phased compilation
│   ├── freight-doc/              # standalone doc generator binary (freight-doc CLI)
│   │   └── src/
│   │       └── main.rs         # freight-doc --format html|md|latex|pdf|all [DIR...] --out DIR
│   ├── freight-migrator/         # library crate — freight migrate (CMake/Makefile/Meson → freight.toml)
│   │   └── src/
│   │       ├── lib.rs          # run_migrate → MigrateOutcome, ImportedProject IR
│   │       ├── detect.rs       # pick format from files present
│   │       ├── emit.rs         # ImportedProject → freight.toml string
│   │       ├── cmake.rs        # CMakeLists.txt parser
│   │       ├── makefile.rs     # Makefile parser
│   │       └── meson.rs        # meson.build parser
│   └── freight-lsp/              # Language Server for freight.toml
│       └── src/
│           ├── lib.rs
│           ├── position.rs     # text-based position mapping for diagnostics
│           ├── completion.rs   # section-aware completions
│           └── docs.rs         # hover docs keyed by dotted path
├── toolchains/                 # compiler scripts (.rhai) + debugger templates (.toml)
│   ├── gcc.rhai
│   ├── clang.rhai
│   ├── nasm.rhai
│   ├── gfortran.rhai
│   ├── gnat.rhai
│   ├── dmd.rhai
│   ├── nvcc.rhai
│   ├── hipcc.rhai
│   ├── icpx.rhai
│   ├── opencl.rhai
│   ├── ispc.rhai
│   ├── tcc.rhai
│   ├── nvhpc.rhai
│   ├── ifx.rhai
│   ├── flang.rhai
│   ├── ldc2.rhai
│   ├── yasm.rhai
│   ├── circle.rhai
│   └── debuggers/
│       ├── lldb.toml
│       └── gdb.toml
└── examples/                   # every example is buildable via `freight build`
    ├── hello-cpp/
    ├── multi-lang/
    ├── with-deps/
    ├── c-simple/
    ├── multi-bin/
    ├── cpp-modules/
    ├── tri-lang/
    ├── asm-hello/
    ├── with-cmake-dep/
    ├── with-make-dep/
    ├── with-git-dep/
    ├── with-external-deps/
    ├── doc-example/
    └── migrated-from-cmake/
```

---

## Build engine pipeline

```
freight build
  │
  ├── 1. Parse + validate freight.toml
  ├── 2. Detect toolchain (probe $PATH, evaluate .rhai scripts, version cache)
  ├── 3. Resolve dependency graph (topo sort, compile path deps in order)
  │       ├── freight deps: compile dep → archive (.a)
  │       ├── foreign deps: cmake/meson/make/autotools/scons → install → collect headers + archive
  │       └── collect dep include dirs
  ├── 4. Walk src/ — discover sources by file extension → language key
  ├── 5. Scan C++ sources for `export module` / `import` declarations
  │       ├── [no modules] → flat parallel compile (step 6a)
  │       └── [modules found] → module-aware pipeline (step 6b)
  ├── 6a. Flat: dirty-check + compile all sources in parallel (rayon)
  ├── 6b. Module-aware:
  │       ├── topo-sort MIUs into batches (Kahn's algorithm)
  │       ├── for each batch: compile MIUs in parallel → produce .pcm + .o
  │       │     GCC: one pass with -fmodule-output=
  │       │     Clang: --precompile → .pcm, then -c → .o
  │       └── compile MImplUs + regular TUs in parallel with -fmodule-file= per import
  └── 7. Link all .o + dep .a files → binary / .a / .so
          (each [[bin]] only links its own entry-point .o, not other bins')
```

---

> Architecture rules are maintained in **`CLAUDE.md`** under the "Architecture rules" section.

---

## Key Rust dependencies

| Crate | Version | Used for |
|-------|---------|----------|
| `clap` | 4 | CLI argument parsing |
| `owo-colors` | 4 | Coloured terminal output |
| `toml_edit` | 0.22 | freight.toml parsing and mutation |
| `serde` | 1 | Deserialization of manifests and templates |
| `rayon` | 1 | Parallel source compilation |
| `walkdir` | 2 | Source file discovery |
| `regex` | 1 | Version extraction, doc comment scanning |
| `semver` | 1 | Dependency version parsing |
| `pulldown-cmark` | 0.12 | Markdown processing in `doc/markdown.rs` |
| `thiserror` | 1 | Error types in `freight-core` |
| `tempfile` | 3 | Test helpers |
| `clap_mangen` | 0.2 | Man page generation for `freight man` |
| `rhai` | 1 | Compiler template scripting engine |
| `tower-lsp` | 0.20 | LSP transport in `freight-lsp` |
| `tokio` | 1 | Async runtime for the LSP server |
| `sha2` | 0.10 | SHA-256 verification for HTTP/GitHub deps |
