# crane.dev Registry — Implementation Plan

## Overview

crane.dev is the official package registry for crane. It stores and serves source archives.
Publishers upload a tarball; consumers download it and build locally — crane is a
source-based package manager (like Cargo), not a binary distribution.

This document covers:
- **Server** (`crates/crane-registry/`) — the Axum HTTP service
- **Client** (`crane-core`) — download, resolve, fetch
- **CLI** (`crane`) — wiring the Phase 9 stubs to the real API

---

## Architecture

```
crane add mylib@1.0                 crane publish
        │                                  │
        ▼                                  ▼
┌──────────────────────────────────────────────────┐
│  crane.dev  (Axum HTTP, tokio)                   │
│                                                  │
│  GET  /api/v1/packages/{name}                    │
│  GET  /api/v1/packages/{name}/{version}/download │
│  GET  /api/v1/search?q=...                       │
│  POST /api/v1/publish                            │
│  POST /api/v1/yank/{name}/{version}              │
│  DEL  /api/v1/yank/{name}/{version}              │
│                                                  │
│  ┌──────────────────────────────────────────┐   │
│  │  Filesystem storage                       │   │
│  │  registry-data/                           │   │
│  │    index/<name>.json   (versions+meta)    │   │
│  │    packages/<name>/<version>.tar.gz       │   │
│  │    tokens.toml         (bearer auth)      │   │
│  └──────────────────────────────────────────┘   │
└──────────────────────────────────────────────────┘
        │ tar.gz + SHA-256
        ▼
.deps/<name>/   (extracted source)
      └── build via foreign build system detection
```

---

## Package model

A crane package is a source archive with `crane.toml` at the top level. It:
- Must contain every source file needed to build
- Must NOT contain `target/`, `.crane-build/`, or `.deps/`
- Is produced by `crane publish` via `tar --exclude=target --exclude=.deps -czf`
- Is identified by `[package].name` + `[package].version` in the bundled `crane.toml`

Semver versioning is enforced. The registry rejects non-semver versions at publish time.

---

## Storage layout

```
registry-data/
  index/
    mylib.json       # all version records for "mylib"
    fmt.json
  packages/
    mylib/
      1.0.0.tar.gz
      1.0.1.tar.gz
    fmt/
      10.2.1.tar.gz
  tokens.toml        # bearer token → owner
```

### Index file (`index/<name>.json`)

One file per package name. Append-only in practice — publish adds a record,
yank flips a flag. The file is locked with a `flock` during writes to prevent races.

```json
{
  "name": "mylib",
  "description": "A useful library",
  "repository": "https://github.com/alice/mylib",
  "license": "MIT",
  "owner": "alice",
  "versions": [
    {
      "version": "1.0.0",
      "sha256": "abc123...",
      "published_at": "2026-04-26T00:00:00Z",
      "yanked": false,
      "dependencies": [
        { "name": "libfmt", "req": ">=10.0.0" }
      ]
    },
    {
      "version": "1.0.1",
      "sha256": "def456...",
      "published_at": "2026-04-27T00:00:00Z",
      "yanked": false,
      "dependencies": []
    }
  ]
}
```

### Token file (`tokens.toml`)

Simple bearer token → owner mapping. Loaded at startup; reloaded on SIGHUP.
JWT/OAuth can replace this in v2 with no API changes.

```toml
[tokens]
"abc123secret" = "alice"
"def456secret" = "bob"
```

Tokens are generated out-of-band (e.g. `openssl rand -hex 32`) and given to users.

---

## HTTP API

All endpoints are under `/api/v1`. The server always responds with JSON unless
streaming a binary download.

### Read endpoints (no auth)

#### `GET /api/v1/packages/{name}`

Returns all versions (including yanked) and package metadata.

```json
{
  "name": "mylib",
  "description": "A useful library",
  "license": "MIT",
  "repository": "https://github.com/alice/mylib",
  "versions": [
    { "version": "1.0.1", "sha256": "def456...", "yanked": false },
    { "version": "1.0.0", "sha256": "abc123...", "yanked": false }
  ]
}
```

