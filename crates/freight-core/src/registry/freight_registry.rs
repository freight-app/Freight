//! HTTP client for the freight.dev package registry (and any compatible registry).
//!
//! API contract (all responses are JSON):
//!   GET    /api/v1/packages/{name}                                → ApiPackage (or 404)
//!   GET    /api/v1/search?q={query}                               → ApiSearchResult
//!   GET    /api/v1/packages/{name}/{ver}/download                  → source tarball bytes
//!   PUT    /api/v1/packages                                       → publish source (binary wire format)
//!   DELETE /api/v1/packages/{name}/{ver}/yank                     → yank
//!   PUT    /api/v1/packages/{name}/{ver}/yank                     → unyank
//!   GET    /api/v1/packages/{name}/{ver}/prebuilts                 → list prebuilt triples
//!   GET    /api/v1/packages/{name}/{ver}/prebuilt/{triple}/download → prebuilt tarball
//!   PUT    /api/v1/packages/{name}/{ver}/prebuilt/{triple}         → upload prebuilt
//!
//! The default registry is `https://freight.dev`. Additional registries are
//! configured via `[[registry]]` entries in the config file.

use std::path::Path;

use curl::easy::{Easy, List};
use serde::Deserialize;

use crate::error::FreightError;
use crate::toolchain::cache::RegistryConfig;
use super::{DEFAULT_REGISTRY_URL, PackageInfo, PackageVersion, PackageRepo};

// ── Public client ─────────────────────────────────────────────────────────────

pub struct FreightRegistry {
    /// Registry identifier — empty string for the default `freight.dev` registry.
    name: String,
    base_url: String,
    token: Option<String>,
}

impl FreightRegistry {
    /// Build from a [`RegistryConfig`] entry.
    pub fn from_config(cfg: &RegistryConfig) -> Self {
        Self {
            name: cfg.name.clone(),
            base_url: cfg.url.trim_end_matches('/').to_string(),
            token: cfg.token.clone(),
        }
    }

    /// Default public registry, used when no registries are configured.
    /// Falls back to `FREIGHT_REGISTRY_URL` env var, then `https://freight.dev`.
    pub fn default_registry() -> Self {
        let url = std::env::var("FREIGHT_REGISTRY_URL")
            .unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_string());
        Self {
            name: String::new(),
            base_url: url.trim_end_matches('/').to_string(),
            token: None,
        }
    }
}

impl PackageRepo for FreightRegistry {
    fn repo_key(&self) -> &str {
        // Empty string → stored as a plain version string in freight.toml (default registry).
        // Named registries → stored as `repo = "<name>"`.
        if self.name.is_empty() || self.name == "freight" { "" } else { &self.name }
    }

    fn lookup(&self, name: &str, channel: Option<&str>) -> Result<Option<PackageInfo>, FreightError> {
        let url = match channel {
            Some(ch) => format!("{}/api/v1/packages/{}?channel={}", self.base_url, name, url_encode(ch)),
            None     => format!("{}/api/v1/packages/{}", self.base_url, name),
        };
        match http_get_json::<ApiPackage>(&url, self.token.as_deref()) {
            Ok(pkg)                              => Ok(Some(pkg.into())),
            Err(FreightError::RegistryNotFound(_)) => Ok(None),
            Err(e)                               => Err(e),
        }
    }

    fn search(&self, query: &str) -> Result<Vec<PackageInfo>, FreightError> {
        let url = format!("{}/api/v1/search?q={}", self.base_url, url_encode(query));
        let result = http_get_json::<ApiSearchResult>(&url, self.token.as_deref())?;
        Ok(result.packages.into_iter().map(Into::into).collect())
    }

    fn fetch_readme(&self, name: &str) -> Option<String> {
        let url = format!("{}/api/v1/packages/{}/readme", self.base_url, name);
        http_get(&url, self.token.as_deref()).ok()
    }
}

// ── Write API (publish / yank / download) ────────────────────────────────────

