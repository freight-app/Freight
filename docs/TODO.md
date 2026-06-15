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

## Done

### ~~B5 — System lib cache~~
Resolution chain is `pkg-config → system stubs → registry`. Conan and vcpkg dropped.
`PkgConfigCache` persists probe results to `target/.pkg-config-cache.msgpack` —
hits store flags+version, and **misses are cached negatively** so a dep that falls
through to a stub/registry doesn't re-run `pkg-config` every build. Wiped by
`freight clean`.

### ~~S15 — Workspace improvements~~
- Per-member `freight build -p` / `freight run -p`.
- `freight workspace graph` — visualises inter-member path-dep relationships.
- `[patch]` table — path override anywhere in the graph (incl. transitive).

### ~~Cargo parity~~
Workspace inheritance (`[workspace.dependencies]` / `[workspace.package]`),
`freight metadata`, `[[bin]] required-features`, `[package] default-run`,
`[[example]]` targets + `examples/`, `--offline`/`--locked`/`--frozen`,
`[alias]`, and `freight tree` all-kinds/`--depth`. See the
"Cargo Parity" section of [`roadmap.md`](roadmap.md) and
[`cargo-vs-freight.md`](cargo-vs-freight.md).