`404` if the package is unknown.

#### `GET /api/v1/packages/{name}/{version}/download`

Streams the `.tar.gz` archive. Sets `Content-Type: application/octet-stream` and
`X-Checksum-SHA256: <hex>` for the client to verify without reading the full body first.

`404` if version unknown. `410 Gone` if yanked (unless the request carries
`Accept: application/x-crane-locked`, which `crane fetch` sends when the version is
pinned in `crane.lock` — yanked versions remain downloadable for locked projects).

#### `GET /api/v1/search?q=<query>[&limit=<n>]`

Scans the `index/` directory in memory (loaded at startup, refreshed on publish).
Returns packages whose name or description contain `q` (case-insensitive substring).
Default limit: 20.

```json
{
  "results": [
    { "name": "mylib", "description": "A useful library", "latest": "1.0.1" }
  ],
  "total": 1
}
```

### Write endpoints (Bearer auth required)

All write endpoints require `Authorization: Bearer <token>`. Returns `401` if the
header is missing, `403` if the token is unknown.

#### `POST /api/v1/publish`

Body: `multipart/form-data` with two parts:
- `manifest` — the `crane.toml` content (text/plain)
- `tarball` — the source archive (application/octet-stream)

Server validation order:
1. Parse + validate `crane.toml` (same logic as `crane check`)
2. Check name is a valid package name (letters, digits, `-`, `_`)
3. Check version is valid semver and not already published
4. If package already exists in the index: verify token owner matches package owner
5. If package is new: register it under this token's owner
6. Compute SHA-256 of the uploaded tarball
7. Write archive to `packages/<name>/<version>.tar.gz`
8. Append version record to `index/<name>.json`

```json
{ "name": "mylib", "version": "1.0.0", "sha256": "abc123..." }
```

`409 Conflict` if the version already exists.
`422 Unprocessable` if `crane.toml` is invalid (includes the validation error messages).

#### `POST /api/v1/yank/{name}/{version}`

Marks the version as yanked. Future `crane add` resolution skips yanked versions.
Existing `crane.lock` pins still download successfully (see download endpoint above).

`403` if the caller is not the package owner.

#### `DELETE /api/v1/yank/{name}/{version}`

Removes yank status. Same ownership check.

---

## crane.lock additions

Registry dep entries get a `source` field that encodes the registry URL:

```toml
version = 1

[[package]]
name         = "myproject"
version      = "0.1.0"
dependencies = ["mylib"]

[[package]]
name         = "mylib"
version      = "1.0.1"
source       = "registry+https://crane.dev"
checksum     = "def456..."
dependencies = []
```

The `source` field format is `registry+<url>`. This allows projects to mix packages
from crane.dev with packages from private self-hosted registries.

---

## Client-side changes (`crane-core`)

### New module: `src/registry.rs`

```rust
pub struct RegistryClient {
    registry_url: String,   // e.g. "https://crane.dev"
    token: Option<String>,  // from ~/.crane/credentials.toml
}

impl RegistryClient {
    /// Fetch all version records for a package.
    pub fn package_info(name) -> Result<PackageInfo, CraneError>

    /// Resolve the highest non-yanked version satisfying a semver req.
    pub fn resolve_version(name, req) -> Result<(String, String), CraneError>
    //                                            ↑version  ↑sha256

    /// Download and extract a specific version to .deps/<name>/.
    /// Skips if .crane-fetched sentinel already exists.
    pub fn fetch_dep(name, version, sha256, project_dir) -> Result<PathBuf, CraneError>

    /// Upload a package tarball to the registry.
    pub fn publish(manifest_src, tarball_bytes) -> Result<PublishResult, CraneError>

    /// Search for packages matching a query string.
    pub fn search(query, limit) -> Result<Vec<SearchResult>, CraneError>
}
```

`fetch_dep` reuses the same `--strip-components=1` extraction + `.crane-fetched`
sentinel pattern as `build/http.rs`. The only difference is the download source:
`GET /api/v1/packages/{name}/{version}/download` instead of a direct URL.

