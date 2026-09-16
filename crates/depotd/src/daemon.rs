use std::time::Duration;

use depot_core::{
    Action, Baseline, CommitId, Fact, FactKind, SessionId, Task, TaskId, WorktreeLease,
};

use crate::adapters::profiles::ProfileResolver;
use crate::adapters::sessions::{LaunchRequest, SessionProfile, Sessions};
use crate::adapters::worktrees::{AcquireRequest, Lease, Worktrees};
use crate::clock::now;
use crate::error::{Error, Result};
use crate::project::{LocationKind, Project};
use crate::store::{EventOutcome, Store, event_key};

pub trait ValidationRunner {
    fn validate(&self, task: &Task, commit: &CommitId, command: &str) -> Result<ValidationResult>;
}

pub trait Delivery {
    fn push(&self, task: &Task, commit: &CommitId) -> Result<()>;
    fn open_pull_request(
        &self,
        task: &Task,
        commit: &CommitId,
        body: &str,
    ) -> Result<(u64, String)>;
}

pub trait Notifier {
    fn notify(&self, task: &Task) -> Result<()>;
}

pub struct Daemon<'a, S, W, V, D, N> {
    store: &'a Store,
    project: Project,
    sessions: S,
    worktrees: W,
    validation: V,
    delivery: D,
    notifier: N,
}

pub struct ValidationResult {
    pub exit_code: i32,
    pub duration: Duration,
    pub output_tail: String,
}

