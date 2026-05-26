use anyhow::{bail, Result};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use super::super::common::sha256_hex;

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PackageSummary {
    pub name:        String,
    pub description: Option<String>,
    pub latest:      Option<String>,
    #[serde(default)]
    pub downloads:   i64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct VersionInfo {
    pub version:      String,
    pub yanked:       bool,
    pub downloads:    i64,
    pub download_url: String,
    #[serde(default)]
    pub prebuilt_triples: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackageDetail {
    pub name:        String,
    pub description: Option<String>,
    pub versions:    Vec<VersionInfo>,
    pub owners:      Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct OwnerEntry {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserInfo {
    pub id:       i64,
    pub username: String,
    pub email:    Option<String>,
    pub is_admin: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TokenInfo {
    pub id:         i64,
    pub name:       String,
    pub kind:       String,
    pub scope:      String,
    pub expires_at: Option<i64>,
    pub last_used:  Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AuditEntry {
    pub id:         i64,
    pub username:   Option<String>,
    pub action:     String,
    pub package:    Option<String>,
    pub version:    Option<String>,
    pub ip_addr:    Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct LoginResp {
    pub token:         String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct OrgSummary {
    pub id:          i64,
    pub name:        String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrgMember {
    pub username: String,
    pub role:     String,
}

// ── Client ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Client {
    inner:    reqwest::Client,
    base_url: String,
    token:    Option<String>,
}

impl Client {
    pub fn new(base_url: String, token: Option<String>) -> Self {
        Self {
            inner: reqwest::Client::new(),
            base_url,
            token,
        }
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn encode_pkg(name: &str) -> String {
        name.replace('/', "%2F")
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(t) = &self.token {
            req.bearer_auth(t)
        } else {
            req
        }
    }

    async fn check(resp: reqwest::Response) -> Result<serde_json::Value> {
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            let detail = body["errors"][0]["detail"]
                .as_str()
                .unwrap_or("request failed")
                .to_string();
            bail!("{} — {}", status, detail);
        }
        Ok(body)
    }

    // ── Auth ──────────────────────────────────────────────────────────────────

    pub async fn login(&self, username: &str, password: &str) -> Result<LoginResp> {
        let resp = self
            .inner
            .post(self.url("/api/v1/users/login"))
            .json(&json!({ "username": username, "password": sha256_hex(password) }))
            .send()
            .await?;
        let body = Self::check(resp).await?;
        Ok(serde_json::from_value(body)?)
    }

    pub async fn me(&self) -> Result<(String, bool)> {
        let resp = self.auth(self.inner.get(self.url("/api/v1/me"))).send().await?;
        let body = Self::check(resp).await?;
        let login    = body["login"].as_str().unwrap_or("").to_string();
        let is_admin = body["is_admin"].as_bool().unwrap_or(false);
        Ok((login, is_admin))
    }

    // ── Packages ──────────────────────────────────────────────────────────────

    pub async fn search(&self, q: &str) -> Result<Vec<PackageSummary>> {
        let resp = self
            .inner
            .get(self.url("/api/v1/search"))
            .query(&[("q", q), ("limit", "100")])
            .send()
            .await?;
        let body = Self::check(resp).await?;
        let list: Vec<PackageSummary> =
            serde_json::from_value(body["packages"].clone()).unwrap_or_default();
        Ok(list)
    }

    pub async fn get_package(&self, name: &str) -> Result<PackageDetail> {
        let enc        = Self::encode_pkg(name);
        let url_pkg    = self.url(&format!("/api/v1/packages/{enc}"));
        let url_owners = self.url(&format!("/api/v1/packages/{enc}/owners"));

        let (pkg_resp, own_resp) = tokio::join!(
            self.inner.get(&url_pkg).send(),
            self.inner.get(&url_owners).send(),
        );

        let pkg_body = Self::check(pkg_resp?).await?;
        let own_body = match own_resp {
            Ok(r)  => r.json::<serde_json::Value>().await.unwrap_or_default(),
            Err(_) => serde_json::Value::default(),
        };

        let versions: Vec<VersionInfo> =
            serde_json::from_value(pkg_body["versions"].clone()).unwrap_or_default();
        let owners: Vec<String> = own_body["users"]
            .as_array()
            .map(|arr| arr.iter()
                .filter_map(|v| v["login"].as_str().map(str::to_string))
                .collect())
            .unwrap_or_default();

        Ok(PackageDetail {
            name:        pkg_body["name"].as_str().unwrap_or(name).to_string(),
            description: pkg_body["description"].as_str().map(str::to_string),
            versions,
            owners,
        })
    }

    pub async fn yank(&self, name: &str, version: &str) -> Result<()> {
        let enc = Self::encode_pkg(name);
        let resp = self
            .auth(self.inner.delete(
                self.url(&format!("/api/v1/packages/{enc}/{version}/yank")),
            ))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn unyank(&self, name: &str, version: &str) -> Result<()> {
        let enc = Self::encode_pkg(name);
        let resp = self
            .auth(self.inner.put(
                self.url(&format!("/api/v1/packages/{enc}/{version}/yank")),
            ))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn delete_package(&self, name: &str) -> Result<()> {
        let enc = Self::encode_pkg(name);
        let resp = self
            .auth(self.inner.delete(
                self.url(&format!("/api/v1/admin/packages/{enc}")),
            ))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn publish(&self, name: &str, vers: &str, tarball: Vec<u8>) -> Result<()> {
        let meta       = serde_json::json!({"name": name, "vers": vers}).to_string();
        let meta_bytes = meta.as_bytes();
        let mut body   = Vec::new();
        body.extend_from_slice(&(meta_bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(meta_bytes);
        body.extend_from_slice(&(tarball.len() as u32).to_le_bytes());
        body.extend_from_slice(&tarball);

        let resp = self
            .auth(self.inner.put(self.url("/api/v1/packages")))
            .body(body)
            .send()
            .await?;

        if resp.status() == StatusCode::PAYLOAD_TOO_LARGE {
            bail!("tarball exceeds server upload limit");
        }
        Self::check(resp).await?;
        Ok(())
    }

    // ── Owners ────────────────────────────────────────────────────────────────

    pub async fn add_owner(&self, pkg: &str, username: &str) -> Result<()> {
        let enc = Self::encode_pkg(pkg);
        let resp = self
            .auth(self.inner.put(self.url(&format!("/api/v1/packages/{enc}/owners"))))
            .json(&json!({ "users": [username] }))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn remove_owner(&self, pkg: &str, username: &str) -> Result<()> {
        let enc = Self::encode_pkg(pkg);
        let resp = self
            .auth(self.inner.delete(self.url(&format!("/api/v1/packages/{enc}/owners"))))
            .json(&json!({ "users": [username] }))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    // ── Tokens ────────────────────────────────────────────────────────────────

    pub async fn list_tokens(&self) -> Result<Vec<TokenInfo>> {
        let resp = self.auth(self.inner.get(self.url("/api/v1/me/tokens"))).send().await?;
        let body = Self::check(resp).await?;
        Ok(serde_json::from_value(body["tokens"].clone()).unwrap_or_default())
    }

    pub async fn create_token(&self, name: &str, expires_days: Option<i64>, scope: &str) -> Result<String> {
        let resp = self
            .auth(self.inner.post(self.url("/api/v1/me/tokens")))
            .json(&json!({ "name": name, "expires_days": expires_days, "scope": scope }))
            .send()
            .await?;
        let body = Self::check(resp).await?;
        Ok(body["token"].as_str().unwrap_or("").to_string())
    }

    pub async fn revoke_token(&self, name: &str) -> Result<()> {
        let resp = self
            .auth(self.inner.delete(self.url(&format!("/api/v1/me/tokens/{name}"))))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    // ── Orgs ──────────────────────────────────────────────────────────────────

    pub async fn list_orgs(&self) -> Result<Vec<OrgSummary>> {
        let resp = self.inner.get(self.url("/api/v1/orgs")).send().await?;
        let body = Self::check(resp).await?;
        Ok(serde_json::from_value(body["orgs"].clone()).unwrap_or_default())
    }

    pub async fn get_org(&self, name: &str) -> Result<(OrgSummary, Vec<OrgMember>)> {
        let resp = self.inner.get(self.url(&format!("/api/v1/orgs/{name}"))).send().await?;
        let body = Self::check(resp).await?;
        let org = OrgSummary {
            id:          body["id"].as_i64().unwrap_or(0),
            name:        body["name"].as_str().unwrap_or(name).to_string(),
            description: body["description"].as_str().map(str::to_string),
        };
        let members: Vec<OrgMember> =
            serde_json::from_value(body["members"].clone()).unwrap_or_default();
        Ok((org, members))
    }

    pub async fn create_org(&self, name: &str, description: &str) -> Result<()> {
        let desc = if description.is_empty() { None } else { Some(description) };
        let resp = self
            .auth(self.inner.post(self.url("/api/v1/orgs")))
            .json(&json!({ "name": name, "description": desc }))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn delete_org(&self, name: &str) -> Result<()> {
        let resp = self
            .auth(self.inner.delete(self.url(&format!("/api/v1/orgs/{name}"))))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn add_org_member(&self, org: &str, username: &str, role: &str) -> Result<()> {
        let resp = self
            .auth(self.inner.put(self.url(&format!("/api/v1/orgs/{org}/members"))))
            .json(&json!({ "username": username, "role": role }))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn remove_org_member(&self, org: &str, username: &str) -> Result<()> {
        let resp = self
            .auth(self.inner.delete(self.url(&format!("/api/v1/orgs/{org}/members/{username}"))))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    // ── Users (admin) ─────────────────────────────────────────────────────────

    pub async fn list_users(&self) -> Result<Vec<UserInfo>> {
        let resp = self.auth(self.inner.get(self.url("/api/v1/admin/users"))).send().await?;
        let body = Self::check(resp).await?;
        Ok(serde_json::from_value(body["users"].clone()).unwrap_or_default())
    }

    pub async fn promote_user(&self, username: &str) -> Result<()> {
        let resp = self
            .auth(self.inner.post(self.url(&format!("/api/v1/admin/users/{username}/promote"))))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn demote_user(&self, username: &str) -> Result<()> {
        let resp = self
            .auth(self.inner.post(self.url(&format!("/api/v1/admin/users/{username}/demote"))))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    pub async fn remove_user(&self, username: &str) -> Result<()> {
        let resp = self
            .auth(self.inner.delete(self.url(&format!("/api/v1/admin/users/{username}"))))
            .send()
            .await?;
        Self::check(resp).await?;
        Ok(())
    }

    // ── Audit (admin) ─────────────────────────────────────────────────────────

    pub async fn list_audit(&self, filter: &str) -> Result<Vec<AuditEntry>> {
        let mut req = self.auth(self.inner.get(self.url("/api/v1/audit")));
        if !filter.is_empty() {
            if let Some(uname) = filter.strip_prefix("user:") {
                req = req.query(&[("user", uname)]);
            } else {
                req = req.query(&[("action", filter)]);
            }
        }
        let resp = req.query(&[("limit", "200")]).send().await?;
        let body = Self::check(resp).await?;
        Ok(serde_json::from_value(body["entries"].clone()).unwrap_or_default())
    }
}
