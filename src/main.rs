mod proxy;
mod state;

use axum::Router;
use axum::routing::{get, post};
use clap::{Parser, Subcommand};
use codex_login::{AuthCredentialsStoreMode, CLIENT_ID, ServerOptions, run_login_server};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use state::AppState;

#[derive(Parser)]
#[command(name = "codex2api", about = "Proxy Codex subscription as a standard OpenAI Responses API")]
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
}

#[derive(Subcommand)]
enum Command {
    /// Log in to ChatGPT / OpenAI using the browser-based PKCE flow.
    Login,
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
        None => run_server(codex_home, cli.listen, cli.backend_base_url).await?,
    }

    Ok(())
}

async fn run_login(codex_home: PathBuf) -> anyhow::Result<()> {
    let opts = ServerOptions::new(
        codex_home,
        CLIENT_ID.to_string(),
        /*forced_chatgpt_workspace_id*/ None,
        AuthCredentialsStoreMode::File,
    );
    let server = run_login_server(opts)?;
    println!("Opening browser: {}", server.auth_url);
    server.block_until_done().await?;
    println!("Login successful.");
    Ok(())
}

async fn run_server(
    codex_home: PathBuf,
    listen: SocketAddr,
    backend_base_url: String,
) -> anyhow::Result<()> {
    // Trim trailing slashes so callers can pass either form.
    let base = backend_base_url.trim_end_matches('/').to_string();
    let state = Arc::new(AppState::new(codex_home, base));

    let app = Router::new()
        .route("/v1/responses", post(proxy::responses_handler))
        .route("/v1/models", get(proxy::models_handler))
        .with_state(state);

    tracing::info!("Listening on {listen}");
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
