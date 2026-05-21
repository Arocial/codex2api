use codex_login::{AuthCredentialsStoreMode, AuthManager, default_client::build_reqwest_client};
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppState {
    pub auth_manager: Arc<AuthManager>,
    /// Pre-configured reqwest client with Codex User-Agent and originator header.
    pub http_client: reqwest::Client,
    /// Backend base URL; `/responses` and `/models` are appended at call time.
    pub backend_base_url: String,
    /// Clients must present `Authorization: Bearer <api_key>` on protected
    /// routes. Generated at startup if not provided via env/CLI.
    pub api_key: String,
}

impl AppState {
    pub fn new(codex_home: PathBuf, backend_base_url: String, api_key: String) -> Self {
        let auth_manager = Arc::new(AuthManager::new(
            codex_home,
            /*enable_codex_api_key_env*/ false,
            AuthCredentialsStoreMode::File,
        ));
        Self {
            auth_manager,
            http_client: build_reqwest_client(),
            backend_base_url,
            api_key,
        }
    }
}