impl<'a, S, W, V, D, N> Daemon<'a, S, W, V, D, N>
where
    S: Sessions,
    W: Worktrees,
    V: ValidationRunner,
    D: Delivery,
    N: Notifier,
{
    pub fn new(
        store: &'a Store,
        project: Project,
        sessions: S,
        worktrees: W,
        validation: V,
        delivery: D,
        notifier: N,
    ) -> Self {
        Self {
            store,
            project,
            sessions,
            worktrees,
            validation,
            delivery,
            notifier,
        }
    }

    pub fn recover(&self) -> Result<()> {
        let fact = Fact {
            at: now(),
            kind: FactKind::DaemonRestarted,
        };
        self.record("daemon_restarted", fact)
    }

    pub fn record(&self, key: &str, fact: Fact) -> Result<()> {
        log("fact", key);
        let applied = self.store.apply_fact(&self.project, key, &fact)?;
        if applied.outcome == EventOutcome::Recorded {
            for action in applied.actions {
                self.execute(action)?;
            }
        }
        Ok(())
    }

    fn execute(&self, action: Action) -> Result<()> {
        log("action", &format!("{action:?}"));
        match action {
            Action::AcquireWorktree { task, baseline } => self.acquire(task, baseline),
            Action::LaunchSession { task, profile } => self.launch(task, profile),
            Action::ResumeSession { task } => self.resume(task),
            Action::StopSession { task } => self.stop(task),
            Action::RunValidation { task, commit } => self.validate(task, commit),
            Action::Push { task, commit } => self.push(task, commit),
            Action::OpenPullRequest { task, commit } => self.open_pull_request(task, commit),
            Action::ReleaseWorktree { task, lease } => self.release(task, lease),
            Action::Notify { task } => self.notify(task),
            Action::RenderChecklist | Action::Queue { .. } | Action::HoldForUser { .. } => Ok(()),
            Action::RotateCoordinator { .. } => Ok(()),
        }
    }

    fn acquire(&self, task: TaskId, baseline: Baseline) -> Result<()> {
        let repo = self.repository()?;
        let lease = self
            .worktrees
            .acquire(&AcquireRequest {
                repo,
                holder: format!("depot:{}", task.as_str()),
                baseline: baseline.clone(),
            })
            .map_err(|error| Error::Project(error.to_string()))?;
        self.record(
            &event_key(&["worktree_acquired", task.as_str(), lease.lease.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::WorktreeAcquired {
                    task,
                    lease: lease.lease,
                    baseline,
                    included: Vec::new(),
                },
            },
        )
    }

    fn launch(&self, task: TaskId, profile: depot_core::ProfileId) -> Result<()> {
        let task_record = self.task(&task)?;
        let worktree = self.lease_for(&task_record)?;
        let config = self.store.project_config(&self.project)?;
        let settings = self.store.home().load_settings()?;
        let profiles = settings.configured_profiles(&config)?;
        let resolved = profiles
            .resolve(task_record.role)
            .map_err(|error| Error::Config(error.to_string()))?;
        let spec = if resolved.primary.profile == profile {
            resolved.primary
        } else {
            resolved
                .fallbacks
                .into_iter()
                .find(|candidate| candidate.profile == profile)
                .ok_or_else(|| {
                    Error::Config(format!(
                        "profile `{profile}` is not configured for task `{task}`"
                    ))
                })?
        };
        let session = self
            .sessions
            .launch(&LaunchRequest {
                directory: worktree.path,
                profile: SessionProfile {
                    account: spec.account.into(),
                    harness: spec.harness,
                    model: spec.model,
                    effort: spec.effort,
                },
                kind: Some("worker".to_string()),
                prompt: task_record.intent,
            })
            .map_err(|error| Error::Project(error.to_string()))?;
        self.record(
            &event_key(&["worker_turn_started", task.as_str(), session.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::WorkerTurnStarted { task, session },
            },
        )
    }

    fn resume(&self, task: TaskId) -> Result<()> {
        let session = self.session_for(&self.task(&task)?)?;
        self.sessions
            .resume(&session, "An answer was recorded. Continue the task.")
            .map_err(|error| Error::Project(error.to_string()))
    }

    fn stop(&self, task: TaskId) -> Result<()> {
        let session = self.session_for(&self.task(&task)?)?;
        self.sessions
            .stop(&session)
            .map_err(|error| Error::Project(error.to_string()))
    }

    fn validate(&self, task: TaskId, commit: CommitId) -> Result<()> {
        let task_record = self.task(&task)?;
        let command = self.store.project_config(&self.project)?.validation.command;
        let result = self.validation.validate(&task_record, &commit, &command)?;
        self.record(
            &event_key(&["validation_finished", task.as_str(), commit.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::ValidationFinished {
                    task,
                    command,
                    commit,
                    exit_code: result.exit_code,
                    duration: result.duration,
                    output_tail: result.output_tail,
                },
            },
        )
    }

    fn push(&self, task: TaskId, commit: CommitId) -> Result<()> {
        self.delivery.push(&self.task(&task)?, &commit)?;
        self.record(
            &event_key(&["branch_pushed", task.as_str(), commit.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::BranchPushed { task, commit },
            },
        )
    }

    fn open_pull_request(&self, task: TaskId, commit: CommitId) -> Result<()> {
        let task_record = self.task(&task)?;
        let body = pull_request_body(&task_record, &commit);
        let (number, url) = self
            .delivery
            .open_pull_request(&task_record, &commit, &body)?;
        self.record(
            &event_key(&["pull_request_opened", task.as_str(), &number.to_string()]),
            Fact {
                at: now(),
                kind: FactKind::PullRequestOpened { task, number, url },
            },
        )
    }

    fn release(&self, _task: TaskId, lease: WorktreeLease) -> Result<()> {
        let repo = self.repository()?;
        let lease = self
            .worktrees
            .pool(&repo)
            .map_err(|error| Error::Project(error.to_string()))?
            .into_iter()
            .find(|entry| entry.lease.as_ref() == Some(&lease))
            .ok_or_else(|| {
                Error::Project("the worktree lease is no longer present in the pool".to_string())
            })?;
        self.worktrees
            .release(&Lease {
                lease: lease.lease.expect("matched lease"),
                path: lease.path,
                holder: lease.holder.unwrap_or_default(),
                acquired_at: String::new(),
            })
            .map_err(|error| Error::Project(error.to_string()))
    }

    fn notify(&self, task: TaskId) -> Result<()> {
        let task = self.task(&task)?;
        eprintln!("depot: task {} is waiting on a question", task.id);
        self.notifier.notify(&task)
    }

    fn task(&self, id: &TaskId) -> Result<Task> {
        self.store
            .task(&self.project.id, id)?
            .ok_or_else(|| Error::NotFound(format!("task `{id}` does not exist")))
    }

    fn lease_for(&self, task: &Task) -> Result<Lease> {
        let lease = task
            .attempts
            .last()
            .and_then(|attempt| attempt.worktree.as_ref())
            .ok_or_else(|| Error::Project(format!("task `{}` has no worktree lease", task.id)))?;
        let repo = self.repository()?;
        self.worktrees
            .pool(&repo)
            .map_err(|error| Error::Project(error.to_string()))?
            .into_iter()
            .find(|entry| entry.lease.as_ref() == Some(lease))
            .map(|entry| Lease {
                lease: lease.clone(),
                path: entry.path,
                holder: entry.holder.unwrap_or_default(),
                acquired_at: String::new(),
            })
            .ok_or_else(|| {
                Error::Project(format!(
                    "worktree lease `{lease}` is not present in the pool"
                ))
            })
    }

    fn session_for(&self, task: &Task) -> Result<SessionId> {
        task.attempts
            .last()
            .and_then(|attempt| attempt.session.clone())
            .ok_or_else(|| Error::Project(format!("task `{}` has no worker session", task.id)))
    }

    fn repository(&self) -> Result<std::path::PathBuf> {
        match self.project.kind {
            LocationKind::Path => Ok(self.project.id.as_str().into()),
            LocationKind::Url => Err(Error::Project(
                "a URL project has no local repository to run".to_string(),
            )),
        }
    }
}

pub fn pull_request_body(task: &Task, commit: &CommitId) -> String {
    let validation = task
        .validations
        .iter()
        .rev()
        .find(|record| &record.commit == commit);
    let result = validation
        .map(|record| {
            format!(
                "`{}` at `{}` exited {}",
                record.command, record.commit, record.exit_code
            )
        })
        .unwrap_or_else(|| "no validation record".to_string());
    format!(
        "## Intent\n\n{}\n\n## What changed\n\n{}\n\n## Validation\n\n{}\n",
        task.intent, task.title, result
    )
}

fn log(kind: &str, value: &str) {
    eprintln!("{{\"kind\":\"{}\",\"value\":{:?}}}", kind, value);
}
