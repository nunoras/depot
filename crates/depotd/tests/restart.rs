mod support;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use depot_core::Timestamp;
use depotd::{
    DAEMON_LOG_FILE_NAME, DAEMON_SCOPE_FILE_NAME, DAEMON_STOP_FILE_NAME, InstanceLock,
    RestartOptions, clear_stop_request, daemon_build_mismatch, daemon_scope, launch_spec,
    restart_daemon, stop_requested,
};

const SCOPE_WRITING_DAEMON: &str = r#"
use std::io::Write;

fn main() {
    let cwd = std::env::current_dir().expect("cwd");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let projects = args
        .iter()
        .filter(|argument| argument.as_str() != "--project")
        .map(|project| format!("\"{project}\""))
        .collect::<Vec<_>>()
        .join(",");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    let body = format!(
        "{{\"pid\":{},\"started_at_millis\":{now},\"projects\":[{projects}],\"heartbeat_millis\":{now},\"build_id\":\"\"}}",
        std::process::id()
    );
    let mut scope = std::fs::File::create(cwd.join("depotd.scope.json")).expect("scope");
    scope.write_all(body.as_bytes()).expect("scope");
    println!("args: {}", args.join(" "));
    println!("cwd: {}", cwd.display());
}
"#;

const SILENT_DAEMON: &str = r#"
fn main() {
    std::thread::sleep(std::time::Duration::from_secs(5));
}
"#;

