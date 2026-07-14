//! Minimal compatibility layer for Codex CLI file authentication.
//!
//! Synced against openai/codex commit 996aa23e4ce900468047ed3ec57d1e7271f8d6de.

use base64::Engine;
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

pub const CODEX_UPSTREAM_REV: &str = "996aa23e4ce900468047ed3ec57d1e7271f8d6de";
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_BUILD_VERSION: &str = "0.0.0";
const REFRESH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    pub access_token: String,
    pub account_id: Option<String>,
    pub is_fedramp: bool,
}

#[derive(Debug)]
pub struct AuthManager {
    auth_file: PathBuf,
    client: reqwest::Client,
    refresh_lock: Mutex<()>,
}

impl AuthManager {
    pub fn new(codex_home: PathBuf, client: reqwest::Client) -> Self {
        Self {
            auth_file: codex_home.join("auth.json"),
            client,
            refresh_lock: Mutex::new(()),
        }
    }

    pub async fn credentials(&self) -> std::io::Result<Option<Credentials>> {
        if access_token_expired(&self.auth_file)? {
            self.refresh_token().await?;
        }
        load_credentials(&self.auth_file)
    }

    /// Matches Codex's guarded refresh: serialize refreshes, reload from disk,
    /// then persist only fields returned by the token authority.
    pub async fn refresh_token(&self) -> std::io::Result<()> {
        let attempted_access_token = load_document(&self.auth_file)?
            .and_then(|document| token_string(&document, "access_token"));
        let _guard = self.refresh_lock.lock().await;
        let mut document = load_document(&self.auth_file)?
            .ok_or_else(|| std::io::Error::other("Token data is not available."))?;
        if attempted_access_token != token_string(&document, "access_token") {
            return Ok(());
        }
        let refresh_token = document
            .pointer("/tokens/refresh_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| std::io::Error::other("Refresh token is not available."))?
            .to_string();

        let endpoint = std::env::var("CODEX_REFRESH_TOKEN_URL_OVERRIDE")
            .unwrap_or_else(|_| REFRESH_TOKEN_URL.to_string());
        let response = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&RefreshRequest {
                client_id: CLIENT_ID,
                grant_type: "refresh_token",
                refresh_token,
            })
            .send()
            .await
            .map_err(std::io::Error::other)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(std::io::Error::other(format!(
                "Failed to refresh token: {status}: {body}"
            )));
        }
        let refreshed: RefreshResponse = response.json().await.map_err(std::io::Error::other)?;
        apply_refresh_response(&mut document, refreshed)?;
        save_document(&self.auth_file, &document)
    }
}

fn apply_refresh_response(document: &mut Value, refreshed: RefreshResponse) -> std::io::Result<()> {
    let tokens = document
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| std::io::Error::other("Token data is not available."))?;
    if let Some(value) = refreshed.id_token {
        tokens.insert("id_token".into(), Value::String(value));
    }
    if let Some(value) = refreshed.access_token {
        tokens.insert("access_token".into(), Value::String(value));
    }
    if let Some(value) = refreshed.refresh_token {
        tokens.insert("refresh_token".into(), Value::String(value));
    }
    document["last_refresh"] = Value::String(Utc::now().to_rfc3339());
    Ok(())
}

fn token_string(document: &Value, field: &str) -> Option<String> {
    document
        .pointer(&format!("/tokens/{field}"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn access_token_expired(path: &Path) -> std::io::Result<bool> {
    let Some(document) = load_document(path)? else {
        return Ok(false);
    };
    let Some(jwt) = document
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
    else {
        return Ok(false);
    };
    Ok(decode_jwt_payload(jwt)
        .ok()
        .and_then(|claims| {
            claims
                .get("exp")
                .and_then(Value::as_i64)
                .map(|exp| exp <= Utc::now().timestamp())
        })
        .unwrap_or(false))
}

#[derive(Serialize)]
struct RefreshRequest {
    client_id: &'static str,
    grant_type: &'static str,
    refresh_token: String,
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

fn load_document(path: &Path) -> std::io::Result<Option<Value>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(std::io::Error::other),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn load_credentials(path: &Path) -> std::io::Result<Option<Credentials>> {
    let Some(document) = load_document(path)? else {
        return Ok(None);
    };
    let Some(tokens) = document.get("tokens") else {
        return Ok(None);
    };
    // Codex requires last_refresh for ChatGPT token auth.
    if document.get("last_refresh").is_none() {
        return Ok(None);
    }
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("Access token is not available."))?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id_token = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("ID token is not available."))?;
    let claims = decode_jwt_payload(id_token).map_err(std::io::Error::other)?;
    let is_fedramp = claims
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_is_fedramp"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(Some(Credentials {
        access_token,
        account_id,
        is_fedramp,
    }))
}

pub fn decode_jwt_payload(jwt: &str) -> Result<Value, String> {
    let mut parts = jwt.split('.');
    let (_, payload, signature) = (parts.next(), parts.next(), parts.next());
    let payload = match (payload, signature) {
        (Some(payload), Some(signature)) if !payload.is_empty() && !signature.is_empty() => payload,
        _ => return Err("invalid ID token format".into()),
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|err| err.to_string())?;
    serde_json::from_slice(&bytes).map_err(|err| err.to_string())
}

fn save_document(path: &Path, document: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(document).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path)?;
    file.write_all(&bytes)?;
    file.flush()
}

pub fn build_reqwest_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest_client_builder().build()
}

pub fn build_reqwest_client_with_cookie_store() -> Result<reqwest::Client, reqwest::Error> {
    reqwest_client_builder().cookie_store(true).build()
}

fn reqwest_client_builder() -> reqwest::ClientBuilder {
    let headers = codex_default_headers();
    let user_agent = codex_user_agent();
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent)
        .default_headers(headers);
    if let Some(path) =
        std::env::var_os("CODEX_CA_CERTIFICATE").or_else(|| std::env::var_os("SSL_CERT_FILE"))
    {
        if let Ok(pem) = std::fs::read(path) {
            for certificate in reqwest::Certificate::from_pem_bundle(&pem).unwrap_or_default() {
                builder = builder.add_root_certificate(certificate);
            }
        }
    }
    builder
}

pub fn codex_default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers
}

pub fn codex_user_agent() -> String {
    let os = os_info::get();
    format!(
        "codex_cli_rs/{CODEX_BUILD_VERSION} ({} {}; {}) unknown",
        os.os_type(),
        os.version(),
        os.architecture().unwrap_or("unknown")
    )
}

pub fn codex_tui_user_agent(version: &str) -> String {
    let os = os_info::get();
    format!(
        "codex-tui/{version} ({} {}; {}) xterm (codex-tui; {version})",
        os.os_type(),
        os.version(),
        os.architecture().unwrap_or("unknown")
    )
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
