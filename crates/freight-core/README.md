# freight-core

The core library crate for freight. Everything that is not CLI I/O lives here.

## Module overview

| Path | Responsibility |
|---|---|
| `manifest/` | `freight.toml` parsing, validation, and type definitions (`Manifest`, `Dependency`, `BuildSettings`) |
| `build/` | Compiler invocation, dependency graph, parallel compilation, linking, incremental dirty checking |
| `toolchain/` | Compiler template loading (`.rhai` scripts), detection, version caching, global config |
| `registry/` | `PackageRepo` trait, `FreightRegistry` HTTP client (search/lookup/download/publish/yank/register), multi-registry support |
| `dep_cmds.rs` | Fetch orchestration for all dep types — git, url, path, registry |
| `lock.rs` | `freight.lock` read/write, `upsert_registry_dep` |
| `error.rs` | `FreightError` enum (`thiserror`) |
| `event.rs` | `BuildEvent` — structured build progress callbacks for CLI / GUI consumers |
| `supports.rs` | Platform expression evaluator (`os`, `arch`, `targets`) |
| `fetch/` | HTTP tarball download, SHA-256 verification, `.freight-fetched` sentinel |
| `new.rs` | Project scaffolding templates |
| `install.rs` | Install / package logic |

## Key types

- **`Manifest`** — deserialized `freight.toml`; `build_settings_for(profile)` produces `BuildSettings`
- **`FreightRegistry`** — HTTP client backed by `curl`; implements `PackageRepo` (read) plus write methods (`download_tarball`, `publish_package`, `yank_version`, `register_user`)
- **`PackageRepo`** — trait for `lookup(name)` and `search(query)`; allows multiple registry backends
- **`GlobalConfig`** — `~/.freight/config.toml` + credentials overlay; `save_credential` writes to `~/.freight/credentials.toml`
- **`BuildEvent`** — emitted by the build engine; `Progress = Arc<dyn Fn(BuildEvent) + Send + Sync>`
- **`FreightError`** — top-level error type; most public functions return `Result<_, FreightError>`

## Registry wire protocol

`FreightRegistry` speaks the same HTTP wire protocol as `freight.dev`:

- `GET  /api/v1/packages/:name` — lookup
- `GET  /api/v1/search?q=` — search
- `GET  /api/v1/packages/:name/:version/download` — tarball (returns `X-Checksum-SHA256` header)
- `PUT  /api/v1/packages` — publish (`[u32 JSON len][JSON][u32 tar len][tar]`)
- `DELETE/PUT /api/v1/packages/:name/:version/yank` — yank / unyank
- `POST /api/v1/users/register` — create account

All authenticated calls send `Authorization: Bearer <token>`.