impl FreightRegistry {
    /// Download a specific version's tarball to `.deps/<name>/`.
    ///
    /// Skips if `.deps/<name>/.freight-fetched` already exists.
    /// Returns the SHA-256 checksum (hex) of the downloaded tarball.
    pub fn download_tarball(
        &self,
        name: &str,
        version: &str,
        channel: Option<&str>,
        project_dir: &Path,
    ) -> Result<String, FreightError> {
        let deps_dir = project_dir.join(".deps").join(name);
        let sentinel = deps_dir.join(".freight-fetched");
        if sentinel.exists() {
            // Already fetched — read checksum from sentinel if available.
            return Ok(std::fs::read_to_string(&sentinel)
                .unwrap_or_default()
                .trim()
                .to_string());
        }

        let url = match channel {
            Some(ch) => format!("{}/api/v1/packages/{}/{}/download?channel={}", self.base_url, name, version, url_encode(ch)),
            None     => format!("{}/api/v1/packages/{}/{}/download", self.base_url, name, version),
        };
        let (bytes, checksum_header) = http_get_bytes(&url, self.token.as_deref())?;

        std::fs::create_dir_all(project_dir.join(".deps"))?;
        let archive = project_dir.join(".deps").join(format!("{name}-{version}.tar.gz"));
        std::fs::write(&archive, &bytes)?;

        std::fs::create_dir_all(&deps_dir)?;
        let ok = std::process::Command::new("tar")
            .args([
                "-xf",
                &archive.to_string_lossy(),
                "-C",
                &deps_dir.to_string_lossy(),
                "--strip-components=1",
            ])
            .status()
            .map_err(|e| FreightError::RegistryError(format!("tar not found: {e}")))?
            .success();
        let _ = std::fs::remove_file(&archive);
        if !ok {
            return Err(FreightError::RegistryError(format!(
                "extraction failed for {name}@{version}"
            )));
        }

        // Use the server-supplied checksum when available, else compute locally.
        let checksum = match checksum_header {
            Some(h) => h,
            None => {
                use sha2::{Digest, Sha256};
                let hex: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                hex
            }
        };

        std::fs::write(&sentinel, &checksum)?;
        Ok(checksum)
    }

    /// Upload a new package version using the cargo binary wire format.
    ///
    /// Body layout: `[u32 LE json_len][json bytes][u32 LE tar_len][tar bytes]`
    pub fn publish_package(
        &self,
        name: &str,
        version: &str,
        channel: Option<&str>,
        description: Option<&str>,
        license: Option<&str>,
        tarball: &[u8],
    ) -> Result<(), FreightError> {
        let token = self.token.as_deref().ok_or_else(|| {
            FreightError::RegistryError(
                "no token configured for this registry — run `freight login`".into(),
            )
        })?;

        let meta = serde_json::json!({
            "name": name,
            "vers": version,
            "channel": channel,
            "description": description,
            "license": license,
        });
        let json_bytes = serde_json::to_vec(&meta)
            .map_err(|e| FreightError::RegistryError(format!("serialize metadata: {e}")))?;

        let mut body = Vec::with_capacity(8 + json_bytes.len() + tarball.len());
        body.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(&json_bytes);
        body.extend_from_slice(&(tarball.len() as u32).to_le_bytes());
        body.extend_from_slice(tarball);

        let url = format!("{}/api/v1/packages", self.base_url);
        http_put(&url, Some(token), "application/octet-stream", body)?;
        Ok(())
    }

    /// Yank (`yanked = true`) or unyank (`yanked = false`) a version.
    pub fn yank_version(
        &self,
        name: &str,
        version: &str,
        yanked: bool,
    ) -> Result<(), FreightError> {
        let token = self.token.as_deref().ok_or_else(|| {
            FreightError::RegistryError(
                "no token configured for this registry — run `freight login`".into(),
            )
        })?;
        let url = format!("{}/api/v1/packages/{}/{}/yank", self.base_url, name, version);
        if yanked {
            http_delete(&url, token)?;
        } else {
            http_put(&url, Some(token), "application/json", b"{}".to_vec())?;
        }
        Ok(())
    }

