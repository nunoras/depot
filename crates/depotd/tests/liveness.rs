use std::path::PathBuf;

use depot_core::{Liveness, ProjectId, SessionId, TaskId, Timestamp};
use depotd::adapters::sessions::{
    Capabilities, LaunchRequest, SessionError, SessionState, SessionSummary, Sessions, TurnOutcome,
};
use depotd::adapters::worktrees::{AcquireRequest, Lease, PoolEntry, WorktreeError, Worktrees};
use depotd::{
    Daemon, DepotHome, ForgeDelivery, LocationKind, NoEventHook, Project, ShellValidation, Store,
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

    fn resume(&self, _session: &SessionId, _prompt: &str) -> Result<(), SessionError> {
        Ok(())
    }

    fn status(&self, _session: &SessionId) -> Result<SessionState, SessionError> {
        Ok(SessionState::Running)
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
