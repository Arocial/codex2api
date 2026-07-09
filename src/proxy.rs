use axum::body::Body;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

use crate::state::AppState;

/// Cap on short, non-streaming backend calls (e.g. /models). /responses is
/// streaming and may legitimately run for many minutes, so it gets no total
/// timeout — connect-time issues surface as reqwest errors regardless.
const SHORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Large enough for long prompts and tool definitions while still bounding
/// memory use, since request bodies are parsed and serialized in memory.
pub const MAX_REQUEST_BODY_SIZE: usize = 32 * 1024 * 1024;

pub const DEFAULT_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

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
        .credentials()
        .await
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| anyhow::anyhow!("not authenticated — run `codex login` first"))?;

    let access_token = auth.access_token;
    let account_id = auth.account_id;
    let is_fedramp = auth.is_fedramp;

    let mut req = match method {
        Method::Get => state.http_client.get(url).timeout(SHORT_REQUEST_TIMEOUT),
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
fn stream_response(resp: reqwest::Response) -> Result<Response<Body>, ApiError> {
    let status = resp.status();
    let mut builder = Response::builder().status(status.as_u16());

    for (name, value) in resp.headers() {
        if is_hop_by_hop(name.as_str()) {
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

    let prepared = apply_body_defaults(body).map_err(|err| {
        tracing::error!("Failed to parse request body: {err}");
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Invalid JSON request body: {err}"),
            "invalid_request_error",
            "invalid_json",
        )
    })?;

    let url = format!("{}/responses", state.backend_base_url);
    let resp = do_request_with_retry(&state, Method::Post, &url, Some(prepared))
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

    stream_response(resp)
}

/// GET /v1/models — proxy to the Codex backend models endpoint.
pub async fn models_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Response<Body>, ApiError> {
    let url = format!("{}/models", state.backend_base_url);
    let resp = do_request_with_retry(&state, Method::Get, &url, None)
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
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn defaults_as_json(input: &str) -> Value {
        let out = apply_body_defaults(Bytes::from(input.to_string())).expect("ok");
        serde_json::from_slice(&out).expect("valid JSON")
    }

    #[test]
    fn store_defaults_to_false_when_absent() {
        let v = defaults_as_json(r#"{"model":"gpt-5"}"#);
        assert_eq!(v["store"], json!(false));
        assert_eq!(v["stream"], json!(true));
        assert_eq!(v["model"], json!("gpt-5"));
    }

    #[test]
    fn explicit_store_is_preserved() {
        let v = defaults_as_json(r#"{"store":true}"#);
        assert_eq!(v["store"], json!(true));
        assert_eq!(v["stream"], json!(true));

        let v = defaults_as_json(r#"{"store":false}"#);
        assert_eq!(v["store"], json!(false));
    }

    #[test]
    fn stream_is_forced_true_even_if_client_sends_false() {
        let v = defaults_as_json(r#"{"stream":false}"#);
        assert_eq!(v["stream"], json!(true));
    }

    #[test]
    fn non_object_body_is_rejected() {
        assert!(apply_body_defaults(Bytes::from_static(b"[1,2,3]")).is_err());
        assert!(apply_body_defaults(Bytes::from_static(b"\"hi\"")).is_err());
        assert!(apply_body_defaults(Bytes::from_static(b"42")).is_err());
        assert!(apply_body_defaults(Bytes::from_static(b"null")).is_err());
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(apply_body_defaults(Bytes::from_static(b"not json")).is_err());
        assert!(apply_body_defaults(Bytes::from_static(b"{")).is_err());
    }

    #[test]
    fn hop_by_hop_classification() {
        for h in [
            "connection",
            "Keep-Alive",
            "TRANSFER-ENCODING",
            "content-length",
            "upgrade",
        ] {
            assert!(is_hop_by_hop(h), "{h} should be hop-by-hop");
        }
        for h in ["content-type", "x-request-id", "cache-control"] {
            assert!(!is_hop_by_hop(h), "{h} should NOT be hop-by-hop");
        }
    }

    #[test]
    fn api_error_uses_openai_error_shape() {
        let err = ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Missing or invalid API key.",
            "authentication_error",
            "invalid_api_key",
        );
        let value = json!({
            "error": {
                "message": err.message,
                "type": err.error_type,
                "param": null,
                "code": err.code,
            }
        });

        assert_eq!(value["error"]["type"], "authentication_error");
        assert_eq!(value["error"]["code"], "invalid_api_key");
        assert!(value["error"]["param"].is_null());
    }
}
