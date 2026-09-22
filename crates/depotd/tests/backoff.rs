mod support;

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use depot_core::{
    Attempt, AttemptOutcome, ProfileId, ProjectId, SessionId, Task, TaskState, Timestamp,
};
use depotd::adapters::sessions::{
    Capabilities, LaunchRequest, SessionError, SessionState, SessionStatus, SessionSummary,
    Sessions, TurnOutcome,
};
use depotd::adapters::worktrees::{AcquireRequest, Lease, PoolEntry, WorktreeError, Worktrees};
use depotd::{
    DepotHome, ForgeDelivery, LocationKind, NoEventHook, Project, ShellValidation, Store,
    Supervisor,
};

#[derive(Clone)]
struct CountingSessions {
    status: Rc<Cell<usize>>,
    stop: Rc<Cell<usize>>,
}

impl Sessions for CountingSessions {
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
        self.status.set(self.status.get() + 1);
        Ok(SessionStatus {
            state: SessionState::Stopped,
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
        self.stop.set(self.stop.get() + 1);
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct CountingWorktrees {
    pool: Rc<Cell<usize>>,
}

impl Worktrees for CountingWorktrees {
    fn acquire(&self, _request: &AcquireRequest) -> Result<Lease, WorktreeError> {
        Err(WorktreeError::MalformedOutput {
            command: "acquire".to_owned(),
            detail: "no worktrees in this test".to_owned(),
        })
    }

    fn release(&self, _lease: &Lease) -> Result<(), WorktreeError> {
        Ok(())
    }

    fn pool(&self, _repo: &Path) -> Result<Vec<PoolEntry>, WorktreeError> {
        self.pool.set(self.pool.get() + 1);
        Ok(Vec::new())
    }

    fn branches(&self, _repo: &Path) -> Result<Vec<String>, WorktreeError> {
        Ok(Vec::new())
    }
}

fn registered_project(home: &DepotHome, name: &str) -> Project {
    let directory = home.root().join(name);
    std::fs::create_dir_all(&directory).expect("the project directory");
    std::fs::write(
        directory.join(depotd::PROJECT_CONFIG_FILE_NAME),
        "base_branch = \"main\"\n",
    )
    .expect("the project config");
    Project {
        id: ProjectId::new(directory.display().to_string()),
        kind: LocationKind::Path,
        slug: name.to_owned(),
        created_at: Timestamp::from_millis(0),
    }
}

fn stopped_task(project: &ProjectId, id: &str, session: &str) -> Task {
    let mut task = support::simple_task(project, id, TaskState::Cancelled, 0);
    task.attempts.push(Attempt {
        session: Some(SessionId::new(session)),
        profile: ProfileId::new("fable-5"),
        worktree: None,
        started_at: Timestamp::from_millis(0),
        finished_at: Some(Timestamp::from_millis(1)),
        outcome: AttemptOutcome::Stopped,
        base_merge: false,
        last_seen_at: None,
    });
    task
}

#[test]
fn an_idle_daemon_polls_a_stopped_session_once_and_never_touches_the_pool() {
    let fixture = support::fixture();
    let store = Store::open(&fixture.home).expect("the store opens");
    let project = registered_project(&fixture.home, "example");
    store.put_project(&project).expect("the project is stored");
    store
        .put_task(&stopped_task(&project.id, "t-1", "s-1"))
        .expect("the stopped task is stored");

    let sessions = CountingSessions {
        status: Rc::new(Cell::new(0)),
        stop: Rc::new(Cell::new(0)),
    };
    let worktrees = CountingWorktrees {
        pool: Rc::new(Cell::new(0)),
    };
    let supervisor = Supervisor::new(
        &store,
        vec![project.clone()],
        sessions.clone(),
        worktrees.clone(),
        ShellValidation,
        ForgeDelivery::new(depotd::adapters::forge::GitHub::new(
            "http://forge.test",
            "token".to_owned(),
        )),
        NoEventHook,
    );

    for turn in 0..10 {
        supervisor.tick(turn).expect("an idle tick stays alive");
    }

    assert!(
        sessions.status.get() <= 1,
        "an idle daemon observed a session already stopped {} times in ten ticks",
        sessions.status.get()
    );
    assert_eq!(
        sessions.stop.get(),
        0,
        "a session already stopped is never asked to stop"
    );
    assert_eq!(
        worktrees.pool.get(),
        0,
        "an idle daemon never reads the worktree pool"
    );
}
