#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn supervisor() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("depotd-supervisor.sh")
}

fn rotate(log: &Path, cap: u64, backups: u64) {
    let output = Command::new("sh")
        .arg(supervisor())
        .args(["rotate", log.to_str().expect("utf-8 path")])
        .env("DEPOTD_LOG_MAX_BYTES", cap.to_string())
        .env("DEPOTD_LOG_BACKUPS", backups.to_string())
        .output()
        .expect("the shipped supervisor runs");
    assert!(
        output.status.success(),
        "rotate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_shipped_supervisor_keeps_three_bounded_backups() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let log = temp.path().join("depotd.log");

    for round in 1..=5 {
        std::fs::write(&log, format!("round-{round}-payload\n")).expect("the log is written");
        rotate(&log, 8, 3);
    }

    assert!(
        !log.exists(),
        "each rotation moves the live log into the first backup"
    );
    for backup in 1..=3 {
        let path = temp.path().join(format!("depotd.log.{backup}"));
        assert!(path.exists(), "{} is kept", path.display());
    }
    assert!(
        !temp.path().join("depotd.log.4").exists(),
        "the supervisor never keeps more than three backups"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("depotd.log.1")).expect("backup one"),
        "round-5-payload\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("depotd.log.3")).expect("backup three"),
        "round-3-payload\n"
    );
}

#[test]
fn a_log_below_the_cap_is_left_alone() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let log = temp.path().join("depotd.log");
    std::fs::write(&log, "one line\n").expect("the log is written");

    rotate(&log, 1_048_576, 3);

    assert!(log.exists());
    assert!(!temp.path().join("depotd.log.1").exists());
}

#[test]
fn the_default_cap_is_ten_mebibytes_with_three_backups() {
    let output = Command::new("sh")
        .arg(supervisor())
        .arg("help")
        .output()
        .expect("the shipped supervisor runs");
    let help = String::from_utf8_lossy(&output.stdout);

    assert!(help.contains("10485760"), "got {help}");
    assert!(help.contains("depotd.log.3"), "got {help}");
}
