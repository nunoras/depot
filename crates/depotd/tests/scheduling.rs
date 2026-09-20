use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use depot_core::{Fact, FactKind, ProjectId, Role, SessionId, TaskId, Timestamp, WorktreeLease};
use depotd::adapters::sessions::{
    Capabilities, LaunchRequest, SessionError, SessionState, SessionSummary, Sessions, TurnOutcome,
};
use depotd::adapters::worktrees::{AcquireRequest, Lease, PoolEntry, WorktreeError, Worktrees};
use depotd::{
    Daemon, DepotHome, ForgeDelivery, NoEventHook, Project, Settings, ShellValidation, Store,
    Supervisor,
};
use tempfile::TempDir;

const PROFILE: &str = "sched-profile";

#[derive(Clone, Default)]
struct FakeWorktrees {
    acquired: Arc<Mutex<Vec<String>>>,
    leases: Arc<Mutex<BTreeMap<String, WorktreeLease>>>,
}

impl Worktrees for FakeWorktrees {
    fn acquire(&self, request: &AcquireRequest) -> Result<Lease, WorktreeError> {
        let lease = WorktreeLease::new(format!("l-{}", request.holder));
        self.acquired
            .lock()
            .expect("the acquire log")
            .push(request.repo.display().to_string());
        self.leases
            .lock()
            .expect("the lease table")
            .insert(request.repo.display().to_string(), lease.clone());
        Ok(Lease {
            lease,
            path: request.repo.join("worktree"),
            holder: request.holder.clone(),
            acquired_at: String::new(),
        })
    }

    fn release(&self, _lease: &Lease) -> Result<(), WorktreeError> {
        Ok(())
    }

