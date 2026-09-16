use std::fs::{File, OpenOptions};

use fs2::FileExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use depot_core::{
    Action, Baseline, CommitId, Fact, FactKind, Liveness, SessionId, Task, TaskId, TaskState,
    WorktreeLease,
};

use crate::adapters::forge::{Forge, NewPullRequest, RepoSlug};
use crate::adapters::profiles::ProfileResolver;
use crate::adapters::sessions::{LaunchRequest, SessionProfile, Sessions};
use crate::adapters::worktrees::{AcquireRequest, Lease, Worktrees};
use crate::clock::now;
use crate::error::{Error, Result};
use crate::home::DepotHome;
use crate::project::{LocationKind, Project};
use crate::store::{EventOutcome, Store, event_key};

pub const DAEMON_LOCK_FILE_NAME: &str = "depotd.lock";

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(home: &DepotHome) -> Result<Self> {
        home.ensure()?;
        let path = home.root().join(DAEMON_LOCK_FILE_NAME);
        let file = OpenOptions::new().write(true).create(true).open(&path)?;
        file.try_lock_exclusive().map_err(|error| {
            Error::Home(format!(
                "another depot daemon already holds {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self { _file: file })
    }
}

pub trait ValidationRunner {
    fn validate(
        &self,
        task: &Task,
        worktree: &Path,
        commit: &CommitId,
        command: &str,
    ) -> Result<ValidationResult>;
}

pub trait Delivery {
    fn push(&self, task: &Task, worktree: &Path, commit: &CommitId) -> Result<()>;
    fn open_pull_request(
        &self,
        task: &Task,
        worktree: &Path,
        commit: &CommitId,
        body: &str,
    ) -> Result<(u64, String)>;
}

pub struct ShellValidation;

impl ValidationRunner for ShellValidation {
    fn validate(
        &self,
        _task: &Task,
        worktree: &Path,
        commit: &CommitId,
        command: &str,
    ) -> Result<ValidationResult> {
        let head = git_output(worktree, &["rev-parse", "HEAD"])?;
        if head.trim() != commit.as_str() {
            return Err(Error::Project(format!(
                "worktree {} is at {} rather than submitted commit {commit}",
                worktree.display(),
                head.trim()
            )));
        }
        let started = Instant::now();
        let output = shell(command, worktree)?;
        Ok(ValidationResult {
            exit_code: output.status.code().unwrap_or(-1),
            duration: started.elapsed(),
            output_tail: output_tail(&output),
        })
    }
}

pub struct ForgeDelivery<F> {
    forge: F,
    base: String,
}

impl<F> ForgeDelivery<F> {
    pub fn new(forge: F, base: impl Into<String>) -> Self {
        Self {
            forge,
            base: base.into(),
        }
    }
}

impl<F: Forge> Delivery for ForgeDelivery<F> {
    fn push(&self, _task: &Task, worktree: &Path, commit: &CommitId) -> Result<()> {
        let head = git_output(worktree, &["rev-parse", "HEAD"])?;
        if head.trim() != commit.as_str() {
            return Err(Error::Project(format!(
                "worktree {} is not at submitted commit {commit}",
                worktree.display()
            )));
        }
        let branch = git_output(worktree, &["branch", "--show-current"])?;
        let branch = branch.trim();
        if branch.is_empty() {
            return Err(Error::Project(format!(
                "worktree {} has no branch for delivery",
                worktree.display()
            )));
        }
        git_output(
            worktree,
            &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
        )?;
        Ok(())
    }

    fn open_pull_request(
        &self,
        task: &Task,
        worktree: &Path,
        _commit: &CommitId,
        body: &str,
    ) -> Result<(u64, String)> {
        let remote = git_output(worktree, &["remote", "get-url", "origin"])?;
        let repo = repo_slug(remote.trim())?;
        let head = git_output(worktree, &["branch", "--show-current"])?;
        let head = head.trim().to_owned();
        let opened = match self
            .forge
            .find_open_pull_request(&repo, &head)
            .map_err(|error| Error::Project(error.to_string()))?
        {
            Some(opened) => opened,
            None => self
                .forge
                .open_pull_request(&NewPullRequest {
                    repo,
                    title: task.title.clone(),
                    body: body.to_owned(),
                    head,
                    base: self.base.clone(),
                })
                .map_err(|error| Error::Project(error.to_string()))?,
        };
        Ok((opened.number, opened.url))
    }
}

pub struct StderrNotifier;

impl Notifier for StderrNotifier {
    fn notify(&self, task: &Task) -> Result<()> {
        eprintln!("depot: task {} is waiting on a question", task.id);
        Ok(())
    }
}

fn shell(command: &str, worktree: &Path) -> Result<std::process::Output> {
    #[cfg(windows)]
    let mut process = {
        let mut process = Command::new("cmd");
        process.args(["/C", command]);
        process
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    };
    process.current_dir(worktree).output().map_err(Error::Io)
}

fn git_output(worktree: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(Error::Io)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(Error::Project(
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    ))
}

fn output_tail(output: &std::process::Output) -> String {
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    let tail = combined.chars().rev().take(4_096).collect::<String>();
    tail.chars().rev().collect()
}

fn repo_slug(remote: &str) -> Result<RepoSlug> {
    let remote = remote.trim_end_matches('/').trim_end_matches(".git");
    let target = remote
        .rsplit_once(':')
        .map(|(_, path)| path)
        .unwrap_or(remote);
    let mut parts = target.rsplit('/');
    let name = parts.next().filter(|part| !part.is_empty());
    let owner = parts.next().filter(|part| !part.is_empty());
    match (owner, name) {
        (Some(owner), Some(name)) => Ok(RepoSlug::new(owner, name)),
        _ => Err(Error::Project(format!(
            "cannot read GitHub repository from {remote}"
        ))),
    }
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
        let at = now();
        self.record(
            &event_key(&[
                "daemon_restarted",
                &std::process::id().to_string(),
                &at.millis().to_string(),
            ]),
            Fact {
                at,
                kind: FactKind::DaemonRestarted,
            },
        )?;
        self.reconcile_sessions()
    }

    pub fn tick(&self) -> Result<()> {
        let at = now();
        self.record(
            &event_key(&["polled", &at.millis().to_string()]),
            Fact {
                at,
                kind: FactKind::Polled,
            },
        )?;
        self.reconcile_sessions()?;
        self.reconcile_validation()?;
        self.reconcile_delivery()
    }

    pub fn worker_submitted(&self, task: TaskId, commit: CommitId) -> Result<()> {
        self.record(
            &event_key(&["worker_submitted", task.as_str(), commit.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::WorkerSubmitted { task, commit },
            },
        )
    }

    pub fn worker_liveness(&self, task: TaskId, liveness: depot_core::Liveness) -> Result<()> {
        let at = now();
        self.record(
            &event_key(&[
                "worker_liveness",
                task.as_str(),
                &format!("{liveness:?}"),
                &at.millis().to_string(),
            ]),
            Fact {
                at,
                kind: FactKind::WorkerLivenessChanged { task, liveness },
            },
        )
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
                prompt: self
                    .store
                    .coordinator_context(&self.project)?
                    .brief(&task_record)?,
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
        let worktree = self.lease_for(&task_record)?.path;
        let command = self.store.project_config(&self.project)?.validation.command;
        let result = self
            .validation
            .validate(&task_record, &worktree, &commit, &command)?;
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
        let task_record = self.task(&task)?;
        let worktree = self.lease_for(&task_record)?.path;
        self.delivery.push(&task_record, &worktree, &commit)?;
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
        let worktree = self.lease_for(&task_record)?.path;
        let body = pull_request_body(&task_record, &commit);
        let (number, url) =
            self.delivery
                .open_pull_request(&task_record, &worktree, &commit, &body)?;
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
        self.notifier.notify(&self.task(&task)?)
    }

    fn reconcile_sessions(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.into_values() {
            if !task.state.in_flight() {
                continue;
            }
            let Some(session) = task
                .attempts
                .last()
                .and_then(|attempt| attempt.session.clone())
            else {
                continue;
            };
            let liveness = match self.sessions.status(&session) {
                Ok(crate::adapters::sessions::SessionState::Running) => Liveness::Live,
                Ok(_) => Liveness::Gone,
                Err(error) => return Err(Error::Project(error.to_string())),
            };
            self.worker_liveness(task.id, liveness)?;
        }
        Ok(())
    }

    fn reconcile_validation(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.into_values() {
            if task.state != TaskState::Validating {
                continue;
            }
            let commit = self.submitted_commit(&task.id)?;
            if task.validations.iter().any(|v| v.commit == commit) {
                continue;
            }
            self.validate(task.id, commit)?;
        }
        Ok(())
    }

    fn submitted_commit(&self, task: &TaskId) -> Result<CommitId> {
        let event = self
            .store
            .events(&self.project.id)?
            .into_iter()
            .rev()
            .find(|event| event.task.as_ref() == Some(task) && event.kind == "worker_submitted")
            .ok_or_else(|| Error::Project(format!("task `{task}` has no submitted commit")))?;
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|error| Error::Schema(error.to_string()))?;
        let commit = payload
            .get("commit")
            .and_then(serde_json::Value::as_str)
            .filter(|commit| !commit.is_empty())
            .ok_or_else(|| {
                Error::Schema(format!("worker submission for task `{task}` has no commit"))
            })?;
        Ok(CommitId::new(commit))
    }

    fn reconcile_delivery(&self) -> Result<()> {
        let state = self.store.project_state(&self.project)?;
        for task in state.tasks.values() {
            if task.state != TaskState::Validated
                || task.pull_request().is_some()
                || depot_core::publication_blocked(&state, &task.id)
            {
                continue;
            }
            let Some(commit) = task.validated_commit().cloned() else {
                continue;
            };
            self.push(task.id.clone(), commit.clone())?;
            self.open_pull_request(task.id.clone(), commit)?;
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::{ShellValidation, ValidationRunner, repo_slug};
    use depot_core::CommitId;
    use tempfile::TempDir;

    fn git(path: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn validation_runs_at_the_submitted_commit_and_keeps_its_output() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path();
        git(path, &["init"]);
        git(path, &["config", "user.email", "depot@example.test"]);
        git(path, &["config", "user.name", "Depot"]);
        std::fs::write(path.join("answer"), "42").expect("fixture is written");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "fixture"]);
        let commit = CommitId::new(git(path, &["rev-parse", "HEAD"]));
        let task = depot_core::Task {
            id: depot_core::TaskId::new("T1"),
            project: depot_core::ProjectId::new("test"),
            title: "test".to_owned(),
            intent: "test validation".to_owned(),
            role: depot_core::Role::Build,
            state: depot_core::TaskState::Proposed,
            dependencies: Vec::new(),
            base_dependency: None,
            attempts: Vec::new(),
            questions: Vec::new(),
            validations: Vec::new(),
            artifacts: Vec::new(),
            links: Vec::new(),
            branch_head: None,
            retry: None,
            created_at: depot_core::Timestamp::from_millis(0),
            updated_at: depot_core::Timestamp::from_millis(0),
        };
        let result = ShellValidation
            .validate(&task, path, &commit, "printf validated; exit 7")
            .expect("validation runs");
        assert_eq!(result.exit_code, 7);
        assert_eq!(result.output_tail, "validated");
        let wrong = CommitId::new("0000000000000000000000000000000000000000");
        assert!(
            ShellValidation
                .validate(&task, path, &wrong, "true")
                .is_err()
        );
    }

    #[test]
    fn reads_https_and_ssh_github_remotes() {
        assert_eq!(
            repo_slug("https://github.com/nunoras/depot.git")
                .expect("https remote")
                .path(),
            "nunoras/depot"
        );
        assert_eq!(
            repo_slug("git@github.com:nunoras/depot.git")
                .expect("ssh remote")
                .path(),
            "nunoras/depot"
        );
    }
}
