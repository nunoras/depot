#[allow(dead_code)]
#[path = "support/fake_forge.rs"]
mod fake_forge;
#[path = "support/fake_typesafe.rs"]
mod fake_typesafe;

use depotd::adapters::typesafe::{RuleMatcher, Typesafe, read_key};
use fake_typesafe::{CONDITION, NEUTRAL};

#[test]
fn the_real_choice_edge_accepts_only_protocol_answers() {
    let server = fake_typesafe::endpoint(CONDITION, 0.95);
    let client = Typesafe::new(&server.base_url(), "secret".into());
    let answer = client
        .choose("example", "Repair", "Fix the bug", &[CONDITION.into()])
        .unwrap();
    assert_eq!(answer.chosen, Some(0));
    assert_eq!(answer.confidence.value(), 0.95);
    assert_eq!(answer.model_version, "jev-1.13.0");
    for body in [
        "not json".to_owned(),
        serde_json::json!({"model": "jev-1.13.0", "answers": {}}).to_string(),
        serde_json::json!({"model": "jev-1.13.0", "answers": {"dispatch": {"type": "choice", "choice": "invented", "confidence": 0.99, "probabilities": {CONDITION: 0.99, NEUTRAL: 0.01}}}}).to_string(),
        serde_json::json!({"model": "jev-1.13.0", "answers": {"dispatch": {"type": "choice", "choice": CONDITION, "confidence": 1.1, "probabilities": {CONDITION: 0.99, NEUTRAL: 0.01}}}}).to_string(),
    ] {
        server.replace_route("POST", "/v1/systemone", 200, &body);
        assert!(client.choose("example", "Repair", "Fix the bug", &[CONDITION.into()]).is_err());
    }
    for status in [401, 422, 429, 500, 529, 302] {
        server.replace_route("POST", "/v1/systemone", status, "secret must not appear");
        let error = client
            .choose("example", "Repair", "Fix the bug", &[CONDITION.into()])
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(&status.to_string()), "{error}");
        assert!(!error.contains("secret must not appear"));
    }
}

#[test]
fn key_files_are_named_and_permission_checked() {
    let directory = tempfile::tempdir().unwrap();
    let error = read_key(directory.path()).unwrap_err().to_string();
    assert!(error.contains("typesafe-key"), "{error}");
    fake_typesafe::key(directory.path());
    assert_eq!(read_key(directory.path()).unwrap(), "typesafe-test-secret");
    std::fs::write(directory.path().join("typesafe-key"), "").unwrap();
    assert!(
        read_key(directory.path())
            .unwrap_err()
            .to_string()
            .contains("empty")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            directory.path().join("typesafe-key"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(
            read_key(directory.path())
                .unwrap_err()
                .to_string()
                .contains("mode 644")
        );
    }
    std::fs::remove_file(directory.path().join("typesafe-key")).unwrap();
    std::fs::create_dir(directory.path().join("typesafe-key")).unwrap();
    assert!(
        read_key(directory.path())
            .unwrap_err()
            .to_string()
            .contains("typesafe-key")
    );
}
