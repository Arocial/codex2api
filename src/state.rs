use codex_login::{AuthCredentialsStoreMode, AuthManager, default_client::build_reqwest_client};
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppState {
    pub auth_manager: Arc<AuthManager>,
    /// Pre-configured reqwest client with Codex User-Agent and originator header.
    pub http_client: reqwest::Client,
}

impl AppState {
    pub fn new(codex_home: PathBuf) -> Self {
        let auth_manager = Arc::new(AuthManager::new(
            codex_home,
            /*enable_codex_api_key_env*/ false,
            AuthCredentialsStoreMode::File,
        ));
        Self {
            auth_manager,
            http_client: build_reqwest_client(),
        }
    }
}
