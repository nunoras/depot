#[allow(dead_code)]
#[path = "support/fake_forge.rs"]
mod fake_forge;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use depot_core::{Fact, FactKind, ProjectId, TaskId, Timestamp, WorktreeLease};
use depotd::adapters::sessions::{
    Capabilities, LaunchRequest, SessionError, SessionState, SessionSummary, Sessions, TurnOutcome,
};
use depotd::adapters::worktrees::{AcquireRequest, Lease, PoolEntry, WorktreeError, Worktrees};
use depotd::{
    Daemon, DepotHome, EvidenceRunner, ForgeDelivery, LocationKind, NoEventHook, Project, Settings,
    ShellValidation, Store,
};
use fake_forge::FakeForge;
use tempfile::TempDir;

const REPO_PATH: &str = "/repos/acme/widget";

#[derive(Clone, Default)]
struct FakeWorktrees;

impl Worktrees for FakeWorktrees {
    fn acquire(&self, _request: &AcquireRequest) -> Result<Lease, WorktreeError> {
        unreachable!("evidence scenarios never acquire a worktree")
    }

    fn release(&self, _lease: &Lease) -> Result<(), WorktreeError> {
        Ok(())
    }

    fn pool(&self, repo: &std::path::Path) -> Result<Vec<PoolEntry>, WorktreeError> {
        Ok(vec![PoolEntry {
            name: "1".to_owned(),
            path: repo.join("worktree"),
            state: "leased".to_owned(),
            lease: Some(WorktreeLease::new("w1")),
            holder: Some("depot:t-1".to_owned()),
        }])
    }

    fn branches(&self, _repo: &std::path::Path) -> Result<Vec<String>, WorktreeError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Default)]
struct FakeSessions;

impl Sessions for FakeSessions {
    fn capabilities(&self) -> Result<Capabilities, SessionError> {
        Ok(Capabilities {
            version: "0.0.0".to_owned(),
            surface: Vec::new(),
        })
    }

    fn launch(&self, _request: &LaunchRequest) -> Result<depot_core::SessionId, SessionError> {
        Ok(depot_core::SessionId::new("s-1"))
    }

    fn resume(&self, _session: &depot_core::SessionId, _prompt: &str) -> Result<(), SessionError> {
        Ok(())
    }

    fn status(&self, _session: &depot_core::SessionId) -> Result<SessionState, SessionError> {
        Ok(SessionState::Running)
    }

    fn wait(
        &self,
        _session: &depot_core::SessionId,
        _timeout: Option<Duration>,
    ) -> Result<TurnOutcome, SessionError> {
        Ok(TurnOutcome::StillRunning)
    }

    fn stop(&self, _session: &depot_core::SessionId) -> Result<(), SessionError> {
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Default)]
struct FakeEvidence {
    stdout: Arc<Mutex<String>>,
    fail: Arc<Mutex<Option<String>>>,
    calls: Arc<Mutex<Vec<(String, String, String)>>>,
}

impl FakeEvidence {
    fn calls(&self) -> Vec<(String, String, String)> {
        self.calls.lock().expect("the call log").clone()
    }
}

impl EvidenceRunner for FakeEvidence {
    fn run(
        &self,
        command: &str,
        worktree: &std::path::Path,
        base: &str,
        _timeout: Duration,
    ) -> depotd::Result<depotd::evidence::EvidenceOutput> {
        self.calls.lock().expect("the call log").push((
            command.to_owned(),
            worktree.display().to_string(),
            base.to_owned(),
        ));
        if let Some(reason) = self.fail.lock().expect("the failure").clone() {
            return Err(depotd::Error::Project(reason));
        }
        Ok(depotd::evidence::EvidenceOutput {
            exit_code: 0,
            stdout: self.stdout.lock().expect("the stdout").clone(),
        })
    }
}

struct Fixture {
    _temp: TempDir,
    store: Store,
    project: Project,
    forge: FakeForge,
}

