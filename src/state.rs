use codex_auth_compat::{
    build_reqwest_client, build_reqwest_client_with_cookie_store, AuthManager, Credentials,
};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

struct BackendClient {
    account_identity: Option<String>,
    client: reqwest::Client,
}

pub struct AppState {
    pub auth_manager: Arc<AuthManager>,
    backend_client: Mutex<BackendClient>,
    /// Backend base URL; `/responses` and `/models` are appended at call time.
    pub backend_base_url: String,
    /// Codex CLI protocol version sent to the backend models endpoint.
    pub models_client_version: String,
    /// Stable identity shared by all requests from this proxy installation.
    pub installation_id: Uuid,
    /// Process-local mappings keep arbitrary client session IDs from reaching
    /// the Codex backend while preserving session continuity.
    session_ids: Mutex<HashMap<String, Uuid>>,
    /// Turn IDs use a separate namespace from session IDs.
    turn_ids: Mutex<HashMap<String, Uuid>>,
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
        let auth_client = build_reqwest_client()?;
        let backend_client = BackendClient {
            account_identity: None,
            client: build_reqwest_client_with_cookie_store()?,
        };
        let auth_manager = Arc::new(AuthManager::new(codex_home, auth_client));
        Ok(Self {
            auth_manager,
            backend_client: Mutex::new(backend_client),
            backend_base_url,
            models_client_version,
            installation_id,
            session_ids: Mutex::new(HashMap::new()),
            turn_ids: Mutex::new(HashMap::new()),
            api_key,
        })
    }

    pub fn backend_client_for(&self, credentials: &Credentials) -> anyhow::Result<reqwest::Client> {
        let identity = credentials.account_identity();
        let mut backend = self
            .backend_client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if backend.account_identity.as_deref() != Some(identity) {
            backend.client = build_reqwest_client_with_cookie_store()?;
            backend.account_identity = Some(identity.to_string());
            self.session_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            self.turn_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        Ok(backend.client.clone())
    }

    pub fn resolve_session_id(&self, client_session_id: Option<String>) -> Uuid {
        resolve_client_id(&self.session_ids, client_session_id)
    }

    pub fn resolve_turn_id(&self, client_turn_id: Option<String>) -> Uuid {
        resolve_client_id(&self.turn_ids, client_turn_id)
    }
}

fn resolve_client_id(ids: &Mutex<HashMap<String, Uuid>>, client_id: Option<String>) -> Uuid {
    let Some(client_id) = client_id else {
        return Uuid::now_v7();
    };

    let mut ids = ids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *ids.entry(client_id).or_insert_with(Uuid::now_v7)
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
