mod proxy;
mod state;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use state::AppState;

#[derive(Parser)]
#[command(
    name = "codex2api",
    about = "Proxy Codex subscription as a standard OpenAI Responses API"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Local address to listen on.
    #[arg(long, default_value = "127.0.0.1:3402")]
    listen: SocketAddr,

    /// Codex home directory (default: ~/.codex).
    #[arg(long)]
    codex_home: Option<PathBuf>,

    /// Backend base URL. `/responses` and `/models` are appended to this.
    /// Override for FedRAMP, enterprise, or staging endpoints.
    #[arg(long, env = "CODEX2API_BACKEND_BASE_URL", default_value = proxy::DEFAULT_BACKEND_BASE_URL)]
    backend_base_url: String,

    /// API key required from clients in the `Authorization: Bearer ...` header
    /// on `/v1/*` routes. If unset, a random key is generated at startup and
    /// printed to the log.
    #[arg(long, env = "CODEX2API_API_KEY")]
    api_key: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Log in by delegating to the installed Codex CLI.
    Login,
}

/// 32-char alphanumeric suffix, ~190 bits of entropy. Prefixed `sk-` to match
/// the convention OpenAI-compatible clients expect.
fn generate_api_key() -> String {
    use rand::distr::Alphanumeric;
    use rand::Rng;
    let suffix: String = rand::rng()
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();
    format!("sk-{suffix}")
}

fn default_codex_home() -> PathBuf {
    std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").expect("HOME env var not set");
            PathBuf::from(home).join(".codex")
        })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let codex_home = cli.codex_home.unwrap_or_else(default_codex_home);

    match cli.command {
        Some(Command::Login) => run_login(codex_home).await?,
        None => {
            // Treat an empty env var the same as unset.
            let api_key = cli.api_key.filter(|s| !s.is_empty());
            run_server(codex_home, cli.listen, cli.backend_base_url, api_key).await?
        }
    }

    Ok(())
}

async fn run_login(codex_home: PathBuf) -> anyhow::Result<()> {
    let codex = std::env::var("CODEX2API_CODEX_BIN").unwrap_or_else(|_| "codex".to_string());
    let status = tokio::process::Command::new(&codex)
        .arg("-c")
        .arg("cli_auth_credentials_store=\"file\"")
        .arg("login")
        .env("CODEX_HOME", codex_home)
        .status()
        .await
        .map_err(|err| anyhow::anyhow!("failed to run `{codex} login`: {err}"))?;
    anyhow::ensure!(status.success(), "`{codex} login` exited with {status}");
    Ok(())
}

async fn run_server(
    codex_home: PathBuf,
    listen: SocketAddr,
    backend_base_url: String,
    api_key: Option<String>,
) -> anyhow::Result<()> {
    // Trim trailing slashes so callers can pass either form.
    let base = backend_base_url.trim_end_matches('/').to_string();
    let api_key = match api_key {
        Some(k) => {
            tracing::info!("Client bearer auth enabled (CODEX2API_API_KEY set)");
            k
        }
        None => {
            let k = generate_api_key();
            // Log at WARN so it stands out — this key is ephemeral and must be
            // captured from logs to be reused across restarts.
            tracing::warn!(
                "CODEX2API_API_KEY not set — generated ephemeral key: {k} \
                 (set CODEX2API_API_KEY to make it stable)"
            );
            k
        }
    };
    let state = Arc::new(AppState::new(codex_home, base, api_key));

    // `route_layer` only applies to routes registered *before* it, so
    // `/healthz` (added after) stays publicly reachable.
    let app = Router::new()
        .route("/v1/responses", post(proxy::responses_handler))
        .route("/v1/models", get(proxy::models_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            proxy::require_bearer,
        ))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(proxy::MAX_REQUEST_BODY_SIZE))
        .with_state(state);

    tracing::info!("Listening on {listen}");
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to install Ctrl-C handler: {err}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::error!("failed to install SIGTERM handler: {err}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received, draining...");
}