    /// Register a new user account. Returns `(user_id, token)`.
    ///
    /// The password is SHA-256 hashed before transmission so the plaintext
    /// never leaves the client. The server stores Argon2id(SHA-256(password)).
    /// Any future login-with-password call must hash the password the same way.
    pub fn register_user(
        &self,
        username: &str,
        password: &str,
        email: Option<&str>,
        token_name: Option<&str>,
    ) -> Result<(i64, String), FreightError> {
        let pw_hash = sha256_hex(password);
        let body = serde_json::json!({
            "username":   username,
            "password":   pw_hash,
            "email":      email,
            "token_name": token_name,
        });
        let json_bytes = serde_json::to_vec(&body)
            .map_err(|e| FreightError::RegistryError(format!("serialize: {e}")))?;
        let url = format!("{}/api/v1/users/register", self.base_url);
        let resp = http_post(&url, None, "application/json", json_bytes)?;
        let v: serde_json::Value = serde_json::from_str(&resp)
            .map_err(|e| FreightError::RegistryError(format!("invalid JSON: {e}")))?;
        let id = v["id"].as_i64()
            .ok_or_else(|| FreightError::RegistryError("missing id in response".into()))?;
        let token = v["token"].as_str()
            .ok_or_else(|| FreightError::RegistryError("missing token in response".into()))?
            .to_string();
        Ok((id, token))
    }

    /// Returns the source string for the lockfile: `"registry+<url>"`.
    pub fn source_string(&self) -> String {
        format!("registry+{}", self.base_url)
    }

    // ── Prebuilt API ──────────────────────────────────────────────────────────

    /// List the target triples for which a prebuilt tarball is available.
    pub fn list_prebuilt_triples(
        &self,
        name:    &str,
        version: &str,
        channel: Option<&str>,
    ) -> Result<Vec<String>, FreightError> {
        let url = match channel {
            Some(ch) => format!(
                "{}/api/v1/packages/{}/{}/prebuilts?channel={}",
                self.base_url, name, version, url_encode(ch)
            ),
            None => format!("{}/api/v1/packages/{}/{}/prebuilts", self.base_url, name, version),
        };
        #[derive(Deserialize)]
        struct ListResp { prebuilts: Vec<PrebuiltEntry> }
        #[derive(Deserialize)]
        struct PrebuiltEntry { triple: String }
        match http_get_json::<ListResp>(&url, self.token.as_deref()) {
            Ok(r)                                => Ok(r.prebuilts.into_iter().map(|e| e.triple).collect()),
            Err(FreightError::RegistryNotFound(_)) => Ok(vec![]),
            Err(e)                               => Err(e),
        }
    }

    /// Download a prebuilt tarball for `triple` to `.deps/<name>/`.
    ///
    /// Returns the SHA-256 checksum of the downloaded tarball.
    /// The prebuilt tarball is expected to contain `include/`, `lib/`, and
    /// `lib/pkgconfig/` directories that are extracted into `.deps/<name>/`.
    pub fn download_prebuilt(
        &self,
        name:    &str,
        version: &str,
        channel: Option<&str>,
        triple:  &str,
        project_dir: &Path,
    ) -> Result<String, FreightError> {
        let url = match channel {
            Some(ch) => format!(
                "{}/api/v1/packages/{}/{}/prebuilt/{}/download?channel={}",
                self.base_url, name, version, triple, url_encode(ch)
            ),
            None => format!(
                "{}/api/v1/packages/{}/{}/prebuilt/{}/download",
                self.base_url, name, version, triple
            ),
        };
        let (bytes, checksum_header) = http_get_bytes(&url, self.token.as_deref())?;

        let deps_dir = project_dir.join(".deps").join(name);
        std::fs::create_dir_all(&deps_dir)?;

        let archive = project_dir.join(".deps").join(format!("{name}-{version}-{triple}.tar.gz"));
        std::fs::write(&archive, &bytes)?;

        let ok = std::process::Command::new("tar")
            .args(["-xf", &archive.to_string_lossy(), "-C", &deps_dir.to_string_lossy(), "--strip-components=1"])
            .status()
            .map_err(|e| FreightError::RegistryError(format!("tar not found: {e}")))?
            .success();
        let _ = std::fs::remove_file(&archive);
        if !ok {
            return Err(FreightError::RegistryError(format!(
                "extraction failed for prebuilt {name}@{version} ({triple})"
            )));
        }

        let checksum = match checksum_header {
            Some(h) => h,
            None => {
                use sha2::{Digest, Sha256};
                Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
            }
        };

        // Write sentinel so subsequent fetches are skipped.
        let sentinel = deps_dir.join(".freight-fetched");
        std::fs::write(&sentinel, &checksum)?;
        Ok(checksum)
    }

