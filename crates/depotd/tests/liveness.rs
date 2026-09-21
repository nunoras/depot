mod support;

use std::path::PathBuf;

use depot_core::{
    Attempt, AttemptOutcome, Fact, FactKind, Liveness, ProfileId, ProjectId, SessionId, Task,
    TaskId, TaskState, Timestamp,
};
use depotd::adapters::sessions::{
    Capabilities, LaunchRequest, SessionError, SessionState, SessionStatus, SessionSummary,
    Sessions, TurnOutcome,
};
use depotd::adapters::worktrees::{AcquireRequest, Lease, PoolEntry, WorktreeError, Worktrees};
use depotd::{
    Daemon, DepotHome, ForgeDelivery, LocationKind, NoEventHook, Project, ShellValidation, Store,
    render_checklist_observed,
};
use tempfile::TempDir;

struct LiveSessions;

impl Sessions for LiveSessions {
    fn capabilities(&self) -> Result<Capabilities, SessionError> {
        Ok(Capabilities {
            version: "0.0.0".to_owned(),
            surface: Vec::new(),
        })
    }

    fn launch(&self, _request: &LaunchRequest) -> Result<SessionId, SessionError> {
        Ok(SessionId::new("s-1"))
    }

    fn resume(&self, _session: &SessionId, _prompt: &str) -> Result<SessionId, SessionError> {
        Ok(SessionId::new("s-2"))
    }

    fn status(&self, _session: &SessionId) -> Result<SessionStatus, SessionError> {
        Ok(SessionStatus {
            state: SessionState::Running,
            error: None,
            capture_error: None,
            limit_hit: false,
            started: None,
            last_activity: None,
            current_tool: None,
        })
    }

    fn wait(
        &self,
        _session: &SessionId,
        _timeout: Option<std::time::Duration>,
    ) -> Result<TurnOutcome, SessionError> {
        Ok(TurnOutcome::StillRunning)
    }

    fn stop(&self, _session: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        Ok(Vec::new())
    }
}

struct NoWorktrees;

impl Worktrees for NoWorktrees {
    fn acquire(&self, _request: &AcquireRequest) -> Result<Lease, WorktreeError> {
        Err(WorktreeError::MalformedOutput {
            command: "acquire".to_owned(),
            detail: "no worktrees in test".to_owned(),
        })
    }

    fn release(&self, _lease: &Lease) -> Result<(), WorktreeError> {
        Ok(())
    }

    fn pool(&self, _repo: &std::path::Path) -> Result<Vec<PoolEntry>, WorktreeError> {
        Ok(Vec::new())
    }

    fn branches(&self, _repo: &std::path::Path) -> Result<Vec<String>, WorktreeError> {
        Ok(Vec::new())
    }
}

const LIVENESS: &str = "worker_liveness_changed";

fn liveness_events(store: &Store, project: &ProjectId) -> Vec<String> {
    store
        .events(project)
        .expect("the events are read")
        .into_iter()
        .filter(|event| event.kind == LIVENESS)
        .map(|event| event.payload)
        .collect()
}

#[test]
fn repeated_identical_polls_record_one_liveness_fact_and_a_change_records_another() {
    let temp = TempDir::new().expect("a temporary directory");
    let home = DepotHome::at(temp.path().join("home"));
    home.ensure().expect("the depot home");
    let store = Store::open(&home).expect("the store opens");
    let directory = project_directory(&home, "example");
    std::fs::create_dir_all(&directory).expect("the project directory");
    std::fs::write(directory.join(".depot.toml"), "base_branch = \"main\"\n").expect("the config");
    let project = Project {
        id: ProjectId::new(directory.display().to_string()),
        kind: LocationKind::Path,
        slug: "example".to_owned(),
        created_at: Timestamp::from_millis(0),
    };
    store.put_project(&project).expect("the project is stored");

    let daemon: Daemon<'_, LiveSessions, NoWorktrees, ShellValidation, _, NoEventHook> =
        Daemon::new(
            &store,
            project.clone(),
            LiveSessions,
            NoWorktrees,
            ShellValidation,
            ForgeDelivery::new(depotd::adapters::forge::GitHub::new(
                "http://forge.test",
                "token".to_owned(),
            )),
            NoEventHook,
        );

    for _ in 0..5 {
        daemon
            .worker_liveness(TaskId::new("t-1"), Liveness::Live)
            .expect("the liveness fact is judged");
    }
    let events = liveness_events(&store, &project.id);
    assert_eq!(events.len(), 1, "five identical polls record one fact");
    assert!(events[0].contains("live"), "the fact carries liveness");

    daemon
        .worker_liveness(TaskId::new("t-1"), Liveness::Gone)
        .expect("the changed fact records");
    let events = liveness_events(&store, &project.id);
    assert_eq!(events.len(), 2, "a changed value records a second fact");
    assert!(events[1].contains("gone"));

    daemon
        .worker_liveness(TaskId::new("t-1"), Liveness::Live)
        .expect("the changed fact records");
    assert_eq!(
        liveness_events(&store, &project.id).len(),
        3,
        "each change records exactly once"
    );
}

