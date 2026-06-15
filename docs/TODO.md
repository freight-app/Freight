# TODO

## Open

### B2 — VS Code extension
Activate on `freight.toml`, delegate to `freight lsp` for diagnostics, completions, and
go-to-definition. LSP is implemented and `editors/vscode-freight/` is scaffolded; remaining work:
- Install extension dependencies and run in an Extension Development Host
- Extension packaging and Marketplace publish
- Inlay hints for dep versions and feature flags
- `freight.toml` schema validation (JSON Schema or custom)

### B3 — Neovim plugin
`editors/nvim-freight/` is scaffolded with built-in LSP startup, Freight commands,
and `freight.toml` write notifications. Remaining work:
- Runtime test in Neovim 0.10+
- Optional Telescope/picker integration for package names and targets
- Keymap recommendations

### ~~B5 — System lib cache~~ (done)
Resolution chain is `pkg-config → system stubs → registry`. Conan and vcpkg dropped.
`PkgConfigCache` persists probe results to `target/.pkg-config-cache.msgpack` —
hits store flags+version, and **misses are cached negatively** so a dep that falls
through to a stub/registry doesn't re-run `pkg-config` every build. Wiped by
`freight clean`.

### S15 — Workspace improvements (remaining)
Per-member `freight build -p` / `freight run -p` is done.
Remaining:
- Workspace-level `[patch]` table to override transitive deps
- `freight workspace graph` — visualise inter-member dep relationships