    fn pool(&self, repo: &std::path::Path) -> Result<Vec<PoolEntry>, WorktreeError> {
        let leases = self.leases.lock().expect("the lease table");
        Ok(match leases.get(repo.display().to_string().as_str()) {
            Some(lease) => vec![PoolEntry {
                name: "1".to_owned(),
                path: repo.join("worktree"),
                state: "leased".to_owned(),
                lease: Some(lease.clone()),
                holder: Some(format!("depot:{}", lease.as_str())),
            }],
            None => Vec::new(),
        })
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

struct Fixture {
    _temp: TempDir,
    store: Store,
    projects: Vec<Project>,
    worktrees: FakeWorktrees,
}

fn project_directory(home: &DepotHome, name: &str) -> PathBuf {
    home.root().join(name)
}

fn register_project(store: &Store, home: &DepotHome, name: &str, config: &str) -> Project {
    let directory = project_directory(home, name);
    std::fs::create_dir_all(&directory).expect("the project directory");
    std::fs::write(directory.join(".depot.toml"), config).expect("the project config");
    let project = Project {
        id: ProjectId::new(directory.display().to_string()),
        kind: depotd::LocationKind::Path,
        slug: name.to_owned(),
        created_at: Timestamp::from_millis(0),
    };
    store.put_project(&project).expect("the project is stored");
    project
}

fn propose_and_approve(store: &Store, project: &Project, task: &str) {
    for (key, kind) in [
        (
            format!("proposed:{task}"),
            FactKind::TaskProposed {
                task: TaskId::new(task),
                title: format!("task {task}"),
                intent: "schedule the work".to_owned(),
                role: Role::Build,
                dispatch_profile: None,
                dependencies: Vec::new(),
                base_dependency: None,
                hold_pr: false,
            },
        ),
        (
            format!("approved:{task}"),
            FactKind::TaskApproved {
                task: TaskId::new(task),
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
}

fn fixture(concurrency: usize) -> Fixture {
    let temp = TempDir::new().expect("a temporary directory");
    let home = DepotHome::at(temp.path().join("home"));
    home.ensure().expect("the depot home");
    home.write_settings(&Settings {
        concurrency,
        profiles: BTreeMap::from([(
            PROFILE.to_string(),
            depotd::ProfileSettings {
                harness: "pi".to_string(),
                model: "sched-model".to_string(),
                effort: "high".to_string(),
                account: String::new(),
            },
        )]),
        ..Settings::default()
    })
    .expect("the settings");
    let store = Store::open(&home).expect("the store opens");
    let config = format!("base_branch = \"main\"\n\n[profiles]\nbuild = \"{PROFILE}\"\n");
    let a = register_project(&store, &home, "alpha", &config);
    let b = register_project(&store, &home, "beta", &config);
    Fixture {
        _temp: temp,
        store,
        projects: vec![a, b],
        worktrees: FakeWorktrees::default(),
    }
}

fn supervisor(
    fixture: &Fixture,
) -> Supervisor<
    '_,
    FakeSessions,
    FakeWorktrees,
    ShellValidation,
    ForgeDelivery<depotd::adapters::forge::GitHub>,
    NoEventHook,
> {
    Supervisor::new(
        &fixture.store,
        fixture.projects.clone(),
        FakeSessions,
        fixture.worktrees.clone(),
        ShellValidation,
        ForgeDelivery::new(depotd::adapters::forge::GitHub::new(
            "http://forge.test",
            "token".to_owned(),
        )),
        NoEventHook,
    )
}

fn acquisitions(fixture: &Fixture) -> Vec<String> {
    fixture
        .worktrees
        .acquired
        .lock()
        .expect("the acquire log")
        .clone()
}

#[test]
fn the_global_cap_spans_projects_and_the_per_project_cap_queues_within_one() {
    let fixture = fixture(4);
    let a = &fixture.projects[0];
    let b = &fixture.projects[1];
    propose_and_approve(&fixture.store, a, "t-1");
    propose_and_approve(&fixture.store, a, "t-2");
    propose_and_approve(&fixture.store, b, "t-1");

    supervisor(&fixture).tick(0).expect("the tick runs");

    let acquired = acquisitions(&fixture);
    assert_eq!(acquired.len(), 2, "both projects take a slot: {acquired:?}");
    assert!(
        acquired.contains(&a.id.as_str().to_string())
            && acquired.contains(&b.id.as_str().to_string()),
        "each project's first task runs: {acquired:?}"
    );
    let second = fixture
        .store
        .task(&a.id, &TaskId::new("t-2"))
        .expect("the task is read")
        .expect("the task exists");
    assert_eq!(
        second.state,
        depot_core::TaskState::Approved,
        "the per-project cap of one queues the second task"
    );
}

#[test]
fn a_global_cap_of_one_gives_the_queued_project_the_next_free_slot() {
    let fixture = fixture(1);
    let a = &fixture.projects[0];
    let b = &fixture.projects[1];
    propose_and_approve(&fixture.store, a, "t-1");
    propose_and_approve(&fixture.store, b, "t-1");

    supervisor(&fixture).tick(0).expect("the first tick runs");
    assert_eq!(acquisitions(&fixture).len(), 1, "only one slot is taken");

    fixture
        .store
        .apply_fact(
            a,
            "cancelled:a-t-1",
            &Fact {
                at: Timestamp::from_millis(1),
                kind: FactKind::TaskCancelled {
                    task: TaskId::new("t-1"),
                },
            },
        )
        .expect("the cancel applies");

    supervisor(&fixture).tick(1).expect("the second tick runs");

    let acquired = acquisitions(&fixture);
    assert_eq!(
        acquired.len(),
        2,
        "the freed slot goes to beta: {acquired:?}"
    );
    assert_eq!(
        acquired[1],
        b.id.as_str(),
        "round-robin rotation hands the turn to beta"
    );
}

#[test]
fn a_project_filter_debug_run_still_serves_one_project() {
    let fixture = fixture(1);
    let a = &fixture.projects[0];
    propose_and_approve(&fixture.store, a, "t-1");

    let daemon: Daemon<
        '_,
        FakeSessions,
        FakeWorktrees,
        ShellValidation,
        ForgeDelivery<depotd::adapters::forge::GitHub>,
        NoEventHook,
    > = Daemon::new(
        &fixture.store,
        a.clone(),
        FakeSessions,
        fixture.worktrees.clone(),
        ShellValidation,
        ForgeDelivery::new(depotd::adapters::forge::GitHub::new(
            "http://forge.test",
            "token".to_owned(),
        )),
        NoEventHook,
    );
    daemon.tick().expect("the filtered daemon runs");
    assert_eq!(acquisitions(&fixture).len(), 1);
}