fn project_directory(home: &DepotHome, name: &str) -> PathBuf {
    home.root().join(name)
}

fn running_task(project: &ProjectId, id: &str, session: &str, started_at: u64) -> Task {
    let mut task = support::simple_task(project, id, TaskState::Running, started_at);
    task.attempts.push(Attempt {
        session: Some(SessionId::new(session)),
        profile: ProfileId::new("fable-5"),
        worktree: None,
        started_at: Timestamp::from_millis(started_at),
        finished_at: None,
        outcome: AttemptOutcome::InFlight,
        rebase: false,
        last_seen_at: None,
    });
    task
}

fn observe_daemon<'a>(
    store: &'a Store,
    project: &Project,
) -> Daemon<
    'a,
    LiveSessions,
    NoWorktrees,
    ShellValidation,
    ForgeDelivery<depotd::adapters::forge::GitHub>,
    NoEventHook,
> {
    Daemon::new(
        store,
        project.clone(),
        LiveSessions,
        NoWorktrees,
        ShellValidation,
        ForgeDelivery::new(depotd::adapters::forge::GitHub::new(
            "http://forge.test",
            "token".to_owned(),
        )),
        NoEventHook,
    )
}

fn seeded_liveness(store: &Store, project: &Project, task: &TaskId, at: u64, liveness: Liveness) {
    store
        .apply_fact(
            project,
            &format!("seed-liveness-{at}"),
            &Fact {
                at: Timestamp::from_millis(at),
                kind: FactKind::WorkerLivenessChanged {
                    task: task.clone(),
                    liveness,
                },
            },
        )
        .expect("the seeded liveness fact is recorded");
}

#[test]
fn a_stale_liveness_fact_is_refreshed_but_a_fresh_one_is_left_alone() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let project = added.project.clone();
    let task = running_task(&project.id, "t-1", "s-1", 1_000);
    store.put_task(&task).expect("stored");
    let daemon = observe_daemon(&store, &project);

    seeded_liveness(&store, &project, &task.id, 1, Liveness::Live);
    assert_eq!(liveness_events(&store, &project.id).len(), 1);

    daemon
        .worker_liveness(task.id.clone(), Liveness::Live)
        .expect("a stale identical value refreshes");
    assert_eq!(
        liveness_events(&store, &project.id).len(),
        2,
        "a liveness fact older than the 60 second floor is recorded again"
    );

    daemon
        .worker_liveness(task.id.clone(), Liveness::Live)
        .expect("a fresh identical value is throttled");
    assert_eq!(
        liveness_events(&store, &project.id).len(),
        2,
        "an unchanged value inside 60 seconds records nothing"
    );

    daemon
        .worker_liveness(task.id.clone(), Liveness::Gone)
        .expect("a changed value always records");
    assert_eq!(liveness_events(&store, &project.id).len(), 3);
}

fn real_now() -> Timestamp {
    Timestamp::from_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_millis() as u64,
    )
}

#[test]
fn a_healthy_session_is_never_unobserved_across_ten_minutes_of_polls() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let project = added.project.clone();
    let task = running_task(&project.id, "t-1", "s-1", 0);
    store.put_task(&task).expect("stored");
    let daemon = observe_daemon(&store, &project);
    let now = real_now();

    for minute in 0..=10u64 {
        let last_seen = Timestamp::from_millis(now.millis().saturating_sub(minute * 60_000));
        seeded_liveness(
            &store,
            &project,
            &task.id,
            last_seen.millis(),
            Liveness::Live,
        );
        daemon
            .worker_liveness(task.id.clone(), Liveness::Live)
            .expect("the daemon refreshes a stale observation");
        let state = store.project_state(&project).expect("state");
        let rendered = render_checklist_observed(&state, false, now);
        assert!(
            !rendered.contains("unobserved"),
            "minute {minute} read as unobserved:\n{rendered}"
        );
    }

    let state = store.project_state(&project).expect("state");
    let stale = render_checklist_observed(
        &state,
        false,
        Timestamp::from_millis(now.millis() + 6 * 60_000),
    );
    assert!(
        stale.contains("unobserved"),
        "a six minute gap in observations is unobserved:\n{stale}"
    );
}
