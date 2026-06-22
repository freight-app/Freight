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
│   └── freight/                # package `freight`, library crate `freight_core`, CLI binary `freight`
│       └── src/
│           ├── lib.rs          # build engine public API; emits BuildEvent, no CLI printing
│           ├── bin/freight/    # clap dispatch, commands, LSP, DAP, TUI, output formatting
│           ├── build/          # compile/link/dependency/workspace orchestration
│           ├── manifest/       # freight.toml parsing, workspace parsing, validation
│           ├── toolchain/      # compiler/debugger/tool template detection
│           ├── registry/       # package registry clients and repo dispatch
│           ├── fetch/          # git and URL/archive fetching into .pkgs/
│           ├── doc/            # dependency documentation browser/rendering
│           └── resolve/        # dependency resolution (pkg-config, system libs, build-dep bootstrap)
├── toolchains/                 # compiler, debugger, formatter, linter templates (.rhai) + system-lib stubs (.toml)
│   ├── system-libs/            # freight.toml-compatible stubs for well-known OS libraries
│   │   ├── pthread.toml        # Linux/macOS POSIX threads
│   │   ├── ws2_32.toml         # Windows Winsock2
│   │   └── …                   # 24 built-in stubs total (Linux, macOS, Windows)
│   ├── gnu/
│   │   ├── _gnu-base.rhai   # shared flags/toolset included by gnu compiler files
│   │   ├── g++.rhai
│   │   ├── gcc.rhai
│   │   ├── gfortran.rhai
│   │   ├── gdc.rhai         # D (GCC frontend)
│   │   └── gdb.rhai         # kind = "debugger"
│   ├── llvm/
│   │   ├── _llvm-base.rhai
│   │   ├── clang++.rhai
│   │   ├── clang.rhai
│   │   ├── flang.rhai
│   │   ├── ldc2.rhai        # D (LLVM frontend)
│   │   ├── lldb.rhai        # kind = "debugger"
│   │   ├── clang-format.rhai # kind = "formatter"
│   │   └── clang-tidy.rhai  # kind = "linter"
│   ├── nvidia/
│   │   ├── _nvhpc-base.rhai
│   │   ├── nvc++.rhai
│   │   ├── nvc.rhai
│   │   ├── nvfortran.rhai
│   │   └── nvcc.rhai        # requires_toolchain = ["cpp"]
│   ├── intel/
│   │   ├── _intel-base.rhai
│   │   ├── icpx.rhai
│   │   ├── ifx.rhai
│   │   └── ispc.rhai        # requires_toolchain = ["cpp"]
│   ├── amd/
│   │   └── hipcc.rhai       # requires_toolchain = ["cpp"]
│   ├── asm/
│   │   ├── _asm-base.rhai
│   │   ├── nasm.rhai
│   │   └── yasm.rhai
│   ├── languages/
│   │   ├── _cpp.rhai        # extensions, defaults, standards, linking for C++
│   │   ├── _c.rhai          # extensions, defaults, standards for C
│   │   └── _fortran.rhai    # extensions, defaults, standards, linking for Fortran
│   ├── astyle/
│   │   └── astyle.rhai      # kind = "formatter"
│   ├── uncrustify/
│   │   └── uncrustify.rhai  # kind = "formatter"
│   ├── fprettify/
│   │   └── fprettify.rhai   # kind = "formatter"  (Fortran)
│   ├── cppcheck/
│   │   └── cppcheck.rhai    # kind = "linter"
│   ├── cpplint/
│   │   └── cpplint.rhai     # kind = "linter"
│   ├── flawfinder/
│   │   └── flawfinder.rhai  # kind = "linter"
│   ├── dmd.rhai             # D reference compiler
│   ├── msvc.rhai
│   ├── tcc.rhai
│   └── opencl.rhai          # requires_toolchain = ["cpp"]
└── examples/                   # every example is buildable via `freight build`
    ├── c/hello/
    ├── cpp/hello/
    ├── cpp/modules/
    ├── cpp/multi-bin/
    ├── assembly/hello/
    ├── mixed/c-cpp/
    ├── mixed/tri-lang/
    ├── deps/cmake/
    ├── deps/make/
    ├── deps/git/
    ├── deps/external/
    └── misc/doc/
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

