use codex_auth_compat::{build_reqwest_client, AuthManager};
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppState {
    pub auth_manager: Arc<AuthManager>,
    /// Pre-configured reqwest client with Codex User-Agent and originator header.
    pub http_client: reqwest::Client,
    /// Backend base URL; `/responses` and `/models` are appended at call time.
    pub backend_base_url: String,
    /// Codex CLI protocol version sent to the backend models endpoint.
    pub models_client_version: String,
    /// Clients must present `Authorization: Bearer <api_key>` on protected
    /// routes. Generated at startup if not provided via env/CLI.
    pub api_key: String,
}

impl AppState {
    pub fn new(
        codex_home: PathBuf,
        backend_base_url: String,
        models_client_version: String,
        api_key: String,
    ) -> Self {
        let http_client =
            build_reqwest_client().expect("failed to build Codex-compatible HTTP client");
        let auth_manager = Arc::new(AuthManager::new(codex_home, http_client.clone()));
        Self {
            auth_manager,
            http_client,
            backend_base_url,
            models_client_version,
            api_key,
        }
    }
}
