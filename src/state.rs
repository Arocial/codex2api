use codex_auth_compat::{build_reqwest_client, AuthManager};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

pub struct AppState {
    pub auth_manager: Arc<AuthManager>,
    /// Pre-configured reqwest client with Codex User-Agent and originator header.
    pub http_client: reqwest::Client,
    /// Backend base URL; `/responses` and `/models` are appended at call time.
    pub backend_base_url: String,
    /// Codex CLI protocol version sent to the backend models endpoint.
    pub models_client_version: String,
    /// Stable identity shared by all requests from this proxy installation.
    pub installation_id: Uuid,
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
    ) -> anyhow::Result<Self> {
        let installation_id = load_or_create_installation_id(&codex_home)?;
        let http_client =
            build_reqwest_client().expect("failed to build Codex-compatible HTTP client");
        let auth_manager = Arc::new(AuthManager::new(codex_home, http_client.clone()));
        Ok(Self {
            auth_manager,
            http_client,
            backend_base_url,
            models_client_version,
            installation_id,
            api_key,
        })
    }
}

fn load_or_create_installation_id(codex_home: &PathBuf) -> anyhow::Result<Uuid> {
    let path = codex_home.join("installation_id");
    match std::fs::read_to_string(&path) {
        Ok(value) => {
            return Uuid::parse_str(value.trim()).map_err(|err| {
                anyhow::anyhow!("invalid installation ID in {}: {err}", path.display())
            });
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    std::fs::create_dir_all(codex_home)?;
    let id = Uuid::new_v4();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            writeln!(file, "{id}")?;
            file.flush()?;
            Ok(id)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let value = std::fs::read_to_string(&path)?;
            Uuid::parse_str(value.trim()).map_err(|parse_err| {
                anyhow::anyhow!("invalid installation ID in {}: {parse_err}", path.display())
            })
        }
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
#[path = "../tests/unit/state.rs"]
mod tests;
