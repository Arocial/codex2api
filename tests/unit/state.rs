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