### Config resolution

`RegistryClient::new()` reads:
1. `CRANE_REGISTRY_URL` env var → override the default `https://crane.dev`
2. `~/.crane/credentials.toml` → look up token for the resolved URL

```toml
# ~/.crane/credentials.toml
[registries]
"https://crane.dev" = "abc123secret"
"https://registry.mycompany.com" = "xyz789secret"
```

### Wiring into `build_foreign_deps`

Registry deps are identified by `source = "registry+..."` in `crane.lock`. The build
step calls `RegistryClient::fetch_dep` for each such entry, then treats the extracted
directory exactly like an http dep (foreign build system detection → compile → link).

### Semver resolution in `crane add`

`crane add mylib@1.0`:
1. `RegistryClient::resolve_version("mylib", "^1.0")` → `("1.0.2", "sha256...")`
2. Write `mylib = "1.0"` to `[dependencies]` (user-facing range, not pinned)
3. Append to `crane.lock`: `name=mylib version=1.0.2 source=registry+https://crane.dev checksum=sha256...`

`crane add mylib` (no version) → resolve to `*` → pick latest non-yanked.

---

## CLI wiring (`crane`)

Phase 9 left these commands as stubs. Each wires to a `RegistryClient` method:

| Command | Registry call | Notes |
|---|---|---|
| `crane add <name>[@ver]` | `resolve_version` + lock write | Already does path/git/system; add registry path |
| `crane fetch` | `fetch_dep` for registry lock entries | Already handles git + http deps |
| `crane search <query>` | `search` | Print formatted results table |
| `crane info <name>` | `package_info` | Print version list + description |
| `crane login` | Write credentials file | Interactive token prompt |
| `crane publish` | Bundle + `publish` | Needs tarball assembly step |
| `crane yank <name> <version>` | `yank` / `unyank` | Add `--undo` flag for unyank |

---

## New crate: `crates/crane-registry`

Binary crate (not a library — the server has no consumers). Workspace entry:

```toml
# Cargo.toml
members = [..., "crates/crane-registry"]

[workspace.dependencies]
axum        = "0.7"
tower-http  = { version = "0.5", features = ["fs", "trace", "cors"] }
```

```
crates/crane-registry/
  src/
    main.rs       # CLI: crane-registry serve [--data <dir>] [--addr <addr>]
    server.rs     # axum router setup, state initialization
    index.rs      # IndexStore: load/save/search index files + flock writes
    storage.rs    # archive read/write under packages/<name>/<ver>.tar.gz
    auth.rs       # token validation middleware
    handlers/
      mod.rs
      packages.rs # GET /packages/{name}, GET /packages/{name}/{ver}/download
      search.rs   # GET /search
      publish.rs  # POST /publish
      yank.rs     # POST/DELETE /yank/{name}/{version}
```

### State

```rust
pub struct AppState {
    pub data_dir: PathBuf,
    pub index: Arc<RwLock<IndexStore>>,
    pub tokens: Arc<RwLock<TokenStore>>,
}
```

`IndexStore` is an in-memory map of `name → PackageIndex` loaded from disk at startup
and updated on each publish. `RwLock` gives concurrent reads with exclusive writes.

### Error handling

All handler errors return JSON:
```json
{ "error": "package 'mylib' version '1.0.0' already exists" }
```

Use a custom `ApiError` type that implements `IntoResponse`.

---

## Implementation order

### Step 1 — Crate scaffold + read-only API

Goal: `cargo run -p crane-registry -- serve` starts a server that can answer read queries.

- [ ] Add `crates/crane-registry/` to workspace
- [ ] `main.rs` + `server.rs`: Axum router, bind address from env/args, graceful shutdown
- [ ] `index.rs`: load index files from disk, in-memory search
- [ ] `storage.rs`: stream archive files from disk
- [ ] `handlers/packages.rs`: `GET /api/v1/packages/{name}` and `GET .../download`
- [ ] `handlers/search.rs`: `GET /api/v1/search?q=...`
- [ ] Integration tests: spin up on a random port, seed test data, assert responses

