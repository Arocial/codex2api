use axum::body::Body;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::state::AppState;
use codex_auth_compat::codex_tui_user_agent;

/// Cap on short, non-streaming backend calls (e.g. /models). /responses is
/// streaming and may legitimately run for many minutes, so it gets no total
/// timeout — connect-time issues surface as reqwest errors regardless.
const SHORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Large enough for long prompts and tool definitions while still bounding
/// memory use, since request bodies are parsed and serialized in memory.
pub const MAX_REQUEST_BODY_SIZE: usize = 32 * 1024 * 1024;

pub const DEFAULT_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Codex protocol version advertised to the models endpoint. This must track a
/// Codex CLI version whose model schema this proxy can pass through.
pub const DEFAULT_MODELS_CLIENT_VERSION: &str = "0.142.5";

/// The Codex models endpoint requires a client version. Add it here so callers
/// do not need to know about the backend-specific query parameter.
fn models_url(backend_base_url: &str, client_version: &str) -> String {
    format!("{backend_base_url}/models?client_version={client_version}")
}

pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
    error_type: &'static str,
    code: &'static str,
}

impl ApiError {
    fn new(
        status: StatusCode,
        message: impl Into<String>,
        error_type: &'static str,
        code: &'static str,
    ) -> Self {
        Self {
            status,
            message: message.into(),
            error_type,
            code,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            axum::Json(json!({
                "error": {
                    "message": self.message,
                    "type": self.error_type,
                    "param": null,
                    "code": self.code,
                }
            })),
        )
            .into_response()
    }
}

enum Method {
    Get,
    Post,
}

#[derive(Debug)]
struct CodexRequestContext {
    installation_id: String,
    session_id: String,
    turn_id: String,
    window_id: String,
    turn_metadata: String,
}

impl CodexRequestContext {
    fn new(session_id: Uuid, turn_id: Uuid, installation_id: Uuid) -> Self {
        let session_id = session_id.to_string();
        let turn_id = turn_id.to_string();
        let window_id = format!("{session_id}:0");
        let turn_started_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let installation_id = installation_id.to_string();
        let turn_metadata = json!({
            "installation_id": installation_id,
            "session_id": session_id,
            "thread_id": session_id,
            "turn_id": turn_id,
            "window_id": window_id,
            "request_kind": "turn",
            "thread_source": "user",
            "sandbox": "none",
            "turn_started_at_unix_ms": turn_started_at_unix_ms,
        })
        .to_string();

        Self {
            installation_id,
            session_id,
            turn_id,
            window_id,
            turn_metadata,
        }
    }
}

fn requested_session_id(headers: &HeaderMap) -> Option<String> {
    ["x-session-id", "session-id"]
        .iter()
        .find_map(|name| requested_id(headers, name))
}

fn requested_turn_id(headers: &HeaderMap) -> Option<String> {
    requested_id(headers, "x-turn-id")
}

fn requested_id(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Inject required body defaults and Codex request metadata before forwarding.
///
/// - `store`: defaults to `false` if absent (backend rejects missing `store`); client's
///   explicit value is preserved.
/// - `stream`: always forced to `true` (this proxy only supports SSE passthrough).
/// - Codex-owned `client_metadata` keys are derived from one request context so
///   their header and body projections cannot diverge.
fn prepare_responses_body(body: Bytes, ctx: &CodexRequestContext) -> anyhow::Result<Bytes> {
    let mut json: serde_json::Value = serde_json::from_slice(&body)?;
    let obj = json
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("request body must be a JSON object"))?;
    obj.entry("tool_choice")
        .or_insert(serde_json::Value::String("auto".to_string()));
    obj.entry("parallel_tool_calls")
        .or_insert(serde_json::Value::Bool(false));
    obj.entry("store").or_insert(serde_json::Value::Bool(false));
    obj.insert("stream".to_string(), serde_json::Value::Bool(true));
    obj.entry("include").or_insert_with(|| {
        serde_json::Value::Array(vec![serde_json::Value::String(
            "reasoning.encrypted_content".to_string(),
        )])
    });
    obj.entry("prompt_cache_key")
        .or_insert_with(|| serde_json::Value::String(ctx.session_id.clone()));

    let metadata_value = obj.entry("client_metadata").or_insert_with(|| json!({}));
    if metadata_value.is_null() {
        *metadata_value = json!({});
    }
    let metadata = metadata_value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("client_metadata must be a JSON object or null"))?;
    for (key, value) in [
        (
            "x-codex-installation-id",
            serde_json::Value::String(ctx.installation_id.clone()),
        ),
        (
            "x-codex-window-id",
            serde_json::Value::String(ctx.window_id.clone()),
        ),
        (
            "thread_id",
            serde_json::Value::String(ctx.session_id.clone()),
        ),
        (
            "session_id",
            serde_json::Value::String(ctx.session_id.clone()),
        ),
        ("turn_id", serde_json::Value::String(ctx.turn_id.clone())),
        (
            "x-codex-turn-metadata",
            serde_json::Value::String(ctx.turn_metadata.clone()),
        ),
    ] {
        metadata.insert(key.to_string(), value);
    }
    Ok(Bytes::from(serde_json::to_vec(&json)?))
}

