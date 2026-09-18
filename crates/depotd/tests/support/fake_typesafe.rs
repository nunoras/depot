use super::fake_forge::FakeForge;
use serde_json::json;

pub const CONDITION: &str = "Implement or repair software";
pub const NEUTRAL: &str = "This work matches none of the conditions above.";

pub fn endpoint(choice: &str, confidence: f64) -> FakeForge {
    let server = FakeForge::start();
    let probabilities = if choice == NEUTRAL {
        json!({ CONDITION: 0.01, NEUTRAL: 0.99 })
    } else {
        json!({ CONDITION: 0.99, NEUTRAL: 0.01 })
    };
    server.route("POST", "/v1/systemone", 200, &json!({
        "model": "jev-1.13.0",
        "answers": {"dispatch": {"type": "choice", "choice": choice, "confidence": confidence, "probabilities": probabilities}},
        "usage": {"input_tokens": 50, "output_tokens": 0}
    }).to_string());
    server
}

pub fn key(home: &std::path::Path) {
    let path = home.join("typesafe-key");
    std::fs::write(&path, "typesafe-test-secret").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap();
        let status = std::process::Command::new("icacls")
            .arg(&path)
            .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
            .status()
            .unwrap();
        assert!(status.success());
    }
}
