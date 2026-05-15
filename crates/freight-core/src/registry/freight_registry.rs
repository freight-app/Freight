//! HTTP client for the freight.dev package registry.
//!
//! API contract (all responses are JSON):
//!   GET /api/v1/packages/{name}         → ApiPackage (or 404)
//!   GET /api/v1/search?q={query}        → ApiSearchResult
//!
//! The base URL defaults to `https://freight.dev` and can be overridden via
//! the `FREIGHT_REGISTRY_URL` environment variable.

use curl::easy::Easy;
use serde::Deserialize;

use crate::error::FreightError;
use super::{DEFAULT_REGISTRY_URL, PackageInfo, PackageVersion, Registry};

// ── Public client ─────────────────────────────────────────────────────────────

pub struct FreightRegistry {
    base_url: String,
}

impl FreightRegistry {
    pub fn new() -> Self {
        let base_url = std::env::var("FREIGHT_REGISTRY_URL")
            .unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_string());
        Self { base_url: base_url.trim_end_matches('/').to_string() }
    }
}

impl Registry for FreightRegistry {
    fn lookup(&self, name: &str) -> Result<Option<PackageInfo>, FreightError> {
        let url = format!("{}/api/v1/packages/{}", self.base_url, name);
        match http_get_json::<ApiPackage>(&url) {
            Ok(pkg)                              => Ok(Some(pkg.into())),
            Err(FreightError::RegistryNotFound(_)) => Ok(None),
            Err(e)                               => Err(e),
        }
    }

    fn search(&self, query: &str) -> Result<Vec<PackageInfo>, FreightError> {
        let url = format!("{}/api/v1/search?q={}", self.base_url, url_encode(query));
        let result = http_get_json::<ApiSearchResult>(&url)?;
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

fn http_get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, FreightError> {
    let body = http_get(url)?;
    serde_json::from_str(&body).map_err(|e| {
        FreightError::RegistryError(format!("invalid JSON from registry: {e}"))
    })
}

fn http_get(url: &str) -> Result<String, FreightError> {
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