## Build pipeline (Mermaid)

```mermaid
flowchart TD
    Start["freight build"]

    Start --> ParseManifest["1. Parse & validate freight.toml"]
    ParseManifest --> DetectToolchain["2. Detect toolchain\nprobe $PATH + evaluate .rhai scripts\nconsult version cache"]
    DetectToolchain --> ResolveFeatures["3. Resolve features\nexpand dep:name activations\nreject cycles"]
    ResolveFeatures --> CollectDeps["4. Collect dependencies\nmerge base + os.* + arch.*\napply feature / os / arch filters"]
    CollectDeps --> ResolveDeps["5. Resolve dep graph\ntopo-sort\n(see Dependency resolution diagram)"]
    ResolveDeps --> FetchBuildDeps["6. Fetch build-dependencies\nprepend bin/ dirs to PATH"]
    FetchBuildDeps --> ForeignDeps["7. Build foreign deps\ncmake / make / meson / autotools\nrayon parallel"]
    ForeignDeps --> DiscoverSrc["8. Discover sources\nwalk src/ by extension → language key\nSRC.src = entry-point (linker dedup only)"]
    DiscoverSrc --> ModuleScan{"C++20 modules?"}
    ModuleScan -->|yes| ScanModules["9a. Scan export module / import\nbuild module DAG\nbatch by topo-order (Kahn)"]
    ModuleScan -->|no| Compile
    ScanModules --> Compile["10. Compile\nrayon parallel (flat) or module batches\none .o (+ .pcm) per source file"]
    Compile --> Link["11. Link\n.o + dep .a → binary / .a / .so\neach [[bin]] links its own entry-point .o only"]
    Link --> Done["Done — emit BuildEvent::Finished"]
```

---

## Dependency resolution

```mermaid
flowchart TD
    Dep["Dependency entry in freight.toml"]

    Dep --> HasPath{"path =?"}
    HasPath -->|yes| PathDep["Local path dep\nauto-detect freight.toml vs foreign\nbuild in-place"]

    HasPath -->|no| HasGit{"git =?"}
    HasGit -->|yes| GitDep["Clone / update repo\nbranch / tag / rev\ncache under .pkgs/"]

    HasGit -->|no| HasUrl{"url =?"}
    HasUrl -->|yes| UrlDep["Download archive\nSHA-256 verify\nextract to .pkgs/"]

    HasUrl -->|no| HasSystem{"system =?"}
    HasSystem -->|yes| SystemDep["Link -l<name> directly\nno fetch, no build"]

    HasSystem -->|no| VersionDep["Version dep\nconstraint: 1.2 / >=1.0 / *"]
    VersionDep --> PkgConfig["1. pkg-config\nquery system for name + version"]
    PkgConfig -->|found| UseSystem["use system library\n(-I, -L, -l from pkg-config)"]
    PkgConfig -->|miss| Stubs["2. System stubs\nhardcoded map: pthread, m, ws2_32, dl, rt, …"]
    Stubs -->|found| UseStub["link stub flags"]
    Stubs -->|miss| Registry["3. Registry / .pkgs/ cache\nfetch from freight registry if absent"]
    Registry --> UseCache["build from .pkgs/ → link .a"]
```

---

## CLI commands

```mermaid
flowchart LR
    subgraph Build["Build & run"]
        B["freight build\n--release / --debug\n--jobs N / --features …"]
        R["freight run -- args"]
        T["freight test"]
        F["freight fetch"]
    end

    subgraph Tools["Tooling"]
        D["freight doc"]
        N["freight new name"]
        U["freight update"]
        P["freight publish"]
    end

    subgraph Servers["Servers (stdio)"]
        L["freight lsp\nLSP multiplexer\n(clangd + fortls + asm-lsp)"]
        DA["freight dap\nDAP proxy\n(lldb-dap / gdb-mi)"]
    end
```

---

## Compiler template evaluation