async fn synchronize_backend_account(state: &AppState) -> anyhow::Result<()> {
    let credentials = state
        .auth_manager
        .credentials()
        .await
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| anyhow::anyhow!("not authenticated — run `codex login` first"))?;
    state.backend_client_for(&credentials)?;
    Ok(())
}

/// Build and send an authenticated request to the backend.
async fn do_request(
    state: &AppState,
    method: &Method,
    url: &str,
    body: Option<&Bytes>,
    codex_context: Option<&CodexRequestContext>,
) -> anyhow::Result<reqwest::Response> {
    let auth = state
        .auth_manager
        .credentials()
        .await
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| anyhow::anyhow!("not authenticated — run `codex login` first"))?;

    let http_client = state.backend_client_for(&auth)?;
    let access_token = auth.access_token;
    let account_id = auth.account_id;
    let is_fedramp = auth.is_fedramp;

    let mut req = match method {
        Method::Get => http_client.get(url).timeout(SHORT_REQUEST_TIMEOUT),
        Method::Post => http_client.post(url),
    };

    req = req.header("Authorization", format!("Bearer {access_token}"));
    req = req.header("Content-Type", "application/json");
    if let Some(ctx) = codex_context {
        req = apply_codex_request_headers(req, state, ctx);
    }

    if let Some(id) = account_id {
        req = req.header("ChatGPT-Account-ID", id);
    }
    if is_fedramp {
        req = req.header("X-OpenAI-Fedramp", "true");
    }
    if let Some(b) = body {
        // Bytes clone is an Arc bump, not a buffer copy.
        req = req.body(b.clone());
    }

    Ok(req.send().await?)
}

fn apply_codex_request_headers(
    req: reqwest::RequestBuilder,
    state: &AppState,
    ctx: &CodexRequestContext,
) -> reqwest::RequestBuilder {
    let version = &state.models_client_version;
    let user_agent = codex_tui_user_agent(version);
    req.header("Accept", "text/event-stream")
        .header("version", version)
        .header("originator", "codex-tui")
        .header("User-Agent", user_agent)
        .header("session-id", &ctx.session_id)
        .header("thread-id", &ctx.session_id)
        .header("x-client-request-id", &ctx.session_id)
        .header("x-codex-installation-id", &ctx.installation_id)
        .header("x-codex-window-id", &ctx.window_id)
        .header("x-codex-turn-metadata", &ctx.turn_metadata)
}

/// Send an authenticated request, retrying once after a token refresh on 401.
async fn do_request_with_retry(
    state: &AppState,
    method: Method,
    url: &str,
    body: Option<Bytes>,
    codex_context: Option<&CodexRequestContext>,
) -> anyhow::Result<reqwest::Response> {
    let resp = do_request(state, &method, url, body.as_ref(), codex_context).await?;
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(resp);
    }

    tracing::info!("Received 401, attempting token refresh");
    if let Err(err) = state.auth_manager.refresh_token().await {
        tracing::error!("Token refresh failed: {err}");
        // Refresh failed: return the original 401 to the client rather than
        // making a doomed second request.
        return Ok(resp);
    }

    do_request(state, &method, url, body.as_ref(), codex_context).await
}

