use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use bytes::Bytes;
use std::sync::Arc;

use crate::state::AppState;

pub const DEFAULT_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

enum Method {
    Get,
    Post,
}

/// Inject required body defaults before forwarding to the backend.
///
/// - `store`: defaults to `false` if absent (backend rejects missing `store`); client's
///   explicit value is preserved.
/// - `stream`: always forced to `true` (this proxy only supports SSE passthrough).
fn apply_body_defaults(body: Bytes) -> anyhow::Result<Bytes> {
    let mut json: serde_json::Value = serde_json::from_slice(&body)?;
    let obj = json
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("request body must be a JSON object"))?;
    obj.entry("store").or_insert(serde_json::Value::Bool(false));
    obj.insert("stream".to_string(), serde_json::Value::Bool(true));
    Ok(Bytes::from(serde_json::to_vec(&json)?))
}

/// Build and send an authenticated request to the backend.
async fn do_request(
    state: &AppState,
    method: &Method,
    url: &str,
    body: Option<&Bytes>,
) -> anyhow::Result<reqwest::Response> {
    let auth = state
        .auth_manager
        .auth()
        .await
        .ok_or_else(|| anyhow::anyhow!("not authenticated — run `codex login` first"))?;

    let access_token = auth.get_token()?;
    let account_id = auth.get_account_id();
    let is_fedramp = auth.is_fedramp_account();

    let mut req = match method {
        Method::Get => state.http_client.get(url),
        Method::Post => state.http_client.post(url),
    };

    req = req.header("Authorization", format!("Bearer {access_token}"));
    req = req.header("Content-Type", "application/json");

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

/// Send an authenticated request, retrying once after a token refresh on 401.
async fn do_request_with_retry(
    state: &AppState,
    method: Method,
    url: &str,
    body: Option<Bytes>,
) -> anyhow::Result<reqwest::Response> {
    let resp = do_request(state, &method, url, body.as_ref()).await?;
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

    do_request(state, &method, url, body.as_ref()).await
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

/// Stream a backend response back to the client. Upstream status and body —
/// including error responses — are forwarded verbatim so clients see real
/// backend error messages instead of an opaque proxy status.
fn stream_response(resp: reqwest::Response) -> Result<Response<Body>, StatusCode> {
    let status = resp.status();
    let mut builder = Response::builder().status(status.as_u16());

    for (name, value) in resp.headers() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }

    let stream = resp.bytes_stream();
    builder
        .body(Body::from_stream(stream))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// POST /v1/responses — proxy to the Codex backend responses endpoint.
pub async fn responses_handler(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response<Body>, StatusCode> {
    let prepared = apply_body_defaults(body).map_err(|err| {
        tracing::error!("Failed to parse request body: {err}");
        StatusCode::BAD_REQUEST
    })?;

    let url = format!("{}/responses", state.backend_base_url);
    let resp = do_request_with_retry(&state, Method::Post, &url, Some(prepared))
        .await
        .map_err(|err| {
            tracing::error!("Backend request failed: {err}");
            StatusCode::BAD_GATEWAY
        })?;

    stream_response(resp)
}

/// GET /v1/models — proxy to the Codex backend models endpoint.
pub async fn models_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Response<Body>, StatusCode> {
    let url = format!("{}/models", state.backend_base_url);
    let resp = do_request_with_retry(&state, Method::Get, &url, None)
        .await
        .map_err(|err| {
            tracing::error!("Models request failed: {err}");
            StatusCode::BAD_GATEWAY
        })?;

    stream_response(resp)
}
