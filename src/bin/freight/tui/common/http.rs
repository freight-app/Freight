//! Shared async HTTP helpers for freight TUI forms.
//!
//! Keeps SHA-256 pre-hashing and the login/register request shapes in one
//! place so that login.rs, register.rs, and the admin-panel client all use
//! identical wire formats.
use anyhow::Result;
use sha2::{Digest, Sha256};

/// Compute the SHA-256 hex digest of `s`.
///
/// The freight registry stores `Argon2id(SHA-256(plaintext))`, so every client
/// must apply this layer before sending passwords over the wire.
pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// POST `/api/v1/users/login` and return the access token on success.
pub async fn post_login(url: &str, username: &str, password: &str) -> Result<String> {
    let resp = reqwest::Client::new()
        .post(format!("{url}/api/v1/users/login"))
        .json(&serde_json::json!({
            "username": username,
            "password": sha256_hex(password),
        }))
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if let Some(t) = body["token"].as_str() {
        Ok(t.to_string())
    } else {
        let detail = body["errors"][0]["detail"].as_str().unwrap_or("login failed");
        anyhow::bail!("{detail}")
    }
}

/// POST `/api/v1/users/register` and return the initial access token on success.
pub async fn post_register(
    url:        &str,
    username:   &str,
    password:   &str,
    email:      Option<&str>,
    token_name: Option<&str>,
) -> Result<String> {
    let resp = reqwest::Client::new()
        .post(format!("{url}/api/v1/users/register"))
        .json(&serde_json::json!({
            "username":   username,
            "password":   sha256_hex(password),
            "email":      email,
            "token_name": token_name,
        }))
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if let Some(t) = body["token"].as_str() {
        Ok(t.to_string())
    } else {
        let detail = body["errors"][0]["detail"].as_str().unwrap_or("registration failed");
        anyhow::bail!("{detail}")
    }
}
