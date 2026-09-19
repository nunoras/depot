use std::fs::{File, OpenOptions};

use fs2::FileExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use depot_core::{
    Action, Baseline, Checks, CommitId, Fact, FactKind, Liveness, Role, SessionId, Task, TaskId,
    TaskState, WorktreeLease,
};

use crate::adapters::forge::{Forge, NewPullRequest, PrState, RepoSlug};
use crate::adapters::profiles::ProfileResolver;
use crate::adapters::sessions::{LaunchRequest, SessionProfile, Sessions};
use crate::adapters::worktrees::{AcquireRequest, Lease, Worktrees};
use crate::clock::now;
use crate::error::{Error, Result};
use crate::factcodec::payload_field;
use crate::home::DepotHome;
use crate::project::{LocationKind, Project};
use crate::store::{EventOutcome, Store, event_key};
use crate::vocabulary::{FactTag, checks_name, fact_tag_name};

pub const DAEMON_LOCK_FILE_NAME: &str = "depotd.lock";
const MAX_RESUME_ATTEMPTS: u32 = 3;

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(home: &DepotHome) -> Result<Self> {
        home.ensure()?;
        let path = home.root().join(DAEMON_LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
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
    fn observe_pull_request(
        &self,
        task: &Task,
        repo: &RepoSlug,
    ) -> Result<Option<ObservedPullRequest>>;
    fn merge_pull_request(
        &self,
        task: &Task,
        repo: &RepoSlug,
        number: u64,
        head: &CommitId,
    ) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedPullRequest {
    pub state: PrState,
    pub checks: Checks,
    pub commit: CommitId,
    pub base: CommitId,
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
    fn push(&self, task: &Task, worktree: &Path, commit: &CommitId) -> Result<()> {
        let head = git_output(worktree, &["rev-parse", "HEAD"])?;
        if head.trim() != commit.as_str() {
            return Err(Error::Project(format!(
                "worktree {} is not at submitted commit {commit}",
                worktree.display()
            )));
        }
        let branch = delivery_branch(worktree, task)?;
        let mut args = vec!["push".to_owned(), "origin".to_owned()];
        if task.role == Role::Fix {
            args.push("--force-with-lease".to_owned());
        }
        args.push(format!("HEAD:refs/heads/{branch}"));
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        git_output(worktree, &arg_refs)?;
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
        let head = delivery_branch(worktree, task)?;
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

    fn observe_pull_request(
        &self,
        task: &Task,
        repo: &RepoSlug,
    ) -> Result<Option<ObservedPullRequest>> {
        let Some((number, _, _)) = task.pull_request() else {
            return Ok(None);
        };
        let observed = self
            .forge
            .pull_request(repo, number)
            .map_err(|error| Error::Project(error.to_string()))?;
        Ok(Some(ObservedPullRequest {
            state: observed.state,
            checks: observed.checks,
            commit: observed.head,
            base: observed.base,
        }))
    }

    fn merge_pull_request(
        &self,
        _task: &Task,
        repo: &RepoSlug,
        number: u64,
        head: &CommitId,
    ) -> Result<()> {
        self.forge
            .merge_pull_request(repo, number, head)
            .map_err(|error| Error::Project(error.to_string()))
    }
}

impl EventHook for Box<dyn EventHook> {
    fn notify(&self, notice: &EventNotice) -> Result<()> {
        self.as_ref().notify(notice)
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

fn delivery_branch(worktree: &Path, task: &Task) -> Result<String> {
    let current = git_output(worktree, &["branch", "--show-current"])?;
    let current = current.trim();
    if !current.is_empty() {
        return Ok(current.to_owned());
    }
    let branch = format!("depot-{}", task.id);
    git_output(worktree, &["checkout", "-B", &branch])?;
    Ok(branch)
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

pub struct EventNotice {
    pub project: String,
    pub task: TaskId,
    pub title: String,
    pub event: &'static str,
    pub question: Option<String>,
    pub pull_request: Option<String>,
}

impl EventNotice {
    pub fn payload(&self) -> String {
        serde_json::json!({
            "project": self.project,
            "task": self.task.as_str(),
            "title": self.title,
            "event": self.event,
            "question": self.question,
            "recommended_default": Option::<String>::None,
            "pull_request": self.pull_request,
        })
        .to_string()
    }
}

pub trait EventHook {
    fn notify(&self, notice: &EventNotice) -> Result<()>;
}

pub struct ShellEventHook {
    command: String,
}

impl ShellEventHook {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }
}

impl EventHook for ShellEventHook {
    fn notify(&self, notice: &EventNotice) -> Result<()> {
        #[cfg(windows)]
        let mut process = {
            let mut process = Command::new("cmd");
            process.args(["/C", &self.command]);
            process
        };
        #[cfg(not(windows))]
        let mut process = {
            let mut process = Command::new("sh");
            process.args(["-c", &self.command]);
            process
        };
        let mut child = process
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(Error::Io)?;
        use std::io::Write;
        child
            .stdin
            .take()
            .ok_or_else(|| Error::Project("the on_event command closed its stdin".to_string()))?
            .write_all(notice.payload().as_bytes())
            .map_err(Error::Io)?;
        let status = child.wait().map_err(Error::Io)?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Project(format!(
                "the on_event command exited {}",
                status.code().unwrap_or(-1)
            )))
        }
    }
}

pub struct NoEventHook;

impl EventHook for NoEventHook {
    fn notify(&self, _notice: &EventNotice) -> Result<()> {
        Ok(())
    }
}

pub struct Daemon<'a, S, W, V, D, H> {
    store: &'a Store,
    project: Project,
    sessions: S,
    worktrees: W,
    validation: V,
    delivery: D,
    hook: H,
}

pub struct ValidationResult {
    pub exit_code: i32,
    pub duration: Duration,
    pub output_tail: String,
}

impl<'a, S, W, V, D, H> Daemon<'a, S, W, V, D, H>
where
    S: Sessions,
    W: Worktrees,
    V: ValidationRunner,
    D: Delivery,
    H: EventHook,
{
    pub fn new(
        store: &'a Store,
        project: Project,
        sessions: S,
        worktrees: W,
        validation: V,
        delivery: D,
        hook: H,
    ) -> Self {
        Self {
            store,
            project,
            sessions,
            worktrees,
            validation,
            delivery,
            hook,
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
        self.reconcile_start()?;
        self.reconcile_resume()?;
        self.reconcile_sessions()?;
        self.reconcile_notify()
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
        self.reconcile_start()?;
        self.reconcile_resume()?;
        self.reconcile_sessions()?;
        self.reconcile_validation()?;
        self.reconcile_delivery()?;
        self.reconcile_forge()?;
        self.reconcile_notify()
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
            Action::Notify { .. } => Ok(()),
            Action::RenderChecklist | Action::Queue { .. } | Action::HoldForUser { .. } => Ok(()),
            Action::RotateCoordinator { .. } => Ok(()),
        }
    }

    fn acquire(&self, task: TaskId, baseline: Baseline) -> Result<()> {
        if self.acquire_is_pending(&task)?
            && let Some(lease) = self.leased_worktree(&task)?
        {
            return self.record_worktree_acquired(&task, lease, baseline);
        }
        let attempt = self.task(&task)?.attempts.len();
        if attempt == 0 {
            return Err(Error::Project(format!("task `{task}` has no attempt")));
        }
        self.record(
            &event_key(&[
                "worktree_acquire_requested",
                task.as_str(),
                &attempt.to_string(),
            ]),
            Fact {
                at: now(),
                kind: FactKind::WorktreeAcquireRequested { task: task.clone() },
            },
        )?;
        let repo = self.repository()?;
        let lease = self
            .worktrees
            .acquire(&AcquireRequest {
                repo,
                holder: format!("depot:{}", task.as_str()),
                baseline: baseline.clone(),
            })
            .map_err(|error| Error::Project(error.to_string()))?;
        self.record_worktree_acquired(&task, lease.lease, baseline)
    }

    fn record_worktree_acquired(
        &self,
        task: &TaskId,
        lease: WorktreeLease,
        baseline: Baseline,
    ) -> Result<()> {
        self.record(
            &event_key(&["worktree_acquired", task.as_str(), lease.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::WorktreeAcquired {
                    task: task.clone(),
                    lease,
                    baseline,
                    included: Vec::new(),
                },
            },
        )
    }

    fn launch(&self, task: TaskId, profile: depot_core::ProfileId) -> Result<()> {
        if self.launch_is_pending(&task)? {
            return self.surface_unresolved_turn(&task);
        }
        let task_record = self.task(&task)?;
        let worktree = self.lease_for(&task_record)?;
        let attempt = task_record.attempts.len();
        if attempt == 0 {
            return Err(Error::Project(format!("task `{task}` has no attempt")));
        }
        let config = self.store.project_config(&self.project)?;
        let settings = self.store.home().load_settings()?;
        let spec = if let Some(pinned) = &task_record.dispatch_profile {
            if &profile != pinned && !settings.profile_fallbacks().contains(&profile) {
                return Err(Error::Config(format!(
                    "profile `{profile}` is not authorized for task `{task}`"
                )));
            }
            settings.profile_spec(profile.clone())?
        } else {
            let profiles = settings.configured_profiles(&config)?;
            let resolved = profiles
                .resolve(task_record.role)
                .map_err(|error| Error::Config(error.to_string()))?;
            if resolved.primary.profile == profile {
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
            }
        };
        let brief = self
            .store
            .coordinator_context(&self.project)?
            .brief(&task_record)?;
        self.record(
            &event_key(&[
                "worker_turn_launch_requested",
                task.as_str(),
                &attempt.to_string(),
            ]),
            Fact {
                at: now(),
                kind: FactKind::WorkerTurnLaunchRequested { task: task.clone() },
            },
        )?;
        let account = spec.account.trim();
        let session = self
            .sessions
            .launch(&LaunchRequest {
                directory: worktree.path,
                profile: SessionProfile {
                    account: if account.is_empty() {
                        None
                    } else {
                        Some(account.into())
                    },
                    harness: spec.harness,
                    model: spec.model,
                    effort: spec.effort,
                },
                kind: Some(crate::vocabulary::role_name(task_record.role).to_owned()),
                prompt: brief,
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

    fn surface_unresolved_turn(&self, task: &TaskId) -> Result<()> {
        let attempt = self.task(task)?.attempts.len();
        if attempt == 0 {
            return Err(Error::Project(format!("task `{task}` has no attempt")));
        }
        self.record(
            &event_key(&[
                "worker_turn_unresolved",
                task.as_str(),
                &attempt.to_string(),
            ]),
            Fact {
                at: now(),
                kind: FactKind::WorkerTurnUnresolved { task: task.clone() },
            },
        )
    }

    fn record_resumed(&self, task: &TaskId, session: &SessionId) -> Result<()> {
        let at = now();
        self.record(
            &event_key(&["worker_resumed", task.as_str(), &at.millis().to_string()]),
            Fact {
                at,
                kind: FactKind::WorkerTurnStarted {
                    task: task.clone(),
                    session: session.clone(),
                },
            },
        )
    }

    fn resume(&self, task: TaskId) -> Result<()> {
        let record = self.task(&task)?;
        let session = self.session_for(&record)?;
        let answers = self.answers_since_turn_start(&task)?;
        let redirect = self.pending_redirect(&task)?;
        if answers.is_empty() && redirect.is_none() {
            return Err(Error::Project(format!(
                "task `{task}` has no answer to resume"
            )));
        }
        let attempt = self.resume_attempts(&task)? + 1;
        if attempt > MAX_RESUME_ATTEMPTS {
            return self.surface_unresolved_turn(&task);
        }
        let prompt = resume_reason(&redirect, &answers);
        self.record(
            &event_key(&[
                "worker_turn_resume_requested",
                task.as_str(),
                &now().millis().to_string(),
                &attempt.to_string(),
            ]),
            Fact {
                at: now(),
                kind: FactKind::WorkerTurnResumeRequested { task: task.clone() },
            },
        )?;
        if let Err(error) = self.sessions.resume(&session, &prompt) {
            log("resume_failed", &error.to_string());
            return Ok(());
        }
        self.record_resumed(&task, &session)
    }

    fn stop(&self, task: TaskId) -> Result<()> {
        let Ok(session) = self.session_for(&self.task(&task)?) else {
            return Ok(());
        };
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
        if let Err(error) = self.delivery.push(&task_record, &worktree, &commit) {
            let reason = error.to_string();
            log("push-failed", &reason);
            return self.record(
                &event_key(&["push_failed", task.as_str(), commit.as_str()]),
                Fact {
                    at: now(),
                    kind: FactKind::PushFailed {
                        task,
                        commit,
                        reason,
                    },
                },
            );
        }
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

    fn reconcile_notify(&self) -> Result<()> {
        let settings = self.store.home().load_settings()?;
        let Some(on_event) = &settings.on_event else {
            return Ok(());
        };
        let state = self.store.project_state(&self.project)?;
        for task in state.tasks.values() {
            let Some(event) = depot_core::blocking_event(task) else {
                continue;
            };
            let name = event.name();
            if !on_event.includes(name) {
                continue;
            }
            let key = event_key(&[
                "on_event",
                task.id.as_str(),
                name,
                &task.updated_at.millis().to_string(),
            ]);
            if self.store.event(&self.project.id, &key)?.is_some() {
                continue;
            }
            let notice = EventNotice {
                project: self.project.slug.clone(),
                task: task.id.clone(),
                title: task.title.clone(),
                event: name,
                question: task
                    .questions
                    .iter()
                    .rev()
                    .find(|question| question.answer.is_none())
                    .map(|question| question.text.clone()),
                pull_request: task.pull_request().map(|(_, url, _)| url.to_owned()),
            };
            if let Err(error) = self.hook.notify(&notice) {
                log("on_event_failed", &error.to_string());
                continue;
            }
            self.record(
                &key,
                Fact {
                    at: now(),
                    kind: FactKind::OnEventNotified {
                        task: task.id.clone(),
                        event: name.to_owned(),
                    },
                },
            )?;
        }
        Ok(())
    }

    fn reconcile_start(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.values() {
            let Some(attempt) = task.attempts.last() else {
                continue;
            };
            if task.state.in_flight() && attempt.outcome.is_open() && attempt.worktree.is_none() {
                self.acquire(task.id.clone(), depot_core::worktree_baseline(task))?;
            }
        }
        for task in self.store.tasks(&self.project.id)?.values() {
            let Some(attempt) = task.attempts.last() else {
                continue;
            };
            if task.state.in_flight()
                && attempt.outcome.is_open()
                && attempt.worktree.is_some()
                && attempt.session.is_none()
            {
                self.launch(task.id.clone(), attempt.profile.clone())?;
            }
        }
        Ok(())
    }

    fn reconcile_resume(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.into_values() {
            let resumable = task
                .attempts
                .last()
                .is_some_and(|attempt| attempt.outcome.is_open() && attempt.session.is_some());
            if !resumable || task.has_unanswered_question() {
                continue;
            }
            let redirect = self.pending_redirect(&task.id)?;
            if !self.resume_is_owed(&task.id)?
                && !self.resume_is_pending(&task.id)?
                && redirect.is_none()
            {
                continue;
            }
            if redirect.is_some()
                && !self.resume_is_owed(&task.id)?
                && !self.resume_is_pending(&task.id)?
                && self.session_turn_is_running(&task)?
            {
                continue;
            }
            self.resume(task.id)?;
        }
        Ok(())
    }

    fn session_turn_is_running(&self, task: &Task) -> Result<bool> {
        let session = self.session_for(task)?;
        Ok(matches!(
            self.sessions
                .status(&session)
                .map_err(|error| Error::Project(error.to_string()))?,
            crate::adapters::sessions::SessionState::Running
        ))
    }

    fn leased_worktree(&self, task: &TaskId) -> Result<Option<WorktreeLease>> {
        let repo = self.repository()?;
        let holder = format!("depot:{}", task.as_str());
        let pool = self
            .worktrees
            .pool(&repo)
            .map_err(|error| Error::Project(error.to_string()))?;
        Ok(pool
            .into_iter()
            .find(|entry| entry.holder.as_deref() == Some(holder.as_str()))
            .and_then(|entry| entry.lease))
    }

    fn acquire_is_pending(&self, task: &TaskId) -> Result<bool> {
        let attempt = self.task(task)?.attempts.len();
        if attempt == 0 {
            return Ok(false);
        }
        let requested = self.store.event(
            &self.project.id,
            &event_key(&[
                "worktree_acquire_requested",
                task.as_str(),
                &attempt.to_string(),
            ]),
        )?;
        Ok(requested.is_some())
    }

    fn launch_is_pending(&self, task: &TaskId) -> Result<bool> {
        let attempt = self.task(task)?.attempts.len();
        if attempt == 0 {
            return Ok(false);
        }
        let requested = self.store.event(
            &self.project.id,
            &event_key(&[
                "worker_turn_launch_requested",
                task.as_str(),
                &attempt.to_string(),
            ]),
        )?;
        let Some(requested) = requested else {
            return Ok(false);
        };
        let started = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerTurnStarted),
        )?;
        Ok(Some(requested.id) > started)
    }

    fn latest_answered(&self, task: &TaskId) -> Result<Option<u64>> {
        self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::QuestionAnswered),
        )
    }

    fn answers_since_turn_start(&self, task: &TaskId) -> Result<Vec<(String, String)>> {
        let events = self.store.events(&self.project.id)?;
        let start = events
            .iter()
            .rev()
            .find(|event| {
                event.task.as_ref() == Some(task)
                    && event.kind == fact_tag_name(FactTag::WorkerTurnStarted)
            })
            .map(|event| event.id)
            .unwrap_or(0);
        let mut open: Vec<(u32, String, bool)> = Vec::new();
        let mut owed: Vec<(u32, String, String)> = Vec::new();
        for event in events
            .iter()
            .filter(|event| event.task.as_ref() == Some(task))
        {
            match event.kind.as_str() {
                kind if kind == fact_tag_name(FactTag::QuestionAsked) => {
                    let asked = open.len() as u32;
                    open.push((asked, payload_field(&event.payload, "text")?, false));
                }
                kind if kind == fact_tag_name(FactTag::QuestionAnswered) => {
                    let answer = payload_field(&event.payload, "answer")?;
                    if let Some(question) =
                        open.iter_mut().rev().find(|(_, _, answered)| !*answered)
                    {
                        question.2 = true;
                        if event.id > start {
                            owed.push((question.0, question.1.clone(), answer));
                        }
                    }
                }
                _ => {}
            }
        }
        owed.sort_by_key(|(asked, _, _)| *asked);
        Ok(owed
            .into_iter()
            .map(|(_, question, answer)| (question, answer))
            .collect())
    }

    fn resume_attempts(&self, task: &TaskId) -> Result<u32> {
        let resumed = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerTurnStarted),
        )?;
        let attempts = self
            .store
            .events_since(&self.project.id, resumed.unwrap_or(0))?
            .into_iter()
            .filter(|event| {
                event.task.as_ref() == Some(task)
                    && event.kind == fact_tag_name(FactTag::WorkerTurnResumeRequested)
            })
            .count();
        Ok(attempts as u32)
    }

    fn answer_owed(&self, task: &TaskId) -> Result<bool> {
        let answered = self.latest_answered(task)?;
        let resumed = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerTurnStarted),
        )?;
        Ok(answered.is_some() && answered > resumed)
    }

    fn resume_is_pending(&self, task: &TaskId) -> Result<bool> {
        let requested = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerTurnResumeRequested),
        )?;
        let resumed = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerTurnStarted),
        )?;
        Ok(requested > resumed)
    }

    fn resume_is_owed(&self, task: &TaskId) -> Result<bool> {
        Ok(self.answer_owed(task)? && !self.resume_is_pending(task)?)
    }

    fn pending_redirect(&self, task: &TaskId) -> Result<Option<String>> {
        let events = self.store.events(&self.project.id)?;
        let started = events
            .iter()
            .rev()
            .find(|event| {
                event.task.as_ref() == Some(task)
                    && event.kind == fact_tag_name(FactTag::WorkerTurnStarted)
            })
            .map(|event| event.id)
            .unwrap_or(0);
        let redirected = events
            .iter()
            .rev()
            .find(|event| {
                event.task.as_ref() == Some(task)
                    && event.kind == fact_tag_name(FactTag::WorkerRedirected)
            })
            .filter(|event| event.id > started);
        match redirected {
            Some(event) => Ok(Some(payload_field(&event.payload, "text")?)),
            None => Ok(None),
        }
    }

    fn reconcile_sessions(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.into_values() {
            if !task.state.in_flight()
                || self.answer_owed(&task.id)?
                || self.pending_redirect(&task.id)?.is_some()
            {
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
            if depot_core::publication_blocked(&state, &task.id) {
                continue;
            }
            match task.state {
                TaskState::Validated if task.pull_request().is_none() => {
                    let Some(commit) = task.validated_commit().cloned() else {
                        continue;
                    };
                    self.push(task.id.clone(), commit.clone())?;
                    self.open_pull_request(task.id.clone(), commit)?;
                }
                TaskState::PrOpen => {
                    let Some(commit) = task.push_owed().cloned() else {
                        continue;
                    };
                    self.push(task.id.clone(), commit)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn reconcile_forge(&self) -> Result<()> {
        let state = self.store.project_state(&self.project)?;
        let mut observed_open: Vec<(TaskId, u64, ObservedPullRequest)> = Vec::new();
        for task in state.tasks.values() {
            if task.state != TaskState::PrOpen {
                continue;
            }
            let Some((number, _, recorded)) = task.pull_request() else {
                continue;
            };
            let observed = match self
                .project_repo()
                .and_then(|repo| self.delivery.observe_pull_request(task, &repo))
            {
                Ok(observed) => observed,
                Err(error) => {
                    log("forge_unavailable", &error.to_string());
                    continue;
                }
            };
            let Some(observed) = observed else {
                continue;
            };
            let at = now();
            match observed.state {
                PrState::Merged => {
                    self.record(
                        &event_key(&[
                            "pull_request_merged",
                            task.id.as_str(),
                            observed.commit.as_str(),
                        ]),
                        Fact {
                            at,
                            kind: FactKind::PullRequestMerged {
                                task: task.id.clone(),
                                commit: observed.commit,
                            },
                        },
                    )?;
                }
                PrState::Closed => {
                    self.record(
                        &event_key(&["pull_request_closed_unmerged", task.id.as_str()]),
                        Fact {
                            at,
                            kind: FactKind::PullRequestClosedUnmerged {
                                task: task.id.clone(),
                            },
                        },
                    )?;
                }
                PrState::Open => {
                    if observed.checks != recorded {
                        self.record(
                            &event_key(&[
                                "pull_request_checks_changed",
                                task.id.as_str(),
                                checks_name(observed.checks),
                                &at.millis().to_string(),
                            ]),
                            Fact {
                                at,
                                kind: FactKind::PullRequestChecksChanged {
                                    task: task.id.clone(),
                                    checks: observed.checks,
                                },
                            },
                        )?;
                    }
                    observed_open.push((task.id.clone(), number, observed));
                }
            }
        }
        self.auto_merge(observed_open)
    }

    fn auto_merge(&self, observed_open: Vec<(TaskId, u64, ObservedPullRequest)>) -> Result<()> {
        let state = self.store.project_state(&self.project)?;
        for (id, number, observed) in observed_open {
            let Some(task) = state.tasks.get(&id) else {
                continue;
            };
            if !depot_core::auto_merge_due(&state, task, &observed.commit) {
                continue;
            }
            if self.merge_was_refused_at(&id, &observed)? {
                continue;
            }
            let attempt = self.project_repo().and_then(|repo| {
                self.delivery
                    .merge_pull_request(task, &repo, number, &observed.commit)
            });
            if let Err(error) = attempt {
                log("auto_merge_failed", &error.to_string());
                self.record_merge_refusal(&id, &observed, &error.to_string())?;
                continue;
            }
            self.record(
                &event_key(&["pull_request_merged", id.as_str(), observed.commit.as_str()]),
                Fact {
                    at: now(),
                    kind: FactKind::PullRequestMerged {
                        task: id.clone(),
                        commit: observed.commit.clone(),
                    },
                },
            )?;
        }
        Ok(())
    }

    fn merge_was_refused_at(&self, task: &TaskId, observed: &ObservedPullRequest) -> Result<bool> {
        let refusal = self
            .store
            .events(&self.project.id)?
            .into_iter()
            .rev()
            .find(|event| {
                event.task.as_ref() == Some(task)
                    && event.kind == fact_tag_name(FactTag::PullRequestMergeRefused)
            });
        let Some(refusal) = refusal else {
            return Ok(false);
        };
        Ok(
            payload_field(&refusal.payload, "commit")? == observed.commit.as_str()
                && payload_field(&refusal.payload, "base")? == observed.base.as_str()
                && payload_field(&refusal.payload, "checks")? == checks_name(observed.checks),
        )
    }

    fn record_merge_refusal(
        &self,
        task: &TaskId,
        observed: &ObservedPullRequest,
        reason: &str,
    ) -> Result<()> {
        let at = now();
        self.record(
            &event_key(&[
                "pull_request_merge_refused",
                task.as_str(),
                observed.commit.as_str(),
                observed.base.as_str(),
                checks_name(observed.checks),
                &at.millis().to_string(),
            ]),
            Fact {
                at,
                kind: FactKind::PullRequestMergeRefused {
                    task: task.clone(),
                    commit: observed.commit.clone(),
                    base: observed.base.clone(),
                    checks: observed.checks,
                    reason: reason.to_owned(),
                },
            },
        )
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

    fn project_repo(&self) -> Result<RepoSlug> {
        let remote = git_output(&self.repository()?, &["remote", "get-url", "origin"])?;
        repo_slug(remote.trim())
    }
}

pub fn resume_prompt(answers: &[(String, String)]) -> String {
    match answers {
        [] => "An answer was recorded. Continue the task.".to_string(),
        several => {
            let answered: Vec<String> = several
                .iter()
                .map(|(question, answer)| {
                    format!(
                        "Your question \"{}\" was answered: {}.",
                        crate::checklist::one_line(question),
                        stripped(answer)
                    )
                })
                .collect();
            format!("{} Continue the task.", answered.join(" "))
        }
    }
}

pub fn resume_reason(redirect: &Option<String>, answers: &[(String, String)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = redirect {
        parts.push(format!(
            "The task was redirected: {}. Take the new direction into account.",
            stripped(text)
        ));
    }
    if !answers.is_empty() {
        parts.push(resume_prompt(answers));
    }
    parts.join(" ")
}

fn stripped(answer: &str) -> String {
    let answer = crate::checklist::one_line(answer);
    answer.strip_suffix('.').unwrap_or(&answer).to_owned()
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
    use super::{
        EventHook, EventNotice, ShellEventHook, ShellValidation, ValidationRunner, repo_slug,
    };
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
            dispatch_profile: None,
            state: depot_core::TaskState::Proposed,
            dependencies: Vec::new(),
            base_dependency: None,
            attempts: Vec::new(),
            questions: Vec::new(),
            validations: Vec::new(),
            submission: None,
            artifacts: Vec::new(),
            links: Vec::new(),
            branch_head: None,
            merge_refused: None,
            acknowledged_at: None,
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
    fn the_shell_hook_feeds_the_notice_payload_on_stdin() {
        let temp = TempDir::new().expect("temporary directory");
        let log = temp.path().join("hook.log");
        let command = if cfg!(windows) {
            format!("more > {}", log.display())
        } else {
            format!("cat > {}", log.display())
        };
        let notice = EventNotice {
            project: "depot".to_owned(),
            task: depot_core::TaskId::new("t-7"),
            title: "on_event hook".to_owned(),
            event: "question",
            question: Some("which store?".to_owned()),
            pull_request: None,
        };
        ShellEventHook::new(command)
            .notify(&notice)
            .expect("the hook runs");
        let payload: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&log).expect("hook output").trim())
                .expect("the payload is json");
        assert_eq!(payload["project"], "depot");
        assert_eq!(payload["task"], "t-7");
        assert_eq!(payload["event"], "question");
        assert_eq!(payload["question"], "which store?");
        assert!(payload["recommended_default"].is_null());
        assert!(payload["pull_request"].is_null());
    }

    #[test]
    fn a_failing_hook_command_is_an_error() {
        let notice = EventNotice {
            project: "depot".to_owned(),
            task: depot_core::TaskId::new("t-7"),
            title: "on_event hook".to_owned(),
            event: "failed",
            question: None,
            pull_request: None,
        };
        let exit = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
        let error = ShellEventHook::new(exit)
            .notify(&notice)
            .expect_err("a failing command is an error");
        assert!(error.to_string().contains("3"), "{error}");
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
