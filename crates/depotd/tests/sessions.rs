#[path = "support/fake_program.rs"]
mod fake_program;
#[path = "support/temp.rs"]
mod temp;

use std::path::Path;
use std::time::Duration;

use depot_core::{ProfileId, SessionId};
use depotd::adapters::sessions::{
    Boxr, LaunchRequest, MINIMUM_BOXR_VERSION, SessionProfile, SessionState, Sessions, TurnOutcome,
};
use fake_program::FakeProgram;
use temp::TempDir;

const HELP: &str = "\
Launch coding agents and record every session in a local ledger

Usage: boxr [OPTIONS] [PROMPT]

Commands:
  ps        List running sessions
  status    Report one session without blocking
  wait      Block until a session ends
  stop      End a session
  resume    Continue a finished session
  show      Print a session summary
  export    Write an ATIF trajectory

Options:
      --harness <HARNESS>  Harness to launch
      --model <MODEL>      Model to launch
      --effort <EFFORT>    Effort level
      --account <PROFILE>  Account profile to launch with
      --detach             Return the session id and leave the session running
  -h, --help               Print help
";

const HELP_WITHOUT_WAIT: &str = "\
Launch coding agents and record every session in a local ledger

Commands:
  ps        List running sessions
  status    Report one session without blocking
  stop      End a session
  show      Print a session summary

Options:
      --harness <HARNESS>  Harness to launch
      --model <MODEL>      Model to launch
      --effort <EFFORT>    Effort level
      --account <PROFILE>  Account profile to launch with
  -h, --help               Print help
";

fn adapter(fake: &FakeProgram) -> Boxr {
    Boxr::new(fake.program())
}

fn request(directory: &Path) -> LaunchRequest {
    LaunchRequest {
        directory: directory.to_owned(),
        profile: SessionProfile {
            account: Some(ProfileId::new("work")),
            harness: "claude".to_owned(),
            model: "opus".to_owned(),
            effort: "high".to_owned(),
        },
        kind: Some("build".to_owned()),
        prompt: "fix the login redirect".to_owned(),
    }
}

#[test]
fn drives_a_headless_session_from_launch_to_turn_end() {
    let dir = TempDir::new("sessions-round-trip");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&worktree).expect("the worktree directory is created");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond("--version", "boxr 0.3.1\n", "", 0);
    fake.respond("--help", HELP, "", 0);
    fake.respond("--harness", "session: 4f2a91\nstatus: running\n", "", 0);
    fake.respond("status", "session: 4f2a91\nstate: finished\n", "", 0);
    fake.respond(
        "wait",
        "session: 4f2a91\nstatus: ok\nmessage: \"the redirect points at /login\"\ntokens: 1234\n",
        "",
        0,
    );
    fake.respond("resume", "session: 4f2a91\nstatus: running\n", "", 0);
    fake.respond("stop", "session: 4f2a91\nstate: stopped\n", "", 0);
    fake.respond(
        "ps",
        "sessions[2]{id,state,harness,model}:\n  4f2a91,finished,claude,opus\n  9b01cc,running,pi,glm-5.3\n",
        "",
        0,
    );

    let boxr = adapter(&fake);
    let capabilities = boxr.capabilities().expect("boxr reports its surface");
    assert_eq!(capabilities.version, "0.3.1");
    assert!(capabilities.surface.contains(&"wait".to_owned()));

    let session = boxr
        .launch(&request(&worktree))
        .expect("the session is launched");
    assert_eq!(session, SessionId::new("4f2a91"));

    assert_eq!(
        boxr.status(&session).expect("the session state is read"),
        SessionState::Finished
    );

    assert_eq!(
        boxr.wait(&session, None).expect("the turn ends"),
        TurnOutcome::Completed
    );
    fake.respond("wait", "session: 4f2a91\nstatus: running\n", "", 0);
    assert_eq!(
        boxr.wait(&session, Some(Duration::from_secs(30)))
            .expect("the wait returns on its timeout"),
        TurnOutcome::StillRunning
    );

    boxr.resume(&session, "add a test for the redirect")
        .expect("the session resumes");
    boxr.stop(&session).expect("the session stops");

    let sessions = boxr.list().expect("the sessions are listed");
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].session, SessionId::new("4f2a91"));
    assert_eq!(sessions[0].state, SessionState::Finished);
    assert_eq!(sessions[0].harness, "claude");
    assert_eq!(sessions[1].session, SessionId::new("9b01cc"));
    assert_eq!(sessions[1].state, SessionState::Running);
    assert_eq!(sessions[1].model, "glm-5.3");

    assert_eq!(
        fake.calls(),
        vec![
            "--version",
            "--help",
            "--harness claude --model opus --effort high --account work --kind build --detach fix the login redirect",
            "status 4f2a91",
            "wait 4f2a91",
            "wait 4f2a91 --timeout 30",
            "resume 4f2a91 add a test for the redirect",
            "stop 4f2a91",
            "ps",
        ]
    );
}

#[test]
fn accepts_a_bare_session_id_on_stdout() {
    let dir = TempDir::new("sessions-bare-id");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond("--harness", "4f2a91\n", "", 0);

    let session = adapter(&fake)
        .launch(&request(dir.path()))
        .expect("a bare session id is read");
    assert_eq!(session, SessionId::new("4f2a91"));
}