    /// Upload a prebuilt tarball for `triple`.
    pub fn upload_prebuilt(
        &self,
        name:    &str,
        version: &str,
        channel: Option<&str>,
        triple:  &str,
        tarball: &[u8],
    ) -> Result<(), FreightError> {
        let token = self.token.as_deref().ok_or_else(|| {
            FreightError::RegistryError(
                "no token configured for this registry — run `freight login`".into(),
            )
        })?;
        let url = match channel {
            Some(ch) => format!(
                "{}/api/v1/packages/{}/{}/prebuilt/{}?channel={}",
                self.base_url, name, version, triple, url_encode(ch)
            ),
            None => format!(
                "{}/api/v1/packages/{}/{}/prebuilt/{}",
                self.base_url, name, version, triple
            ),
        };
        http_put(&url, Some(token), "application/gzip", tarball.to_vec())?;
        Ok(())
    }
}

// ── API response shapes ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ApiPackage {
    name: String,
    #[serde(default)]
    description: Option<String>,
    latest: String,
    #[serde(default)]
    versions: Vec<ApiVersion>,
}

#[derive(Deserialize)]
struct ApiVersion {
    version: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    prebuilt_triples: Vec<String>,
    #[serde(default)]
    dependencies: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct ApiSearchResult {
    packages: Vec<ApiPackage>,
}

impl From<ApiPackage> for PackageInfo {
    fn from(a: ApiPackage) -> Self {
        Self {
            name: a.name,
            description: a.description,
            latest: a.latest,
            versions: a.versions.into_iter().map(|v| PackageVersion {
                version: v.version,
                checksum: v.checksum,
                download_url: v.download_url,
                prebuilt_triples: v.prebuilt_triples,
                dependencies: v.dependencies,
            }).collect(),
        }
    }
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

fn http_get_json<T: for<'de> Deserialize<'de>>(url: &str, token: Option<&str>) -> Result<T, FreightError> {
    let body = http_get(url, token)?;
    serde_json::from_str(&body).map_err(|e| {
        FreightError::RegistryError(format!("invalid JSON from registry: {e}"))
    })
}

fn http_get(url: &str, token: Option<&str>) -> Result<String, FreightError> {
    let mut body = Vec::new();
    let mut easy = Easy::new();

    easy.url(url)
        .map_err(|e| FreightError::RegistryError(format!("curl url: {e}")))?;
    easy.follow_location(true)
        .map_err(|e| FreightError::RegistryError(format!("curl option: {e}")))?;
    easy.fail_on_error(false) // we check status ourselves
        .map_err(|e| FreightError::RegistryError(format!("curl option: {e}")))?;
    easy.connect_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| FreightError::RegistryError(format!("curl option: {e}")))?;
    easy.timeout(std::time::Duration::from_secs(30))
        .map_err(|e| FreightError::RegistryError(format!("curl option: {e}")))?;
    easy.useragent(&format!("freight/{}", env!("CARGO_PKG_VERSION")))
        .map_err(|e| FreightError::RegistryError(format!("curl option: {e}")))?;

    if let Some(tok) = token {
        let mut headers = curl::easy::List::new();
        headers.append(&format!("Authorization: Bearer {tok}"))
            .map_err(|e| FreightError::RegistryError(format!("curl header: {e}")))?;
        easy.http_headers(headers)
            .map_err(|e| FreightError::RegistryError(format!("curl option: {e}")))?;
    }

    {
        let mut transfer = easy.transfer();
        transfer.write_function(|data| {
            body.extend_from_slice(data);
            Ok(data.len())
        }).map_err(|e| FreightError::RegistryError(format!("curl write: {e}")))?;
        transfer.perform()
            .map_err(|e| FreightError::RegistryError(format!("request failed: {e}")))?;
    }

    let code = easy.response_code()
        .map_err(|e| FreightError::RegistryError(format!("curl response code: {e}")))?;

    match code {
        200 => {}
        404 => return Err(FreightError::RegistryNotFound(url.to_string())),
        _   => return Err(FreightError::RegistryError(format!("registry HTTP {code} for {url}"))),
    }

