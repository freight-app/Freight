//! HTTP client for the freight.dev package registry (and any compatible registry).
//!
//! API contract (all responses are JSON):
//!   GET /api/v1/packages/{name}         → ApiPackage (or 404)
//!   GET /api/v1/search?q={query}        → ApiSearchResult
//!
//! The default registry is `https://freight.dev`. Additional registries are
//! configured via `[[registry]]` entries in the config file.

use curl::easy::Easy;
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

    fn lookup(&self, name: &str) -> Result<Option<PackageInfo>, FreightError> {
        let url = format!("{}/api/v1/packages/{}", self.base_url, name);
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

fn url_encode(s: &str) -> String {
    s.chars().flat_map(|c| match c {
        'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => {
            vec![c]
        }
        ' ' => vec!['+'],
        c => format!("%{:02X}", c as u32).chars().collect::<Vec<_>>(),
    }).collect()
}