fn register(config: &str) -> Fixture {
    let temp = TempDir::new().expect("a temporary directory");
    let home = DepotHome::at(temp.path().join("home"));
    home.ensure().expect("the depot home");
    home.write_settings(&Settings {
        profiles: std::collections::BTreeMap::from([(
            "evidence-model".to_owned(),
            depotd::ProfileSettings {
                harness: "pi".to_owned(),
                model: "evidence-model".to_owned(),
                effort: "high".to_owned(),
                account: String::new(),
            },
        )]),
        ..Settings::default()
    })
    .expect("the settings");
    let store = Store::open(&home).expect("the store opens");
    let directory = temp.path().join("acme-widget");
    std::fs::create_dir_all(&directory).expect("the project directory");
    std::fs::write(directory.join(".depot.toml"), config).expect("the project config");
    git(&directory, &["init", "--initial-branch=main"]);
    git(
        &directory,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widget.git",
        ],
    );
    let project = Project {
        id: ProjectId::new(directory.display().to_string()),
        kind: LocationKind::Path,
        slug: "acme-widget".to_owned(),
        created_at: Timestamp::from_millis(0),
    };
    store.put_project(&project).expect("the project is stored");
    Fixture {
        _temp: temp,
        store,
        project,
        forge: FakeForge::start(),
    }
}

fn git(directory: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn open_pull_request_routes(forge: &FakeForge, commit: &str) {
    forge.route(
        "GET",
        &format!("{REPO_PATH}/pulls/7"),
        200,
        &format!(
            "{{\"number\":7,\"html_url\":\"https://github.com/acme/widget/pull/7\",\"state\":\"open\",\"mergeable\":true,\"head\":{{\"sha\":\"{commit}\",\"ref\":\"depot-t-1\"}},\"base\":{{\"sha\":\"ba5eba11\"}}}}"
        ),
    );
    forge.route(
        "GET",
        &format!("{REPO_PATH}/commits/{commit}/check-runs"),
        200,
        "{\"total_count\":0,\"check_runs\":[]}",
    );
}

fn drive_to_pr_open(fixture: &Fixture, commit: &str) {
    let store = &fixture.store;
    let project = &fixture.project;
    for (key, kind) in [
        (
            "proposed:t-1".to_owned(),
            FactKind::TaskProposed {
                task: TaskId::new("t-1"),
                title: "the work".to_owned(),
                intent: "do the work".to_owned(),
                role: depot_core::Role::Build,
                dispatch_profile: None,
                dependencies: Vec::new(),
                base_dependency: None,
                hold_pr: false,
            },
        ),
        (
            "approved:t-1".to_owned(),
            FactKind::TaskApproved {
                task: TaskId::new("t-1"),
            },
        ),
        (
            "acquired:t-1".to_owned(),
            FactKind::WorktreeAcquired {
                task: TaskId::new("t-1"),
                lease: WorktreeLease::new("w1"),
                baseline: depot_core::Baseline::DefaultBranchHead,
                included: Vec::new(),
            },
        ),
        (
            "submitted:t-1".to_owned(),
            FactKind::WorkerSubmitted {
                task: TaskId::new("t-1"),
                commit: depot_core::CommitId::new(commit),
            },
        ),
        (
            "validated:t-1".to_owned(),
            FactKind::ValidationFinished {
                task: TaskId::new("t-1"),
                command: "cargo test".to_owned(),
                commit: depot_core::CommitId::new(commit),
                base_commit: None,
                exit_code: 0,
                duration: Duration::from_secs(1),
                output_tail: String::new(),
            },
        ),
        (
            "pushed:t-1".to_owned(),
            FactKind::BranchPushed {
                task: TaskId::new("t-1"),
                commit: depot_core::CommitId::new(commit),
            },
        ),
        (
            "opened:t-1".to_owned(),
            FactKind::PullRequestOpened {
                task: TaskId::new("t-1"),
                number: 7,
                url: "https://github.com/acme/widget/pull/7".to_owned(),
            },
        ),
    ] {
        store
            .apply_fact(
                project,
                &key,
                &Fact {
                    at: Timestamp::from_millis(0),
                    kind,
                },
            )
            .expect("the fact applies");
    }
    open_pull_request_routes(&fixture.forge, commit);
}

fn request_to(forge: &FakeForge, method: &str, path: &str) -> fake_forge::Recorded {
    forge
        .requests()
        .into_iter()
        .find(|request| request.method == method && request.path == path)
        .unwrap_or_else(|| panic!("no {method} request was made to {path}"))
}

fn daemon(
    fixture: &Fixture,
    evidence: FakeEvidence,
) -> Daemon<
    '_,
    FakeSessions,
    FakeWorktrees,
    ShellValidation,
    ForgeDelivery<depotd::adapters::forge::GitHub>,
    NoEventHook,
> {
    Daemon::new(
        &fixture.store,
        fixture.project.clone(),
        FakeSessions,
        FakeWorktrees,
        ShellValidation,
        ForgeDelivery::new(depotd::adapters::forge::GitHub::new(
            fixture.forge.base_url(),
            "token".to_owned(),
        )),
        NoEventHook,
    )
    .with_evidence_runner(Box::new(evidence))
}

const EVIDENCE_CONFIG: &str = "base_branch = \"main\"\n\n[profiles]\nbuild = \"evidence-model\"\n\n[evidence]\ncommand = \"./capture\"\ntimeout_seconds = 60\n";

#[test]
fn an_open_pull_request_gets_one_evidence_comment_and_a_rerun_does_not_duplicate_it() {
    let fixture = register(EVIDENCE_CONFIG);
    drive_to_pr_open(&fixture, "aaa111");
    fixture.forge.route_query(
        "GET",
        &format!("{REPO_PATH}/issues/7/comments"),
        Some("per_page=100"),
        200,
        "[]",
    );
    fixture.forge.route(
        "POST",
        &format!("{REPO_PATH}/issues/7/comments"),
        201,
        "{\"id\":55,\"body\":\"x\"}",
    );

    let evidence = FakeEvidence {
        stdout: Arc::new(Mutex::new(
            "https://img.example/shot.png\tlogin screen\n".to_owned(),
        )),
        ..FakeEvidence::default()
    };
    let _calls = evidence.calls();
    let daemon = daemon(&fixture, evidence);
    daemon.tick().expect("the first tick runs");

    let posted = request_to(
        &fixture.forge,
        "POST",
        &format!("{REPO_PATH}/issues/7/comments"),
    );
    assert!(posted.body.contains("depot-evidence"), "{}", posted.body);
    assert!(
        posted
            .body
            .contains("![login screen](https://img.example/shot.png)"),
        "{}",
        posted.body
    );
    let task = fixture
        .store
        .task(&fixture.project.id, &TaskId::new("t-1"))
        .expect("the task is read")
        .expect("the task exists");
    assert_eq!(task.state, depot_core::TaskState::PrOpen);

    daemon.tick().expect("the second tick runs");
    assert_eq!(
        fixture
            .forge
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST"
                && request.path == format!("{REPO_PATH}/issues/7/comments"))
            .count(),
        1,
        "the rerun edits nothing and posts nothing"
    );
}