/// Hop-by-hop headers (RFC 7230 §6.1) plus content-length, which hyper recomputes
/// for the outgoing stream. Forwarding the upstream value risks mismatched framing.
fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

fn should_forward_response_header(name: &str) -> bool {
    !is_hop_by_hop(name) && !name.eq_ignore_ascii_case("set-cookie")
}

/// Stream a backend response back to the client. Upstream status and body —
/// including error responses — are forwarded verbatim so clients see real
/// backend error messages instead of an opaque proxy status. Cookies remain in
/// the proxy's shared cookie store and are not exposed to downstream clients.
fn stream_response(resp: reqwest::Response) -> Result<Response<Body>, ApiError> {
    let status = resp.status();
    let mut builder = Response::builder().status(status.as_u16());

    for (name, value) in resp.headers() {
        if !should_forward_response_header(name.as_str()) {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }

    let stream = resp.bytes_stream();
    builder.body(Body::from_stream(stream)).map_err(|err| {
        tracing::error!("Failed to build backend response: {err}");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The proxy failed to construct the backend response.",
            "proxy_error",
            "response_build_failed",
        )
    })
}

/// Middleware that requires `Authorization: Bearer <api_key>` on protected
/// routes.
pub async fn require_bearer(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    match provided {
        Some(p) if p == state.api_key => Ok(next.run(req).await),
        _ => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Missing or invalid API key.",
            "authentication_error",
            "invalid_api_key",
        )),
    }
}

/// POST /v1/responses — proxy to the Codex backend responses endpoint.
pub async fn responses_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response<Body>, ApiError> {
    let body = body.map_err(|err| {
        let status = err.status();
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                status,
                format!(
                    "Request body exceeds the {} MiB limit.",
                    MAX_REQUEST_BODY_SIZE / 1024 / 1024
                ),
                "invalid_request_error",
                "request_too_large",
            )
        } else {
            ApiError::new(
                status,
                "Failed to read the request body.",
                "invalid_request_error",
                "invalid_request_body",
            )
        }
    })?;

    synchronize_backend_account(&state).await.map_err(|err| {
        tracing::error!("Failed to prepare backend account state: {err}");
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "The proxy could not prepare the Codex backend account.",
            "proxy_error",
            "backend_unavailable",
        )
    })?;
    let session_id = state.resolve_session_id(requested_session_id(&headers));
    let turn_id = state.resolve_turn_id(requested_turn_id(&headers));
    let context = CodexRequestContext::new(session_id, turn_id, state.installation_id);
    let prepared = prepare_responses_body(body, &context).map_err(|err| {
        tracing::error!("Failed to parse request body: {err}");
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Invalid JSON request body: {err}"),
            "invalid_request_error",
            "invalid_json",
        )
    })?;

    let url = format!("{}/responses", state.backend_base_url);
    let resp = do_request_with_retry(&state, Method::Post, &url, Some(prepared), Some(&context))
        .await
        .map_err(|err| {
            tracing::error!("Backend request failed: {err}");
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "The proxy could not reach the Codex backend.",
                "proxy_error",
                "backend_unavailable",
            )
        })?;

    let mut response = stream_response(resp)?;
    if let Ok(value) = context.session_id.parse() {
        response.headers_mut().insert("x-session-id", value);
    }
    Ok(response)
}

/// GET /v1/models — proxy to the Codex backend models endpoint.
pub async fn models_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Response<Body>, ApiError> {
    let url = models_url(&state.backend_base_url, &state.models_client_version);
    let resp = do_request_with_retry(&state, Method::Get, &url, None, None)
        .await
        .map_err(|err| {
            tracing::error!("Models request failed: {err}");
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "The proxy could not reach the Codex backend.",
                "proxy_error",
                "backend_unavailable",
            )
        })?;

    stream_response(resp)
}

#[cfg(test)]
#[path = "../tests/unit/proxy.rs"]
mod tests;
