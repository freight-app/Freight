# Freight LSP Architecture

The freight language server is a **multiplexing proxy** that sits between VS Code and
the underlying language servers (clangd, fortls, asm-lsp). It intercepts requests it
can answer itself, enriches or filters what passes through, and owns all
include/import-related features entirely.

---

## Overall architecture

```mermaid
graph TD
    VSCode["VS Code / LSP client"]

    subgraph FreightLSP["freight lsp (multiplexer)"]
        Router["Request router\n(match method)"]
        DocIndex["DocIndex\n(docify symbols)"]
        HeaderIndex["HeaderIndex\n(include → package)"]
        PendingMap["clangd_pending\n(inlay hint intercepts)"]
    end

    subgraph Passthroughs["Passthrough servers"]
        Clangd["clangd\n(C / C++ / CUDA / ObjC)"]
        Fortls["fortls\n(Fortran)"]
        AsmLsp["asm-lsp\n(Assembly)"]
    end

    VSCode -->|"JSON-RPC (stdio)"| Router
    Router -->|"freight-owned response"| VSCode
    Router -->|"forward"| Clangd
    Router -->|"forward"| Fortls
    Router -->|"forward"| AsmLsp
    Clangd -->|"response (reader thread)"| PendingMap
    PendingMap -->|"merged + filtered"| VSCode
    Fortls  -->|"response (reader thread)"| VSCode
    AsmLsp  -->|"response (reader thread)"| VSCode
    Router  --> DocIndex
    Router  --> HeaderIndex
```

---

## Request routing

Which handler owns each LSP method:

```mermaid
flowchart TD
    Req["Incoming request"]

    Req --> IsManifest{"freight.toml\nfile?"}

    IsManifest -->|yes| ManifestHandler["freight handles\n(completion, hover,\ndiagnostics, sig-help)"]

    IsManifest -->|no| MethodSwitch{"method"}

    MethodSwitch -->|"hover"| HoverPipeline["Hover pipeline\n(see below)"]
    MethodSwitch -->|"inlayHint"| InlayPipeline["Inlay hint pipeline\n(see below)"]
    MethodSwitch -->|"definition\ndeclaration"| DefHandler{"cursor on\n#include line?"}
    DefHandler -->|yes| FreightDef["freight → open header file"]
    DefHandler -->|no| PassDef["forward → clangd / fortls"]
    MethodSwitch -->|"documentLink"| FreightLinks["freight → links for\nevery include line"]
    MethodSwitch -->|"completion\nsignatureHelp\ncodeAction"| PassComp["forward by file extension"]
    MethodSwitch -->|"didOpen\ndidSave\ndidChange"| SyncHandler["freight updates state\nthen forward to all"]
    MethodSwitch -->|"everything else"| ForwardAll["forward → all passthroughs"]
```

---

## File extension → passthrough server

```mermaid
flowchart LR
    ext{"file extension"}

    ext -->|".c .h .cc .cpp .hpp\n.cxx .cu .hip .m .mm\n.cl .ispc .cppm .ixx"| Clangd["clangd"]
    ext -->|".f .for .ftn .f90\n.f95 .f03 .f08 etc."| Fortls["fortls"]
    ext -->|".asm .nasm .s .S"| AsmLsp["asm-lsp"]
    ext -->|"freight.toml"| Freight["freight (built-in)"]
    ext -->|"other"| Null["null response"]
```

---

## Hover pipeline

```mermaid
flowchart TD
    H["textDocument/hover\n(cursor position)"]

    H --> IsManifest{"freight.toml?"}
    IsManifest -->|yes| ManifestHover["freight manifest hover\n(dep version, key docs)"]

    IsManifest -->|no| Step1["1. include_hover\nparse #include line at cursor\nlook up HeaderIndex"]
    Step1 -->|found| IncludeHover["return markdown:\nheader · package · full path"]
    Step1 -->|not an include| Step2["2. doc_hover\nlook up DocIndex\nby position, then by name"]
    Step2 -->|found| DocHover["return markdown:\nsignature · brief · params · returns"]
    Step2 -->|miss| Step3{"file type?"}
    Step3 -->|"C/C++\n(clangd)"| ClangdHover["forward → clangd\n(types, stl docs)"]
    Step3 -->|"Fortran"| FortlsHover["forward → fortls"]
    Step3 -->|"Assembly"| AsmHover["forward → asm-lsp"]
    Step3 -->|"other"| NullHover["null"]
```

---

## Inlay hint pipeline

```mermaid
sequenceDiagram
    participant VSCode as VS Code
    participant Freight as freight lsp
    participant Pending as clangd_pending map
    participant Clangd as clangd

    VSCode->>Freight: textDocument/inlayHint {id: 42, range}

    Freight->>Freight: compute_inlay_hints()<br/>scan each line in range<br/>parse #include / import<br/>look up HeaderIndex + system dirs

    alt file goes to clangd
        Freight->>Pending: store (id=42, our_hints) under "__freight_inlayhint_42"
        Freight->>Clangd: forward request {id: "__freight_inlayhint_42"}
        Clangd-->>Freight: response {id: "__freight_inlayhint_42", result: [...clangd hints]}
        Freight->>Freight: reader thread intercepts<br/>remove hints on include lines<br/>append our_hints
        Freight-->>VSCode: response {id: 42, result: [clangd hints (filtered) + ← pkg hints]}
    else no clangd (Fortran / asm / no server)
        Freight-->>VSCode: response {id: 42, result: [← pkg hints only]}
    end
```

---

## Doc index rebuild

```mermaid
flowchart TD
    Trigger["Trigger:\n• initialize\n• freight.toml saved\n• first file opened (manifest discovered)"]

    Trigger --> FindManifest["active_manifest_dir()\nwalk up from root or opened file"]
    FindManifest -->|None| LogSkip["log: no manifest dir — skip"]
    FindManifest -->|found| GenCC["generate_lsp_compile_commands_at()\nwrite compile_commands.json"]
    GenCC --> BuildIndex["DocIndex::build_freight_packages()\nwalk src/ include/ inc/\nrun docify extractors per language"]
    BuildIndex --> StoreIndex["store in Arc<Mutex<Option<DocIndex>>>"]
    StoreIndex --> BuildHeader["HeaderIndex::build()\nwalk include/ dirs per package\nwalk .pkgs/ cache\nprobe system include dirs via gcc -v"]
    BuildHeader --> Notify["send freight/docIndexUpdated {items: N}\n→ VS Code status bar updates"]
```

---

## Include / import ownership

Summary of which layer owns each feature for `#include` / `import` lines:

```mermaid
flowchart LR
    subgraph Freight["freight (owns)"]
        Hover["hover → package origin card"]
        InlayHint["inlay hint → ← pkg-version"]
        Definition["go-to-definition → open header"]
        DocLink["document link → clickable header name"]
    end

    subgraph Clangd["clangd (filtered out)"]
        ClangdHints["inlay hints on include lines\n(stripped before client sees them)"]
        ClangdDef["definition on include lines\n(intercepted, never forwarded)"]
    end

    subgraph ClangdKept["clangd (kept)"]
        ParamHints["parameter name inlay hints"]
        TypeHints["deduced type hints"]
        OtherDef["go-to-definition (non-include)"]
        Diagnostics["diagnostics / errors"]
        Completions["completions"]
    end
```