#[test]
fn omits_account_when_the_profile_has_none() {
    let dir = TempDir::new("sessions-no-account");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&worktree).expect("the worktree directory is created");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond("--harness", "session: 4f2a91\n", "", 0);

    let mut launch = request(&worktree);
    launch.profile.account = None;
    let session = adapter(&fake)
        .launch(&launch)
        .expect("a launch without an account succeeds");
    assert_eq!(session, SessionId::new("4f2a91"));
    assert_eq!(
        fake.calls(),
        vec![
            "--harness claude --model opus --effort high --kind build --detach fix the login redirect"
        ]
    );
}

#[test]
fn names_every_missing_capability_and_the_minimum_version() {
    let dir = TempDir::new("sessions-missing-capability");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond("--version", "boxr 0.1.0\n", "", 0);
    fake.respond("--help", HELP_WITHOUT_WAIT, "", 0);

    let error = adapter(&fake)
        .capabilities()
        .expect_err("a missing capability is refused");
    let message = error.to_string();
    for missing in ["wait", "resume", "--detach"] {
        assert!(
            message.contains(missing),
            "{message} does not name {missing}"
        );
    }
    assert!(message.contains(MINIMUM_BOXR_VERSION), "{message}");
}

#[test]
fn refuses_a_boxr_older_than_the_surface_it_needs() {
    let dir = TempDir::new("sessions-too-old");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond("--version", "boxr 0.1.9\n", "", 0);
    fake.respond("--help", HELP, "", 0);

    let error = adapter(&fake)
        .capabilities()
        .expect_err("an old boxr is refused");
    let message = error.to_string();
    assert!(message.contains("0.1.9"), "{message}");
    assert!(message.contains(MINIMUM_BOXR_VERSION), "{message}");
}

#[test]
fn fails_loudly_when_boxr_refuses_a_launch() {
    let dir = TempDir::new("sessions-launch-refused");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond(
        "--harness",
        "",
        "boxr: the claude profile work is not logged in\n",
        1,
    );

    let error = adapter(&fake)
        .launch(&request(dir.path()))
        .expect_err("the launch failure is reported");
    let message = error.to_string();
    assert!(message.contains("--harness claude"), "{message}");
    assert!(message.contains("not logged in"), "{message}");
    assert!(message.contains("status 1"), "{message}");
}

#[test]
fn never_guesses_a_session_id() {
    let dir = TempDir::new("sessions-no-id");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond("--harness", "status: running\nmessage: started\n", "", 0);

    let error = adapter(&fake)
        .launch(&request(dir.path()))
        .expect_err("a launch without an id is refused");
    assert!(error.to_string().contains("never guesses one"), "{error}");

    fake.respond("--harness", "not toon at all\n", "", 0);
    let error = adapter(&fake)
        .launch(&request(dir.path()))
        .expect_err("unreadable output is refused");
    assert!(error.to_string().contains("cannot read"), "{error}");
}

#[test]
fn reads_nested_boxr_status_wait_and_launch_fields() {
    let dir = TempDir::new("sessions-nested-toon");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&worktree).expect("the worktree directory is created");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond(
        "--harness",
        "session:\n  id: s-4f2a91\n  status: running\n  harness: pi\n  model: xai/grok-4.5\n",
        "",
        0,
    );
    fake.respond(
        "status",
        "session:\n  id: s-4f2a91\n  status: ok\n  state: finished\n  harness: pi\n",
        "",
        0,
    );
    fake.respond(
        "wait",
        "session:\n  id: s-4f2a91\n  status: ok\n  state: finished\nhelp[1]:\n  done\n",
        "",
        0,
    );

    let boxr = adapter(&fake);
    let session = boxr
        .launch(&request(&worktree))
        .expect("nested launch id is read");
    assert_eq!(session, SessionId::new("s-4f2a91"));
    assert_eq!(
        boxr.status(&session).expect("nested status state is read"),
        SessionState::Finished
    );
    assert_eq!(
        boxr.wait(&session, None)
            .expect("nested wait status is read"),
        TurnOutcome::Completed
    );
}

#[test]
fn refuses_a_session_state_it_does_not_know() {
    let dir = TempDir::new("sessions-unknown-state");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond("status", "session: 4f2a91\nstate: wobbling\n", "", 0);

    let error = adapter(&fake)
        .status(&SessionId::new("4f2a91"))
        .expect_err("an unknown state is refused");
    let message = error.to_string();
    assert!(message.contains("wobbling"), "{message}");
    assert!(message.contains("status 4f2a91"), "{message}");
}

#[test]
fn refuses_a_session_list_that_does_not_match_its_own_header() {
    let dir = TempDir::new("sessions-short-list");
    let fake = FakeProgram::new(dir.path(), "boxr");
    fake.respond(
        "ps",
        "sessions[2]{id,state}:\n  4f2a91,finished\nstatus: ok\n",
        "",
        0,
    );

    let error = adapter(&fake)
        .list()
        .expect_err("a truncated table is refused");
    assert!(error.to_string().contains("2 rows"), "{error}");
}