#[test]
fn a_second_commit_edits_the_marked_comment_instead_of_posting_another() {
    let fixture = register(EVIDENCE_CONFIG);
    drive_to_pr_open(&fixture, "aaa111");
    fixture.forge.route_query(
        "GET",
        &format!("{REPO_PATH}/issues/7/comments"),
        Some("per_page=100"),
        200,
        "[{\"id\":9,\"body\":\"older\\n<!-- depot-evidence -->\"}]",
    );
    fixture.forge.route(
        "PATCH",
        &format!("{REPO_PATH}/issues/comments/9"),
        200,
        "{\"id\":9,\"body\":\"x\"}",
    );

    let evidence = FakeEvidence {
        stdout: Arc::new(Mutex::new(
            "https://vid.example/clip.mp4\tthe flow\n".to_owned(),
        )),
        ..FakeEvidence::default()
    };
    daemon(&fixture, evidence).tick().expect("the tick runs");

    let edited = request_to(
        &fixture.forge,
        "PATCH",
        &format!("{REPO_PATH}/issues/comments/9"),
    );
    assert_eq!(edited.method, "PATCH");
    assert!(
        edited
            .body
            .contains("[the flow](https://vid.example/clip.mp4)"),
        "{}",
        edited.body
    );
    assert!(
        !fixture
            .forge
            .requests()
            .iter()
            .any(|request| request.method == "POST"),
        "no second comment is posted"
    );
}