    String::from_utf8(body)
        .map_err(|_| FreightError::RegistryError("non-UTF-8 response from registry".into()))
}

/// GET a URL, returning raw bytes and the `X-Checksum-SHA256` header if present.
fn http_get_bytes(url: &str, token: Option<&str>) -> Result<(Vec<u8>, Option<String>), FreightError> {
    let mut body = Vec::new();
    let mut checksum_header: Option<String> = None;
    let mut easy = Easy::new();

    easy.url(url).map_err(|e| FreightError::RegistryError(format!("curl url: {e}")))?;
    easy.follow_location(true).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.fail_on_error(false).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.connect_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.timeout(std::time::Duration::from_secs(30))
        .map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.useragent(&format!("freight/{}", env!("CARGO_PKG_VERSION")))
        .map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;

    if let Some(tok) = token {
        let mut hdrs = List::new();
        hdrs.append(&format!("Authorization: Bearer {tok}"))
            .map_err(|e| FreightError::RegistryError(format!("curl header: {e}")))?;
        easy.http_headers(hdrs).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    }

    {
        let mut transfer = easy.transfer();
        transfer.header_function(|header| {
            let s = String::from_utf8_lossy(header).to_lowercase();
            if let Some(rest) = s.strip_prefix("x-checksum-sha256:") {
                checksum_header = Some(rest.trim().to_string());
            }
            true
        }).map_err(|e| FreightError::RegistryError(format!("curl header fn: {e}")))?;
        transfer.write_function(|data| {
            body.extend_from_slice(data);
            Ok(data.len())
        }).map_err(|e| FreightError::RegistryError(format!("curl write: {e}")))?;
        transfer.perform().map_err(|e| FreightError::RegistryError(format!("request: {e}")))?;
    }

    let code = easy.response_code()
        .map_err(|e| FreightError::RegistryError(format!("curl code: {e}")))?;
    match code {
        200 => Ok((body, checksum_header)),
        404 => Err(FreightError::RegistryNotFound(url.to_string())),
        410 => Err(FreightError::RegistryError(format!("version is yanked: {url}"))),
        _   => Err(FreightError::RegistryError(format!("HTTP {code} from {url}"))),
    }
}

fn http_put(url: &str, token: Option<&str>, content_type: &str, body: Vec<u8>) -> Result<String, FreightError> {
    let mut resp = Vec::new();
    let mut easy = Easy::new();

    easy.url(url).map_err(|e| FreightError::RegistryError(format!("curl url: {e}")))?;
    easy.upload(true).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.in_filesize(body.len() as u64).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.fail_on_error(false).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.useragent(&format!("freight/{}", env!("CARGO_PKG_VERSION")))
        .map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;

    let mut hdrs = List::new();
    hdrs.append(&format!("Content-Type: {content_type}"))
        .map_err(|e| FreightError::RegistryError(format!("curl header: {e}")))?;
    if let Some(tok) = token {
        hdrs.append(&format!("Authorization: Bearer {tok}"))
            .map_err(|e| FreightError::RegistryError(format!("curl header: {e}")))?;
    }
    easy.http_headers(hdrs).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;

    let mut body_cursor = std::io::Cursor::new(body);
    {
        let mut transfer = easy.transfer();
        transfer.read_function(|buf| {
            use std::io::Read;
            Ok(body_cursor.read(buf).unwrap_or(0))
        }).map_err(|e| FreightError::RegistryError(format!("curl read: {e}")))?;
        transfer.write_function(|data| {
            resp.extend_from_slice(data);
            Ok(data.len())
        }).map_err(|e| FreightError::RegistryError(format!("curl write: {e}")))?;
        transfer.perform().map_err(|e| FreightError::RegistryError(format!("request: {e}")))?;
    }

    let code = easy.response_code()
        .map_err(|e| FreightError::RegistryError(format!("curl code: {e}")))?;
    let body_str = String::from_utf8_lossy(&resp).into_owned();
    match code {
        200 | 201 => Ok(body_str),
        401 => Err(FreightError::RegistryError("authentication required — check your token".into())),
        403 => Err(FreightError::RegistryError("permission denied".into())),
        409 => Err(FreightError::RegistryError(format!("conflict: {body_str}"))),
        _   => Err(FreightError::RegistryError(format!("HTTP {code}: {body_str}"))),
    }
}

fn http_post(url: &str, token: Option<&str>, content_type: &str, body: Vec<u8>) -> Result<String, FreightError> {
    let mut resp = Vec::new();
    let mut easy = Easy::new();

    easy.url(url).map_err(|e| FreightError::RegistryError(format!("curl url: {e}")))?;
    easy.post(true).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.post_field_size(body.len() as u64).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.fail_on_error(false).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.useragent(&format!("freight/{}", env!("CARGO_PKG_VERSION")))
        .map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;

    let mut hdrs = List::new();
    hdrs.append(&format!("Content-Type: {content_type}"))
        .map_err(|e| FreightError::RegistryError(format!("curl header: {e}")))?;
    if let Some(tok) = token {
        hdrs.append(&format!("Authorization: Bearer {tok}"))
            .map_err(|e| FreightError::RegistryError(format!("curl header: {e}")))?;
    }
    easy.http_headers(hdrs).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;

    let mut body_cursor = std::io::Cursor::new(body);
    {
        let mut transfer = easy.transfer();
        transfer.read_function(|buf| {
            use std::io::Read;
            Ok(body_cursor.read(buf).unwrap_or(0))
        }).map_err(|e| FreightError::RegistryError(format!("curl read: {e}")))?;
        transfer.write_function(|data| {
            resp.extend_from_slice(data);
            Ok(data.len())
        }).map_err(|e| FreightError::RegistryError(format!("curl write: {e}")))?;
        transfer.perform().map_err(|e| FreightError::RegistryError(format!("request: {e}")))?;
    }

    let code = easy.response_code()
        .map_err(|e| FreightError::RegistryError(format!("curl code: {e}")))?;
    let body_str = String::from_utf8_lossy(&resp).into_owned();
    match code {
        200 | 201 => Ok(body_str),
        401 => Err(FreightError::RegistryError("authentication required — check your token".into())),
        403 => Err(FreightError::RegistryError("permission denied".into())),
        409 => Err(FreightError::RegistryError(format!("conflict: {body_str}"))),
        _   => Err(FreightError::RegistryError(format!("HTTP {code}: {body_str}"))),
    }
}

fn http_delete(url: &str, token: &str) -> Result<String, FreightError> {
    let mut resp = Vec::new();
    let mut easy = Easy::new();

    easy.url(url).map_err(|e| FreightError::RegistryError(format!("curl url: {e}")))?;
    easy.custom_request("DELETE").map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.fail_on_error(false).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;
    easy.useragent(&format!("freight/{}", env!("CARGO_PKG_VERSION")))
        .map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;

    let mut hdrs = List::new();
    hdrs.append(&format!("Authorization: Bearer {token}"))
        .map_err(|e| FreightError::RegistryError(format!("curl header: {e}")))?;
    easy.http_headers(hdrs).map_err(|e| FreightError::RegistryError(format!("curl opt: {e}")))?;

    {
        let mut transfer = easy.transfer();
        transfer.write_function(|data| {
            resp.extend_from_slice(data);
            Ok(data.len())
        }).map_err(|e| FreightError::RegistryError(format!("curl write: {e}")))?;
        transfer.perform().map_err(|e| FreightError::RegistryError(format!("request: {e}")))?;
    }

    let code = easy.response_code()
        .map_err(|e| FreightError::RegistryError(format!("curl code: {e}")))?;
    let body_str = String::from_utf8_lossy(&resp).into_owned();
    match code {
        200 | 204 => Ok(body_str),
        401 => Err(FreightError::RegistryError("authentication required — check your token".into())),
        403 => Err(FreightError::RegistryError("permission denied".into())),
        404 => Err(FreightError::RegistryNotFound(url.to_string())),
        _   => Err(FreightError::RegistryError(format!("HTTP {code}: {body_str}"))),
    }
}

/// SHA-256 hex digest of a string — used to pre-hash passwords before transmission.
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn url_encode(s: &str) -> String {
    s.chars().flat_map(|c| match c {
        'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => {
            vec![c]
        }
        ' ' => vec!['+'],
        c => format!("%{:02X}", c as u32).chars().collect::<Vec<_>>(),
    }).collect()
}
