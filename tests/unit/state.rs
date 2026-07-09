use super::*;

#[test]
fn installation_id_is_created_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let first = load_or_create_installation_id(&dir.path().to_path_buf()).unwrap();
    let second = load_or_create_installation_id(&dir.path().to_path_buf()).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("installation_id"))
            .unwrap()
            .trim(),
        first.to_string()
    );
}

#[test]
fn invalid_existing_installation_id_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("installation_id"), "not-a-uuid").unwrap();
    assert!(load_or_create_installation_id(&dir.path().to_path_buf()).is_err());
}

#[test]
fn client_session_ids_map_to_stable_local_uuid_v7_ids() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        dir.path().to_path_buf(),
        "https://example.com".into(),
        "0.142.5".into(),
        "key".into(),
    )
    .unwrap();

    let first = state.resolve_session_id(Some("arbitrary-client-id".into()));
    let repeated = state.resolve_session_id(Some("arbitrary-client-id".into()));
    let different = state.resolve_session_id(Some("another-client-id".into()));

    assert_eq!(first.get_version_num(), 7);
    assert_eq!(first, repeated);
    assert_ne!(first, different);
    assert_eq!(different.get_version_num(), 7);
}

#[test]
fn client_turn_ids_map_to_stable_local_uuid_v7_ids() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        dir.path().to_path_buf(),
        "https://example.com".into(),
        "0.142.5".into(),
        "key".into(),
    )
    .unwrap();

    let first = state.resolve_turn_id(Some("arbitrary-client-turn".into()));
    let repeated = state.resolve_turn_id(Some("arbitrary-client-turn".into()));
    let different = state.resolve_turn_id(Some("another-client-turn".into()));
    let same_text_session = state.resolve_session_id(Some("arbitrary-client-turn".into()));

    assert_eq!(first.get_version_num(), 7);
    assert_eq!(first, repeated);
    assert_ne!(first, different);
    assert_ne!(first, same_text_session);
    assert_eq!(different.get_version_num(), 7);
}

#[test]
fn missing_client_session_id_gets_a_fresh_uuid_v7() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        dir.path().to_path_buf(),
        "https://example.com".into(),
        "0.142.5".into(),
        "key".into(),
    )
    .unwrap();

    let first = state.resolve_session_id(None);
    let second = state.resolve_session_id(None);

    assert_eq!(first.get_version_num(), 7);
    assert_eq!(second.get_version_num(), 7);
    assert_ne!(first, second);
}