#[test]
fn an_optional_evidence_failure_notes_itself_and_never_blocks_the_pull_request() {
    let fixture = register(EVIDENCE_CONFIG);
    drive_to_pr_open(&fixture, "aaa111");

    let evidence = FakeEvidence {
        fail: Arc::new(Mutex::new(Some("the capture tool crashed".to_owned()))),
        ..FakeEvidence::default()
    };
    let daemon = daemon(&fixture, evidence.clone());
    daemon.tick().expect("the tick runs");

    let task = fixture
        .store
        .task(&fixture.project.id, &TaskId::new("t-1"))
        .expect("the task is read")
        .expect("the task exists");
    assert_eq!(task.state, depot_core::TaskState::PrOpen);
    let events = fixture
        .store
        .events(&fixture.project.id)
        .expect("the events");
    assert!(
        events.iter().any(|event| event.kind == "evidence_failed"
            && event.payload.contains("the capture tool crashed")
            && event.payload.contains("\"required\":false")),
        "{events:?}"
    );
    daemon.tick().expect("the second tick runs");
    assert_eq!(evidence.calls().len(), 1, "the failure is recorded once");
}

#[test]
fn a_required_evidence_failure_holds_the_task_for_the_user() {
    let fixture = register(
        "base_branch = \"main\"\n\n[profiles]\nbuild = \"evidence-model\"\n\n[evidence]\ncommand = \"./capture\"\nrequired = true\n",
    );
    drive_to_pr_open(&fixture, "aaa111");

    let evidence = FakeEvidence {
        fail: Arc::new(Mutex::new(Some("the capture tool crashed".to_owned()))),
        ..FakeEvidence::default()
    };
    daemon(&fixture, evidence).tick().expect("the tick runs");

    let task = fixture
        .store
        .task(&fixture.project.id, &TaskId::new("t-1"))
        .expect("the task is read")
        .expect("the task exists");
    assert_eq!(task.state, depot_core::TaskState::Failed);
    let events = fixture
        .store
        .events(&fixture.project.id)
        .expect("the events");
    assert!(
        events
            .iter()
            .any(|event| event.kind == "evidence_failed"
                && event.payload.contains("\"required\":true")),
        "{events:?}"
    );
}

#[test]
fn an_empty_manifest_is_a_comment_saying_nothing_was_captured() {
    let fixture = register(EVIDENCE_CONFIG);
    drive_to_pr_open(&fixture, "aaa111");
    fixture.forge.route_query(
        "GET",
        &format!("{REPO_PATH}/issues/7/comments"),
        Some("per_page=100"),
        200,
        "[]",
    );
    fixture.forge.route(
        "POST",
        &format!("{REPO_PATH}/issues/7/comments"),
        201,
        "{\"id\":55,\"body\":\"x\"}",
    );

    daemon(&fixture, FakeEvidence::default())
        .tick()
        .expect("the tick runs");

    let posted = request_to(
        &fixture.forge,
        "POST",
        &format!("{REPO_PATH}/issues/7/comments"),
    );
    assert!(posted.body.contains("captured nothing"), "{}", posted.body);
    let task = fixture
        .store
        .task(&fixture.project.id, &TaskId::new("t-1"))
        .expect("the task is read")
        .expect("the task exists");
    assert_eq!(task.state, depot_core::TaskState::PrOpen);
}

#[test]
fn a_project_without_evidence_config_never_touches_the_forge_comments() {
    let fixture = register("base_branch = \"main\"\n\n[profiles]\nbuild = \"evidence-model\"\n");
    drive_to_pr_open(&fixture, "aaa111");

    daemon(&fixture, FakeEvidence::default())
        .tick()
        .expect("the tick runs");

    assert!(
        !fixture
            .forge
            .requests()
            .iter()
            .any(|request| request.path.contains("/issues/")),
        "no comment endpoint is called"
    );
}