```mermaid
flowchart TD
    Probe["Toolchain detection\nprobe $PATH: gcc, clang, gfortran, …\nrun version probe, consult version cache"]

    Probe --> Load["Load .rhai script\ntoolchains/<vendor>/<compiler>.rhai\n#include _<vendor>-base.rhai"]

    Load --> Ctx["Build ctx object\nctx.value, ctx.version\nctx.arch, ctx.os"]

    Ctx --> Eval["Evaluate callbacks\ncompiler_option(name, fn(ctx))\nlanguage_option(lang, name, fn(ctx))\ncallback calls add_flag(s)"]

    Eval --> Flags["Collect flags per source file\n-O2 / -g, -std=c++20, -march=…\n-fsanitize=…, -D<FEATURE>, …"]

    Flags --> Cmd["Inject into compile command\none invocation per .o"]
```

---

## DAP architecture

### Adapter selection

```mermaid
flowchart TD
    DapStart["freight dap (stdio)"]

    DapStart --> Detect["Detect target binary\nread freight.toml in cwd"]
    Detect --> Probe2{"Probe debuggers in $PATH"}

    Probe2 -->|"lldb-vscode or lldb-dap"| LLDB["LLDB adapter\n(preferred on macOS + Linux)"]
    Probe2 -->|"gdb"| GDB["GDB + MI adapter\n(Linux / Windows fallback)"]
    Probe2 -->|"neither"| Err["Error: no debugger found"]
```

### Launch / attach sequence

```mermaid
sequenceDiagram
    participant IDE as IDE (VS Code)
    participant Freight as freight dap
    participant Adapter as lldb-dap / gdb

    IDE->>Freight: initialize {}
    Freight->>Freight: detect binary + debugger
    Freight-->>IDE: initialize response + capabilities

    alt launch
        IDE->>Freight: launch { program, args, cwd }
        Freight->>Adapter: spawn adapter process
        Freight->>Adapter: forward launch request
        Adapter-->>Freight: launch response
        Freight-->>IDE: launch response
    else attach
        IDE->>Freight: attach { pid }
        Freight->>Adapter: spawn adapter process
        Freight->>Adapter: forward attach { pid }
        Adapter-->>Freight: attach response
        Freight-->>IDE: attach response
    end

    loop debug session
        IDE->>Freight: setBreakpoints / next / continue / …
        Freight->>Adapter: forward
        Adapter-->>Freight: response / event
        Freight-->>IDE: forward
    end

    IDE->>Freight: disconnect
    Freight->>Adapter: terminate
```

---

## Registry server

### HTTP router

```mermaid
flowchart LR
    Client["freight / browser"]

    Client --> Auth["POST /v1/auth/login"]
    Client --> Publish["PUT /v1/packages/:name/:version"]
    Client --> Download["GET /v1/packages/:name/:version/download"]
    Client --> Search["GET /v1/packages?q=…"]
    Client --> Meta["GET /v1/packages/:name"]
    Client --> TokenMgmt["POST/DELETE /v1/tokens"]
```

### Publish wire format

```mermaid
flowchart LR
    Wire["TCP stream"]
    Wire --> JLen["u32 LE — JSON length"]
    JLen --> JSON["JSON bytes\n{ name, version, deps, features, … }"]
    JSON --> TLen["u32 LE — tarball length"]
    TLen --> Tar["tarball bytes (source archive)"]
```

### Auth flow

```mermaid
flowchart TD
    Login["Client: POST /v1/auth/login\n{ user, password_plaintext }"]
    Login --> SHA["Client-side: SHA-256(plaintext)\n→ password_sha256"]
    SHA --> Send["Send { user, password_sha256 }"]
    Send --> Argon["Server: Argon2id verify\nArgon2id(sha256) vs stored hash"]
    Argon -->|match| Token["Issue raw_token (random)\nstore token_hash = SHA-256(raw_token)\nreturn raw_token"]
    Argon -->|mismatch| Reject["401 Unauthorized"]
    Token --> Use["Client: Authorization: Bearer raw_token"]
    Use --> Verify["Server: SHA-256(raw_token) vs token_hash"]
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
| `thiserror` | 1 | Error types in `freight` |
| `tempfile` | 3 | Test helpers |
| `clap_mangen` | 0.2 | Man page generation for `freight doc --man` |
| `rhai` | 1 | Compiler template scripting engine |
| `tower-lsp` | 0.20 | LSP transport in `freight-lsp` |
| `tokio` | 1 | Async runtime for the LSP server |
| `sha2` | 0.10 | SHA-256 verification for HTTP/GitHub deps |
