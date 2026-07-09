use super::*;
use axum::http::HeaderValue;
use serde_json::{json, Value};

const CONTEXT_SESSION_ID: &str = "01890f3e-7b2c-7a1d-8e4f-123456789abc";

fn test_context() -> CodexRequestContext {
    CodexRequestContext::new(
        Uuid::parse_str(CONTEXT_SESSION_ID).unwrap(),
        Uuid::parse_str("f691edef-06a3-477d-9a17-7ae9ea4a991a").unwrap(),
    )
}

fn defaults_as_json(input: &str) -> Value {
    let out = prepare_responses_body(Bytes::from(input.to_string()), &test_context()).expect("ok");
    serde_json::from_slice(&out).expect("valid JSON")
}

#[test]
fn store_defaults_to_false_when_absent() {
    let v = defaults_as_json(r#"{"model":"gpt-5"}"#);
    assert_eq!(v["store"], json!(false));
    assert_eq!(v["stream"], json!(true));
    assert_eq!(v["model"], json!("gpt-5"));
    assert_eq!(v["tool_choice"], json!("auto"));
    assert_eq!(v["parallel_tool_calls"], json!(false));
    assert_eq!(v["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(v["prompt_cache_key"], json!(CONTEXT_SESSION_ID));
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
    let ctx = test_context();
    assert!(prepare_responses_body(Bytes::from_static(b"[1,2,3]"), &ctx).is_err());
    assert!(prepare_responses_body(Bytes::from_static(b"\"hi\""), &ctx).is_err());
    assert!(prepare_responses_body(Bytes::from_static(b"42"), &ctx).is_err());
    assert!(prepare_responses_body(Bytes::from_static(b"null"), &ctx).is_err());
}

#[test]
fn invalid_json_is_rejected() {
    let ctx = test_context();
    assert!(prepare_responses_body(Bytes::from_static(b"not json"), &ctx).is_err());
    assert!(prepare_responses_body(Bytes::from_static(b"{"), &ctx).is_err());
}

#[test]
fn session_id_precedence_and_absence() {
    let mut headers = HeaderMap::new();
    headers.insert("session-id", HeaderValue::from_static("fallback"));
    headers.insert("x-session-id", HeaderValue::from_static("preferred"));
    assert_eq!(requested_session_id(&headers).as_deref(), Some("preferred"));
    assert_eq!(requested_session_id(&HeaderMap::new()), None);
}

#[test]
fn codex_metadata_is_coherent_and_custom_metadata_survives() {
    let v = defaults_as_json(
        r#"{
            "client_metadata": {
                "custom": "keep",
                "session_id": "spoofed"
            }
        }"#,
    );
    let metadata = &v["client_metadata"];
    assert_eq!(metadata["custom"], "keep");
    assert_eq!(metadata["session_id"], CONTEXT_SESSION_ID);
    assert_eq!(metadata["thread_id"], CONTEXT_SESSION_ID);
    assert_eq!(
        metadata["x-codex-window-id"],
        Value::String(format!("{CONTEXT_SESSION_ID}:0"))
    );
    assert_eq!(
        metadata["x-codex-installation-id"],
        "f691edef-06a3-477d-9a17-7ae9ea4a991a"
    );

    let turn_metadata: Value =
        serde_json::from_str(metadata["x-codex-turn-metadata"].as_str().unwrap()).unwrap();
    assert_eq!(turn_metadata["session_id"], CONTEXT_SESSION_ID);
    assert_eq!(turn_metadata["thread_id"], CONTEXT_SESSION_ID);
    assert_eq!(
        turn_metadata["window_id"],
        format!("{CONTEXT_SESSION_ID}:0")
    );
    assert_eq!(turn_metadata["turn_id"], metadata["turn_id"]);
}

#[test]
fn non_object_client_metadata_is_rejected() {
    assert!(prepare_responses_body(
        Bytes::from_static(br#"{"client_metadata":"invalid"}"#),
        &test_context()
    )
    .is_err());
}

#[test]
fn null_client_metadata_is_populated() {
    let v = defaults_as_json(r#"{"client_metadata":null}"#);
    assert_eq!(
        v["client_metadata"]["session_id"],
        Value::String(CONTEXT_SESSION_ID.into())
    );
}

#[test]
fn codex_headers_match_request_context() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        dir.path().to_path_buf(),
        "https://example.com".into(),
        "0.142.5".into(),
        "key".into(),
    )
    .unwrap();
    let ctx = test_context();
    let request = apply_codex_request_headers(
        state.http_client.post("https://example.com/responses"),
        &state,
        &ctx,
    )
    .build()
    .unwrap();
    let headers = request.headers();

    assert_eq!(headers["version"], "0.142.5");
    assert_eq!(headers["originator"], "codex-tui");
    assert_eq!(headers["accept"], "text/event-stream");
    assert_eq!(headers["session-id"], CONTEXT_SESSION_ID);
    assert_eq!(headers["thread-id"], CONTEXT_SESSION_ID);
    assert_eq!(headers["x-client-request-id"], CONTEXT_SESSION_ID);
    assert_eq!(
        headers["x-codex-window-id"],
        format!("{CONTEXT_SESSION_ID}:0")
    );
    assert_eq!(headers["x-codex-turn-metadata"], ctx.turn_metadata.as_str());
    assert!(headers["user-agent"]
        .to_str()
        .unwrap()
        .starts_with("codex-tui/0.142.5 ("));
}

#[test]
fn models_url_includes_required_client_version() {
    assert_eq!(
        models_url("https://example.com/backend-api/codex", "1.2.3"),
        "https://example.com/backend-api/codex/models?client_version=1.2.3"
    );
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