### Step 2 — Auth middleware + write API

Goal: `crane publish` and `crane yank` work end-to-end with a test token.

- [ ] `auth.rs`: extract `Authorization: Bearer` header, look up in `TokenStore`
- [ ] `tokens.toml` loading + SIGHUP reload
- [ ] `handlers/publish.rs`: multipart parse, `crane.toml` validation, SHA-256, write
- [ ] `handlers/yank.rs`: flip `yanked` flag in index, ownership check
- [ ] Concurrent write safety: `flock` on index file during publish/yank
- [ ] Integration tests: publish → fetch → yank → verify yank blocks resolution

### Step 3 — Client download + `crane fetch`

Goal: `crane build` on a project with a registry dep fetches and builds it automatically.

- [ ] `crane-core/src/registry.rs`: `RegistryClient`, `fetch_dep`
- [ ] Wire `LockPackage` with `source = "registry+..."` into `build_foreign_deps`
- [ ] `crane fetch` pre-fetches registry deps (same as git/http deps)
- [ ] Integration test: publish a tiny C library, add it to a test project, `crane build`

### Step 4 — `crane add` semver resolution

Goal: `crane add mylib@1.0` resolves, pins, writes manifest + lock.

- [ ] `RegistryClient::resolve_version` — fetch index, semver filter, return best match
- [ ] Update `cmd_add` to accept the registry path (currently only path/git/system)
- [ ] `crane.lock` write for registry deps
- [ ] Integration test: publish two versions, `crane add` resolves to the higher one

### Step 5 — `crane login` / `crane publish` / `crane yank`

Goal: the full publisher workflow works from the CLI.

- [ ] `cmd_login`: prompt, validate token against `GET /api/v1/auth/whoami`, write credentials
- [ ] `cmd_publish`: `tar` bundling respecting `target/` and `.crane-build/` exclusions, POST
- [ ] `cmd_yank`: `POST/DELETE /api/v1/yank/{name}/{version}` with `--undo` flag
- [ ] `crane publish` dry-run: `--dry-run` prints what would be uploaded without sending

### Step 6 — `crane search` / `crane info`

Goal: package discovery works from the CLI.

- [ ] `cmd_search`: tabular output (name, latest version, description truncated to 60 chars)
- [ ] `cmd_info`: detailed output (all versions, published dates, yank status, deps)

---

## Open questions

**1. Tarball bundling scope**
Should `crane publish` bundle everything except `target/` and `.deps/`, or should
`crane.toml` support an explicit `[publish] include = [...]` / `exclude = [...]`?
Cargo uses include/exclude lists. v1 will use a fixed exclusion list; manifest keys
can be added in v2 without breaking published packages.

**2. Flat vs namespaced names**
`mylib` (flat, first-publisher-wins) vs `alice/mylib` (namespaced). Cargo uses flat names
with crates.io enforcing first-publisher ownership. Namespaces are cleaner but require more
thought in resolution and manifest syntax. v1 uses flat names; namespaces are additive later.

**3. Transitive resolution**
v1 resolves only direct deps. Full transitive SAT solving (like Cargo's pubgrub) is deferred.
Projects that need a specific transitive version can pin it as a direct dep in their
`crane.toml`. A proper resolver unblocks this in v2.

**4. Index caching**
Should `crane add` / `crane search` cache the package index locally in
`~/.crane/registry-cache/` to work offline? v1 will always fetch from the server (fast
enough for an index file). ETag-based conditional fetch + local cache comes in v2.

**5. Self-hosting**
The `crane-registry` binary should be deployable with zero config beyond a data directory:
```
CRANE_REGISTRY_DATA=/data crane-registry serve
```
TLS is handled by a reverse proxy (nginx/caddy). The binary will ship a Dockerfile.
`CRANE_REGISTRY_URL` on the client side is how users point `crane` at a private registry.