fn compile_daemon(name: &str, source: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("restart-daemons");
    std::fs::create_dir_all(&directory).expect("the helper directory");
    let source_path = directory.join(format!("{name}.rs"));
    std::fs::write(&source_path, source).expect("the daemon source");
    let binary = directory.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = std::process::Command::new(rustc)
        .arg("--edition")
        .arg("2024")
        .arg(&source_path)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("rustc runs");
    assert!(
        output.status.success(),
        "the fake daemon {name} could not be built: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

fn recording_daemon() -> PathBuf {
    static COMPILED: OnceLock<PathBuf> = OnceLock::new();
    COMPILED
        .get_or_init(|| compile_daemon("scope_writing_daemon", SCOPE_WRITING_DAEMON))
        .clone()
}

fn silent_daemon() -> PathBuf {
    static COMPILED: OnceLock<PathBuf> = OnceLock::new();
    COMPILED
        .get_or_init(|| compile_daemon("silent_daemon", SILENT_DAEMON))
        .clone()
}

fn wait_for_log(log: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(log)
            && text.contains("args:")
        {
            return text;
        }
        assert!(Instant::now() < deadline, "the log stayed empty");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_daemon_reports_its_version_and_build_id() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_depotd"))
        .arg("--version")
        .output()
        .expect("the daemon runs");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("depotd "), "got {stdout}");
    assert!(stdout.contains("(build "), "got {stdout}");
    assert!(
        stdout.contains(depotd::BUILD_ID),
        "the reported build id must be the embedded one, got {stdout}"
    );
}

#[test]
fn a_restart_stops_the_recorded_daemon_and_relaunches_with_its_scope() {
    let fixture = support::fixture();
    let project = support::register(&fixture, "example");
    let second = support::register(&fixture, "second");
    let lock = InstanceLock::acquire(&fixture.home).expect("the lock");
    let scope = lock
        .record_scope(&[project.project.clone(), second.project.clone()])
        .expect("the scope");

    let home = fixture.home.clone();
    let watcher = {
        let home = home.clone();
        let pid = scope.pid;
        let started = scope.started_at_millis;
        std::thread::spawn(move || {
            while !stop_requested(&home, pid, started).expect("the stop request is readable") {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(lock);
        })
    };

    let program = recording_daemon();
    let restarted = restart_daemon(
        &home,
        &RestartOptions {
            program,
            timeout: Duration::from_secs(10),
            takeover_timeout: Duration::from_secs(10),
        },
    )
    .expect("the daemon restarts");
    watcher.join().expect("the watcher finishes");

    assert_eq!(restarted.stopped, Some(scope.pid));
    assert_eq!(restarted.projects, vec!["example", "second"]);
    assert!(restarted.pid > 0);
    assert_eq!(restarted.log, home.root().join(DAEMON_LOG_FILE_NAME));
    assert!(
        !home.root().join(DAEMON_STOP_FILE_NAME).exists(),
        "the stop request is cleared once the daemon is gone"
    );

    let logged = wait_for_log(&restarted.log);
    assert!(
        logged.contains("args: --project example --project second"),
        "the relaunch must preserve the covered scope, got {logged}"
    );
    assert!(
        logged.contains(&format!("cwd: {}", home.root().display())),
        "the relaunch runs from the depot home, got {logged}"
    );
}

#[test]
fn a_restart_gives_up_after_the_bound_and_never_touches_the_holder() {
    let fixture = support::fixture();
    let project = support::register(&fixture, "example");
    let lock = InstanceLock::acquire(&fixture.home).expect("the lock");
    let scope = lock
        .record_scope(std::slice::from_ref(&project.project))
        .expect("the scope");

    let program = recording_daemon();
    let error = restart_daemon(
        &fixture.home,
        &RestartOptions {
            program,
            timeout: Duration::from_millis(300),
            takeover_timeout: Duration::from_secs(5),
        },
    )
    .expect_err("a holder that never stops ends the wait");
    let message = error.to_string();
    assert!(
        message.contains(&format!("pid {}", scope.pid)),
        "the refusal names the holder, got {message}"
    );
    assert!(message.contains("did not stop"), "got {message}");

    assert!(
        InstanceLock::acquire(&fixture.home).is_err(),
        "the holder still owns the lock"
    );
    assert!(
        !fixture.home.root().join(DAEMON_LOG_FILE_NAME).exists(),
        "a bounded failure must not launch a second daemon"
    );
}

#[test]
fn a_restart_leaves_a_lock_record_it_cannot_read_alone() {
    let fixture = support::fixture();
    let lock = InstanceLock::acquire(&fixture.home).expect("the lock");
    let path = fixture.home.root().join(DAEMON_SCOPE_FILE_NAME);
    std::fs::write(&path, b"{}").expect("a record without a pid");

    assert!(daemon_scope(&fixture.home).is_none());

    let program = recording_daemon();
    let restarted = restart_daemon(
        &fixture.home,
        &RestartOptions {
            program,
            timeout: Duration::from_millis(300),
            takeover_timeout: Duration::from_secs(5),
        },
    )
    .expect("an unreadable record is not a reason to fail the launch");

    assert_eq!(restarted.stopped, None);
    assert!(
        !fixture.home.root().join(DAEMON_STOP_FILE_NAME).exists(),
        "nothing may be asked to stop when the holder is unidentified"
    );
    assert!(
        InstanceLock::acquire(&fixture.home).is_err(),
        "the unidentified holder keeps the lock"
    );
    assert!(lock.refresh_heartbeat().is_ok());
}

#[test]
fn a_restart_fails_when_the_new_daemon_never_takes_the_instance_lock() {
    let fixture = support::fixture();
    let _lock = InstanceLock::acquire(&fixture.home).expect("the lock");

    let error = restart_daemon(
        &fixture.home,
        &RestartOptions {
            program: silent_daemon(),
            timeout: Duration::from_millis(300),
            takeover_timeout: Duration::from_millis(300),
        },
    )
    .expect_err("a daemon that never takes the lock ends the wait");
    let message = error.to_string();
    assert!(
        message.contains("did not take the instance lock"),
        "the refusal says the new daemon never took over, got {message}"
    );
    assert!(
        !fixture.home.root().join(DAEMON_SCOPE_FILE_NAME).exists(),
        "the silent daemon must not have recorded a scope"
    );
    assert!(
        daemon_scope(&fixture.home).is_none(),
        "no scope names the launched daemon"
    );
}

#[test]
fn a_second_daemon_is_refused_with_the_holder_identity() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    let lock = InstanceLock::acquire(&fixture.home).expect("the lock");
    lock.record_scope(&[first.project, second.project])
        .expect("the scope");

    let error = InstanceLock::acquire(&fixture.home).expect_err("the lock is taken");
    let message = error.to_string();
    assert!(
        message.contains(&format!("pid {}", std::process::id())),
        "the refusal names the holder pid, got {message}"
    );
    assert!(message.contains("first, second"), "got {message}");
}

#[test]
fn a_second_daemon_names_an_unreadable_lock_record_as_such() {
    let fixture = support::fixture();
    let _lock = InstanceLock::acquire(&fixture.home).expect("the lock");
    std::fs::write(fixture.home.root().join(depotd::DAEMON_LOCK_FILE_NAME), b"")
        .expect("an empty record");

    let error = InstanceLock::acquire(&fixture.home).expect_err("the lock is taken");
    let message = error.to_string();
    assert!(
        message.contains("names no daemon"),
        "the refusal must degrade gracefully, got {message}"
    );
}

#[test]
fn a_stop_request_only_moves_the_identity_it_names() {
    let fixture = support::fixture();
    let home = &fixture.home;
    assert!(!stop_requested(home, 41, 7).expect("no request yet"));

    std::fs::write(
        home.root().join(DAEMON_STOP_FILE_NAME),
        br#"{"pid":41,"started_at_millis":7}"#,
    )
    .expect("a request");

    assert!(stop_requested(home, 41, 7).expect("read"));
    assert!(!stop_requested(home, 41, 8).expect("a later start is another daemon"));
    assert!(!stop_requested(home, 42, 7).expect("another pid is not ours"));

    clear_stop_request(home).expect("cleared");
    assert!(!stop_requested(home, 41, 7).expect("read"));
}

#[test]
fn a_launch_spec_carries_the_projects_and_the_home_log() {
    let fixture = support::fixture();
    let home = &fixture.home;
    let spec = launch_spec(home, PathBuf::from("depotd"), &["first".to_string()]);

    assert_eq!(spec.arguments, vec!["--project", "first"]);
    assert_eq!(spec.directory, home.root());
    assert_eq!(spec.log, home.root().join(DAEMON_LOG_FILE_NAME));
}

#[test]
fn a_build_mismatch_is_reported_only_for_a_fresh_lock_from_another_commit() {
    let fixture = support::fixture();
    let project = support::register(&fixture, "example");
    let lock = InstanceLock::acquire(&fixture.home).expect("the lock");
    lock.record_scope(std::slice::from_ref(&project.project))
        .expect("the scope");

    let now = Timestamp::from_millis(
        daemon_scope(&fixture.home)
            .expect("the record")
            .heartbeat_millis,
    );
    let stale_after = Duration::from_secs(90);
    assert!(daemon_build_mismatch(&fixture.home, now, stale_after).is_none());

    let path = fixture.home.root().join(DAEMON_SCOPE_FILE_NAME);
    let mut scope: depotd::DaemonScope =
        serde_json::from_slice(&std::fs::read(&path).expect("record")).expect("parsed");
    scope.build_id = "0ldbu11d".to_string();
    std::fs::write(&path, serde_json::to_vec(&scope).expect("encoded")).expect("rewritten");

    let mismatch = daemon_build_mismatch(&fixture.home, now, stale_after).expect("a mismatch");
    assert_eq!(mismatch.build_id, "0ldbu11d");

    scope.heartbeat_millis = 0;
    std::fs::write(&path, serde_json::to_vec(&scope).expect("encoded")).expect("rewritten");
    assert!(
        daemon_build_mismatch(&fixture.home, now, stale_after).is_none(),
        "a stale heartbeat means no daemon is running"
    );
}

#[test]
fn a_record_without_a_build_id_stays_quiet() {
    let fixture = support::fixture();
    let home = &fixture.home;
    std::fs::write(
        home.root().join(DAEMON_SCOPE_FILE_NAME),
        br#"{"pid":41,"started_at_millis":1,"projects":["example"],"heartbeat_millis":1}"#,
    )
    .expect("a legacy record");

    let scope = daemon_scope(home).expect("a legacy record still parses");
    assert_eq!(scope.build_id, "");
    assert_eq!(scope.projects, vec!["example"]);
    assert!(
        daemon_build_mismatch(home, Timestamp::from_millis(1), Duration::from_secs(90)).is_none(),
        "a legacy lock cannot be compared and must not warn"
    );
}

#[test]
fn a_missing_daemon_beside_the_client_says_how_to_install_it() {
    let error = depotd::installed_daemon().expect_err("the test binary has no depotd beside it");
    let message = error.to_string();
    assert!(message.contains("depotd"), "got {message}");
    assert!(message.contains("cargo install"), "got {message}");
}
