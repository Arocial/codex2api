use super::*;

fn jwt(payload: &str) -> String {
    format!(
        "e30.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
    )
}

#[test]
fn parses_codex_auth_fixture_and_preserves_account_fields() {
    let dir = tempfile::tempdir().unwrap();
    let auth = serde_json::json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "id_token": jwt(r#"{"https://api.openai.com/auth":{"chatgpt_account_is_fedramp":true}}"#),
            "access_token": "access",
            "refresh_token": "refresh",
            "account_id": "account"
        },
        "last_refresh": "2026-01-01T00:00:00Z",
        "future_field": {"must": "survive"}
    });
    save_document(&dir.path().join("auth.json"), &auth).unwrap();
    let credentials = load_credentials(&dir.path().join("auth.json"))
        .unwrap()
        .unwrap();
    assert_eq!(credentials.access_token, "access");
    assert_eq!(credentials.account_id.as_deref(), Some("account"));
    assert!(credentials.is_fedramp);

    let mut refreshed_document = auth;
    apply_refresh_response(
        &mut refreshed_document,
        RefreshResponse {
            id_token: None,
            access_token: Some("new-access".into()),
            refresh_token: None,
        },
    )
    .unwrap();
    assert_eq!(
        refreshed_document["future_field"],
        serde_json::json!({"must": "survive"})
    );
    assert_eq!(refreshed_document["tokens"]["refresh_token"], "refresh");
}

#[test]
fn refresh_request_matches_codex_snapshot() {
    let request = serde_json::to_value(RefreshRequest {
        client_id: CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token: "refresh".into(),
    })
    .unwrap();
    assert_eq!(
        request,
        serde_json::json!({
            "client_id": "app_EMoamEEZ73f0CkXaXp7hrann",
            "grant_type": "refresh_token",
            "refresh_token": "refresh"
        })
    );
}

#[test]
fn default_headers_match_codex_snapshot() {
    assert_eq!(codex_default_headers()["originator"], "codex_cli_rs");
    assert!(codex_user_agent().starts_with("codex_cli_rs/0.0.0 ("));
    let tui = codex_tui_user_agent("1.2.3");
    assert!(tui.starts_with("codex-tui/1.2.3 ("));
    assert!(tui.ends_with(" xterm (codex-tui; 1.2.3)"));
}
