# TODO

## Open

### B2 — VS Code extension
Activate on `freight.toml`, delegate to `freight lsp` for diagnostics, completions, and
go-to-definition. LSP is implemented; remaining work:
- Extension packaging and Marketplace publish
- Inlay hints for dep versions and feature flags
- `freight.toml` schema validation (JSON Schema or custom)

### B5 — System lib cache (remaining)
Resolution chain is `pkg-config → system stubs → registry`. Conan and vcpkg dropped.
Remaining: cache discovered lib flags on first probe so subsequent rebuilds skip
re-running `pkg-config` on every invocation.

### S15 — Workspace improvements (remaining)
Per-member `freight build -p` / `freight run -p` is done.
Remaining:
- Workspace-level `[patch]` table to override transitive deps
- `freight workspace graph` — visualise inter-member dep relationships
