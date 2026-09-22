use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};

use fs2::FileExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use depot_core::{
    Action, Baseline, Checks, CommitId, Dependency, Fact, FactKind, Liveness, MergePolicy,
    ProjectId, Role, SessionId, Task, TaskId, TaskState, Timestamp, WorktreeLease,
};
use serde::{Deserialize, Serialize};

use crate::adapters::forge::{Forge, NewPullRequest, PrState, RepoSlug};
use crate::adapters::profiles::ProfileResolver;
use crate::adapters::sessions::{LaunchRequest, SessionProfile, Sessions};
use crate::adapters::worktrees::{AcquireRequest, Lease, PoolEntry, WorktreeError, Worktrees};
use crate::clock::now;
use crate::describe::{self, Describer};
use crate::error::{Error, Result};
use crate::evidence::{self, EvidenceArtifact, EvidenceRunner, ResolvedArtifact, ShellEvidence};
use crate::factcodec::payload_field;
use crate::home::DepotHome;
use crate::project::{LocationKind, Project};
use crate::store::{EventOutcome, RecordedEvent, Store, event_key};
use crate::vocabulary::{FactTag, checks_name, fact_tag, fact_tag_name};

pub const DAEMON_LOCK_FILE_NAME: &str = "depotd.lock";
pub const DAEMON_SCOPE_FILE_NAME: &str = "depotd.scope.json";
const MAX_RESUME_ATTEMPTS: u32 = 3;
const MAX_DEFERRED_ATTEMPTS: u32 = 3;
const LIVENESS_REFRESH_MILLIS: u64 = 60_000;
const RELEASE_HOLD_BACKOFF_MILLIS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonScope {
    pub pid: u32,
    pub started_at_millis: u64,
    pub projects: Vec<String>,
    pub heartbeat_millis: u64,
    #[serde(default)]
    pub build_id: String,
}

impl DaemonScope {
    pub fn new(projects: &[Project], pid: u32, at: Timestamp) -> Self {
        Self {
            pid,
            started_at_millis: at.millis(),
            projects: projects
                .iter()
                .map(|project| project.id.to_string())
                .collect(),
            heartbeat_millis: at.millis(),
            build_id: crate::BUILD_ID.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    scope_path: PathBuf,
}

impl InstanceLock {
    pub fn record_scope(&self, projects: &[Project]) -> Result<DaemonScope> {
        let scope = DaemonScope::new(projects, std::process::id(), now());
        self.write_scope(&scope)?;
        Ok(scope)
    }

    pub fn refresh_heartbeat(&self) -> Result<()> {
        let bytes = std::fs::read(&self.scope_path).map_err(|error| {
            Error::Home(format!(
                "could not read the daemon scope record {}: {error}",
                self.scope_path.display()
            ))
        })?;
        let mut scope: DaemonScope = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Home(format!(
                "could not parse the daemon scope record {}: {error}",
                self.scope_path.display()
            ))
        })?;
        scope.heartbeat_millis = now().millis();
        self.write_scope(&scope)
    }

    fn write_scope(&self, scope: &DaemonScope) -> Result<()> {
        let mut temp_name = self.scope_path.as_os_str().to_os_string();
        temp_name.push(".tmp");
        let temp = PathBuf::from(temp_name);
        let bytes = serde_json::to_vec(scope).map_err(|error| {
            Error::Home(format!("could not encode the daemon scope record: {error}"))
        })?;
        std::fs::write(&temp, bytes).map_err(|error| {
            Error::Home(format!(
                "could not write the daemon scope record {}: {error}",
                temp.display()
            ))
        })?;
        std::fs::rename(&temp, &self.scope_path).map_err(|error| {
            Error::Home(format!(
                "could not replace the daemon scope record {}: {error}",
                self.scope_path.display()
            ))
        })?;
        Ok(())
    }

    pub fn acquire(home: &DepotHome) -> Result<Self> {
        home.ensure()?;
        let path = home.root().join(DAEMON_LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let scope = daemon_scope(home);
        file.try_lock_exclusive().map_err(|error| {
            Error::Home(format!(
                "{} ({error})",
                lock_held_message(home, &path, scope.as_ref())
            ))
        })?;
        Ok(Self {
            _file: file,
            scope_path: home.root().join(DAEMON_SCOPE_FILE_NAME),
        })
    }
}

fn lock_held_message(home: &DepotHome, path: &Path, scope: Option<&DaemonScope>) -> String {
    let held = format!("another depot daemon already holds {}", path.display());
    match scope {
        Some(scope) if scope.pid != 0 => {
            let covering = if scope.projects.is_empty() {
                "no recorded projects".to_string()
            } else {
                project_slugs(home, &scope.projects).join(", ")
            };
            format!(
                "{held}: pid {pid} covering {covering}; restart it with `depot daemon restart` or stop pid {pid}",
                pid = scope.pid
            )
        }
        _ => format!(
            "{held}: the lock record names no daemon, so depot cannot identify the holder; stop the process holding it by hand"
        ),
    }
}

pub fn daemon_scope(home: &DepotHome) -> Option<DaemonScope> {
    let bytes = std::fs::read(home.root().join(DAEMON_SCOPE_FILE_NAME)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn scope_is_fresh(scope: &DaemonScope, now: Timestamp, stale_after: Duration) -> bool {
    now.millis().saturating_sub(scope.heartbeat_millis) <= stale_after.as_millis() as u64
}

pub fn daemon_build_mismatch(home: &DepotHome, now: Timestamp) -> Option<DaemonScope> {
    let stale_after = home.load_settings().ok()?.poll_interval().saturating_mul(3);
    let scope = daemon_scope(home)?;
    let mismatched = !scope.build_id.is_empty() && scope.build_id != crate::BUILD_ID;
    (mismatched && scope_is_fresh(&scope, now, stale_after)).then_some(scope)
}

pub fn daemon_scope_covers(home: &DepotHome, project: &ProjectId, now: Timestamp) -> Result<bool> {
    let stale_after = home.load_settings()?.poll_interval().saturating_mul(3);
    let Some(scope) = daemon_scope(home) else {
        return Ok(false);
    };
    Ok(scope_is_fresh(&scope, now, stale_after)
        && scope
            .projects
            .iter()
            .any(|covered| covered == project.as_str()))
}

pub(crate) fn project_slugs(home: &DepotHome, ids: &[String]) -> Vec<String> {
    let Ok(store) = Store::open(home) else {
        return ids.to_vec();
    };
    ids.iter()
        .map(|id| {
            store
                .project(&ProjectId::new(id.clone()))
                .ok()
                .flatten()
                .map(|project| project.slug)
                .unwrap_or_else(|| id.clone())
        })
        .collect()
}

pub trait ValidationRunner {
    fn validate(
        &self,
        task: &Task,
        worktree: &Path,
        commit: &CommitId,
        base: &str,
        command: &str,
    ) -> Result<ValidationResult>;
}

pub trait Delivery {
    fn push(&self, task: &Task, worktree: &Path, commit: &CommitId) -> Result<()>;
    fn commit_on_base(
        &self,
        task: &Task,
        worktree: &Path,
        base: &str,
        commit: &CommitId,
    ) -> Result<bool>;
    fn open_pull_request(
        &self,
        task: &Task,
        worktree: &Path,
        commit: &CommitId,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<(u64, String)>;
    fn update_pull_request(
        &self,
        task: &Task,
        repo: &RepoSlug,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<()>;
    fn pull_request_content(
        &self,
        task: &Task,
        repo: &RepoSlug,
        number: u64,
    ) -> Result<(String, String)>;
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
    fn delete_branch(&self, repo: &RepoSlug, branch: &str) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedPullRequest {
    pub state: PrState,
    pub checks: Checks,
    pub commit: CommitId,
    pub head_ref: String,
    pub base: CommitId,
    pub mergeable: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellValidation;

impl ValidationRunner for ShellValidation {
    fn validate(
        &self,
        _task: &Task,
        worktree: &Path,
        commit: &CommitId,
        base: &str,
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
        let base_commit = fetch_base(worktree, base)?;
        let scratch = ScratchWorktree::add(worktree, commit)?;
        let started = Instant::now();
        let output = match merge_base(scratch.path(), base)? {
            Some(conflict) => conflict,
            None => shell(command, scratch.path())?,
        };
        Ok(ValidationResult {
            exit_code: output.status.code().unwrap_or(-1),
            duration: started.elapsed(),
            output_tail: output_tail(&output),
            base_commit: Some(base_commit),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ForgeDelivery<F> {
    forge: F,
}

impl<F> ForgeDelivery<F> {
    pub fn new(forge: F) -> Self {
        Self { forge }
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
        let args = [
            "push".to_owned(),
            "origin".to_owned(),
            format!("HEAD:refs/heads/{branch}"),
        ];
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        git_output(worktree, &arg_refs)?;
        Ok(())
    }

    fn commit_on_base(
        &self,
        _task: &Task,
        worktree: &Path,
        base: &str,
        commit: &CommitId,
    ) -> Result<bool> {
        fetch_base(worktree, base)?;
        match git_exit(
            worktree,
            &[
                "merge-base",
                "--is-ancestor",
                commit.as_str(),
                &format!("origin/{base}"),
            ],
        )? {
            0 => Ok(true),
            1 => Ok(false),
            code => Err(Error::Project(format!(
                "git merge-base --is-ancestor exited {code} for {} against origin/{base}",
                commit.as_str()
            ))),
        }
    }

    fn open_pull_request(
        &self,
        task: &Task,
        worktree: &Path,
        _commit: &CommitId,
        base: &str,
        title: &str,
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
            Some(opened) => {
                let (_, existing) = self
                    .forge
                    .pull_request_text(&repo, opened.number)
                    .map_err(|error| Error::Project(error.to_string()))?;
                let existing = existing.unwrap_or_default();
                if evidence::is_managed(&existing) {
                    let body = evidence::preserve_proof(&existing, &evidence::mark_managed(body));
                    self.forge
                        .update_pull_request(&repo, opened.number, title, &body)
                        .map_err(|error| Error::Project(error.to_string()))?;
                }
                opened
            }
            None => self
                .forge
                .open_pull_request(&NewPullRequest {
                    repo,
                    title: title.to_owned(),
                    body: evidence::mark_managed(body),
                    head,
                    base: base.to_owned(),
                })
                .map_err(|error| Error::Project(error.to_string()))?,
        };
        Ok((opened.number, opened.url))
    }

    fn update_pull_request(
        &self,
        _task: &Task,
        repo: &RepoSlug,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<()> {
        self.forge
            .update_pull_request(repo, number, title, body)
            .map_err(|error| Error::Project(error.to_string()))
    }

    fn pull_request_content(
        &self,
        _task: &Task,
        repo: &RepoSlug,
        number: u64,
    ) -> Result<(String, String)> {
        let (title, body) = self
            .forge
            .pull_request_text(repo, number)
            .map_err(|error| Error::Project(error.to_string()))?;
        Ok((title, body.unwrap_or_default()))
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
            head_ref: observed.head_ref,
            base: observed.base,
            mergeable: observed.mergeable,
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

    fn delete_branch(&self, repo: &RepoSlug, branch: &str) -> Result<()> {
        self.forge
            .delete_branch(repo, branch)
            .map_err(|error| Error::Project(error.to_string()))
    }
}

impl EventHook for Box<dyn EventHook> {
    fn notify(&self, notice: &EventNotice) -> Result<()> {
        self.as_ref().notify(notice)
    }
}

impl EventHook for std::sync::Arc<dyn EventHook> {
    fn notify(&self, notice: &EventNotice) -> Result<()> {
        self.as_ref().notify(notice)
    }
}

pub(crate) fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    let process = {
        use std::os::windows::process::CommandExt;
        let mut process = Command::new("cmd");
        process.arg("/C").raw_arg(command);
        process
    };
    #[cfg(not(windows))]
    let process = {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    };
    process
}

fn shell(command: &str, worktree: &Path) -> Result<std::process::Output> {
    shell_command(command)
        .current_dir(worktree)
        .output()
        .map_err(Error::Io)
}

fn delivery_branch(worktree: &Path, task: &Task) -> Result<String> {
    let current = git_output(worktree, &["branch", "--show-current"])?;
    let current = current.trim();
    if !current.is_empty() {
        return Ok(current.to_owned());
    }
    let listed = git_output(worktree, &["branch", "--format=%(refname:short)"])?;
    let taken: Vec<String> = listed
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    let branch = depot_core::delivery_branch(task, &taken);
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

fn git_exit(worktree: &Path, args: &[&str]) -> Result<i32> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(Error::Io)?;
    Ok(output.status.code().unwrap_or(-1))
}

fn fetch_base(worktree: &Path, base: &str) -> Result<CommitId> {
    let refspec = format!("+refs/heads/{base}:refs/remotes/origin/{base}");
    git_output(worktree, &["fetch", "origin", &refspec])
        .map_err(|error| Error::Project(format!("could not fetch base `{base}`: {error}")))?;
    let commit = git_output(
        worktree,
        &["rev-parse", &format!("refs/remotes/origin/{base}")],
    )?;
    Ok(CommitId::new(commit.trim()))
}

fn merge_base(scratch: &Path, base: &str) -> Result<Option<std::process::Output>> {
    let hooks = scratch.join("depot-merge-hooks");
    let output = Command::new("git")
        .arg("-C")
        .arg(scratch)
        .args(["-c"])
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(["merge", "--no-commit", "--no-ff", &format!("origin/{base}")])
        .output()
        .map_err(Error::Io)?;
    if output.status.success() {
        Ok(None)
    } else {
        Ok(Some(output))
    }
}

struct ScratchWorktree {
    repo: PathBuf,
    path: PathBuf,
}

impl ScratchWorktree {
    fn add(repo: &Path, commit: &CommitId) -> Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "depot-validation-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "remove", "--force"])
            .arg(&path)
            .output();
        let _ = std::fs::remove_dir_all(&path);
        let path_text = path
            .to_str()
            .ok_or_else(|| Error::Project("the scratch worktree path is not utf-8".into()))?;
        if let Err(error) = git_output(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                "--force",
                path_text,
                commit.as_str(),
            ],
        ) {
            let _ = std::fs::remove_dir_all(&path);
            return Err(error);
        }
        Ok(Self {
            repo: repo.to_path_buf(),
            path,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchWorktree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .output();
        let _ = std::fs::remove_dir_all(&self.path);
    }
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
        let mut child = shell_command(&self.command)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    evidence: Box<dyn EvidenceRunner>,
    budget: Cell<usize>,
    deferred_this_tick: RefCell<BTreeSet<TaskId>>,
}

pub struct ValidationResult {
    pub exit_code: i32,
    pub duration: Duration,
    pub output_tail: String,
    pub base_commit: Option<CommitId>,
}

enum DescribeOutcome {
    Output(describe::DescribeOutput),
    OptOut,
    HeldForUser,
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
            evidence: Box::new(ShellEvidence),
            budget: Cell::new(usize::MAX),
            deferred_this_tick: RefCell::new(BTreeSet::new()),
        }
    }

    pub fn with_evidence_runner(mut self, evidence: Box<dyn EvidenceRunner>) -> Self {
        self.evidence = evidence;
        self
    }

    pub fn recover(&self) -> Result<()> {
        self.deferred_this_tick.borrow_mut().clear();
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
        self.reconcile_budget()?;
        self.reconcile_sessions()?;
        self.reconcile_stops()?;
        self.reconcile_leases(true)?;
        self.reconcile_notify()
    }

    pub fn tick(&self) -> Result<()> {
        self.tick_under(usize::MAX).map(|_| ())
    }

    pub fn tick_under(&self, budget: usize) -> Result<usize> {
        self.budget.set(budget);
        self.deferred_this_tick.borrow_mut().clear();
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
        self.reconcile_budget()?;
        self.reconcile_sessions()?;
        self.reconcile_stops()?;
        self.reconcile_validation()?;
        self.reconcile_delivery()?;
        self.reconcile_evidence()?;
        self.reconcile_forge()?;
        self.reconcile_leases(false)?;
        self.reconcile_notify()?;
        Ok(self.budget.get())
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
        let previous = self.store.last_liveness(&self.project.id, &task)?;
        let resolves_unknown = self
            .store
            .task(&self.project.id, &task)?
            .and_then(|task| task.attempts.last().map(|attempt| attempt.outcome))
            .is_some_and(|outcome| outcome == depot_core::AttemptOutcome::Unknown);
        let recovered = self.deferred_attempts(&task)? > 0;
        let stale = previous.as_ref().is_none_or(|(_, at)| {
            now().millis().saturating_sub(at.millis()) >= LIVENESS_REFRESH_MILLIS
        });
        if previous.as_ref().map(|(value, _)| *value) == Some(liveness)
            && !resolves_unknown
            && !stale
            && !recovered
        {
            return Ok(());
        }
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
        if logs_fact(&fact.kind) {
            log("fact", key);
        }
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
            Action::AcquireWorktree { task, baseline } => {
                self.deferring(&task, || self.admit(task.clone(), baseline))
            }
            Action::LaunchSession { task, profile } => {
                self.deferring(&task, || self.launch(task.clone(), profile))
            }
            Action::ResumeSession { task } => self.deferring(&task, || self.resume(task.clone())),
            Action::StopSession { task } => {
                let id = task.clone();
                self.deferring(&id, || self.stop(task))
            }
            Action::RunValidation { task, commit } => self.validate(task, commit),
            Action::Push { task, commit } => self.push(task, commit),
            Action::OpenPullRequest { task, commit } => self.open_pull_request(task, commit),
            Action::ReleaseWorktree { task, lease } => self.release(task, lease),
            Action::Notify { .. } => Ok(()),
            Action::RenderChecklist | Action::Queue { .. } | Action::HoldForUser { .. } => Ok(()),
            Action::RotateCoordinator { .. } => Ok(()),
        }
    }

    fn admit(&self, task: TaskId, baseline: Baseline) -> Result<()> {
        let budget = self.budget.get();
        if budget == 0 {
            log("queued", task.as_str());
            return Ok(());
        }
        self.budget.set(budget - 1);
        self.acquire(task, baseline)
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
        if let Some(lease) = self
            .task(&task)?
            .attempts
            .iter()
            .rev()
            .find_map(|attempt| attempt.worktree.clone())
            && self.lease_is_in_pool(&lease)?
        {
            return self.record_worktree_acquired(&task, lease, baseline);
        }
        let repo = self.repository()?;
        let task = self.task(&task)?;
        let taken = self
            .worktrees
            .branches(&repo)
            .map_err(|error| Error::Project(error.to_string()))?;
        let lease = self
            .worktrees
            .acquire(&AcquireRequest {
                repo,
                holder: format!("depot:{}", task.id.as_str()),
                branch: depot_core::delivery_branch(&task, &taken),
                baseline: baseline.clone(),
            })
            .map_err(|error| Error::Project(error.to_string()))?;
        self.record_worktree_acquired(&task.id, lease.lease, baseline)
    }

    fn record_worktree_acquired(
        &self,
        task: &TaskId,
        lease: WorktreeLease,
        baseline: Baseline,
    ) -> Result<()> {
        let attempt = self.task(task)?.attempts.len();
        self.record(
            &event_key(&[
                "worktree_acquired",
                task.as_str(),
                &attempt.to_string(),
                lease.as_str(),
            ]),
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
            return self.surface_unresolved_turn(&task, "a previous launch intent never completed");
        }
        let task_record = self.task(&task)?;
        let worktree = match self.lease_for(&task_record) {
            Ok(worktree) => worktree,
            Err(error) => return self.defer_turn(&task, &error.to_string()),
        };
        let attempt = task_record.attempts.len();
        if attempt == 0 {
            return Err(Error::Project(format!("task `{task}` has no attempt")));
        }
        let config = self.store.project_config(&self.project)?;
        let settings = self.store.home().load_settings()?;
        let base_merge = task_record
            .attempts
            .last()
            .is_some_and(|attempt| attempt.base_merge);
        let spec = if base_merge {
            settings.profile_spec(profile.clone())?
        } else if let Some(pinned) = &task_record.dispatch_profile {
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
        let context = self.store.coordinator_context(&self.project)?;
        let brief = if base_merge {
            context.conflict_brief(&task_record)?
        } else {
            context.brief(&task_record)?
        };
        let mut prompt = brief;
        let answers = self.answers_all(&task)?;
        let redirect = self.pending_redirect(&task)?;
        let redirect_event = self.pending_redirect_event(&task)?;
        if !answers.is_empty() || redirect.is_some() {
            prompt.push_str("\n\n");
            prompt.push_str(&resume_reason(&redirect, &answers));
        }
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
                prompt,
            })
            .map_err(|error| Error::Project(error.to_string()))?;
        self.record(
            &event_key(&["worker_turn_started", task.as_str(), session.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::WorkerTurnStarted {
                    task: task.clone(),
                    session: session.clone(),
                },
            },
        )?;
        if let Some(redirect) = redirect_event {
            self.record(
                &event_key(&[
                    "worker_redirect_delivered",
                    task.as_str(),
                    &redirect.id.to_string(),
                ]),
                Fact {
                    at: now(),
                    kind: FactKind::WorkerRedirectDelivered {
                        task,
                        redirect: redirect.id.to_string(),
                    },
                },
            )?;
        }
        Ok(())
    }

    fn surface_unresolved_turn(&self, task: &TaskId, reason: &str) -> Result<()> {
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
                kind: FactKind::WorkerTurnUnresolved {
                    task: task.clone(),
                    reason: reason.to_owned(),
                },
            },
        )
    }

    fn deferring(&self, task: &TaskId, work: impl FnOnce() -> Result<()>) -> Result<()> {
        if self.deferred_this_tick.borrow().contains(task) {
            return Ok(());
        }
        match work() {
            Ok(()) => Ok(()),
            Err(error) if error.is_project() => self.defer_turn(task, &error.to_string()),
            Err(error) if error.is_task_fatal() => {
                self.surface_unresolved_turn(task, &error.to_string())
            }
            Err(error) => Err(error),
        }
    }

    fn defer_turn(&self, task: &TaskId, reason: &str) -> Result<()> {
        if !self.deferred_this_tick.borrow_mut().insert(task.clone()) {
            return Ok(());
        }
        let attempt = self.task(task)?.attempts.len();
        if attempt == 0 {
            return Err(Error::Project(format!("task `{task}` has no attempt")));
        }
        let attempts = self.deferred_attempts(task)? + 1;
        if attempts > MAX_DEFERRED_ATTEMPTS {
            return self.surface_unresolved_turn(
                task,
                &format!("the worker turn could not proceed after {attempts} attempts: {reason}"),
            );
        }
        self.record(
            &event_key(&[
                "worker_turn_deferred",
                task.as_str(),
                &attempt.to_string(),
                &attempts.to_string(),
            ]),
            Fact {
                at: now(),
                kind: FactKind::WorkerTurnDeferred {
                    task: task.clone(),
                    reason: reason.to_owned(),
                },
            },
        )
    }

    fn deferred_attempts(&self, task: &TaskId) -> Result<u32> {
        let Some(record) = self.store.task(&self.project.id, task)? else {
            return Ok(0);
        };
        let attempt = record.attempts.len();
        if attempt == 0 {
            return Ok(0);
        }
        let started = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerTurnStarted),
        )?;
        let observed = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerLivenessChanged),
        )?;
        let after = started.unwrap_or(0).max(observed.unwrap_or(0));
        let prefix = event_key(&["worker_turn_deferred", task.as_str(), &attempt.to_string()]);
        let count = self
            .store
            .events_since(&self.project.id, after)?
            .into_iter()
            .filter(|event| event.key.starts_with(&format!("{prefix}:")))
            .count();
        Ok(count as u32)
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
        let answers = self.answers_owed(&task)?;
        let redirect = self.pending_redirect(&task)?;
        if answers.is_empty() && redirect.is_none() {
            return Err(Error::Project(format!(
                "task `{task}` has no answer to resume"
            )));
        }
        match self.sessions.status(&session) {
            Ok(status) if status.state == crate::adapters::sessions::SessionState::Running => {}
            Ok(status) => {
                log(
                    "resume_dead_session",
                    &format!("{} is {:?}", task.as_str(), status.state),
                );
                return self.relaunch(&task);
            }
            Err(error) => return self.defer_turn(&task, &error.to_string()),
        }
        let attempt = self.resume_attempts(&task)? + 1;
        if attempt > MAX_RESUME_ATTEMPTS {
            return self.surface_unresolved_turn(
                &task,
                "the worker did not come back after the resume attempts",
            );
        }
        let prompt = resume_reason(&redirect, &answers);
        let redirect_event = self.pending_redirect_event(&task)?;
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
        let child = match self.sessions.resume(&session, &prompt) {
            Ok(child) => child,
            Err(error) => {
                log("resume_failed", &error.to_string());
                return Ok(());
            }
        };
        if child == session {
            log(
                "resume_failed",
                "boxr reported the resumed turn under the parent session id",
            );
            return Ok(());
        }
        self.record_resumed(&task, &child)?;
        if let Some(redirect) = redirect_event {
            self.record(
                &event_key(&[
                    "worker_redirect_delivered",
                    task.as_str(),
                    &redirect.id.to_string(),
                ]),
                Fact {
                    at: now(),
                    kind: FactKind::WorkerRedirectDelivered {
                        task,
                        redirect: redirect.id.to_string(),
                    },
                },
            )?;
        }
        Ok(())
    }

    fn relaunch(&self, task: &TaskId) -> Result<()> {
        let attempt = self.task(task)?.attempts.len();
        if attempt == 0 {
            return Err(Error::Project(format!("task `{task}` has no attempt")));
        }
        self.record(
            &event_key(&[
                "worker_relaunch_requested",
                task.as_str(),
                &attempt.to_string(),
            ]),
            Fact {
                at: now(),
                kind: FactKind::WorkerRelaunchRequested { task: task.clone() },
            },
        )
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
        let task_record = match self.task(&task) {
            Ok(record) => record,
            Err(error) => {
                return self.record_validation_failure(&task, &commit, &error.to_string());
            }
        };
        let worktree = match self.lease_for(&task_record) {
            Ok(lease) => lease.path,
            Err(error) => {
                return self.record_validation_failure(&task, &commit, &error.to_string());
            }
        };
        let config = self.store.project_config(&self.project)?;
        let base = config.pull_request.base.clone();
        let command = config.validation.command.clone();
        let result =
            match self
                .validation
                .validate(&task_record, &worktree, &commit, &base, &command)
            {
                Ok(result) => result,
                Err(error) => {
                    return self.record_validation_failure(&task, &commit, &error.to_string());
                }
            };
        self.record(
            &event_key(&["validation_finished", task.as_str(), commit.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::ValidationFinished {
                    task,
                    command,
                    commit,
                    base_commit: result.base_commit,
                    exit_code: result.exit_code,
                    duration: result.duration,
                    output_tail: result.output_tail,
                },
            },
        )
    }

    fn record_validation_failure(
        &self,
        task: &TaskId,
        commit: &CommitId,
        reason: &str,
    ) -> Result<()> {
        log("validation-failed", reason);
        self.record(
            &event_key(&["validation_failed", task.as_str(), commit.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::ValidationFailed {
                    task: task.clone(),
                    commit: commit.clone(),
                    reason: reason.to_owned(),
                },
            },
        )
    }

    fn push(&self, task: TaskId, commit: CommitId) -> Result<()> {
        let task_record = self.task(&task)?;
        let worktree = match self.lease_for(&task_record) {
            Ok(lease) => lease.path,
            Err(error) => {
                return self.record_delivery_failure(&task, &commit, &error.to_string());
            }
        };
        if task_record.state == TaskState::Validated {
            let base = self
                .store
                .project_config(&self.project)?
                .pull_request
                .base
                .clone();
            match self
                .delivery
                .commit_on_base(&task_record, &worktree, &base, &commit)
            {
                Ok(true) => return self.record_landed_on_base(&task, &commit),
                Ok(false) => {}
                Err(error) => {
                    return self.record_delivery_failure(&task, &commit, &error.to_string());
                }
            }
        }
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

    fn record_delivery_failure(
        &self,
        task: &TaskId,
        commit: &CommitId,
        reason: &str,
    ) -> Result<()> {
        log("delivery-failed", reason);
        self.record(
            &event_key(&["delivery_failed", task.as_str(), commit.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::DeliveryFailed {
                    task: task.clone(),
                    commit: commit.clone(),
                    reason: reason.to_owned(),
                },
            },
        )
    }

    fn record_landed_on_base(&self, task: &TaskId, commit: &CommitId) -> Result<()> {
        log("landed-on-base", task.as_str());
        self.record(
            &event_key(&["task_landed_on_base", task.as_str(), commit.as_str()]),
            Fact {
                at: now(),
                kind: FactKind::TaskLandedOnBase {
                    task: task.clone(),
                    commit: commit.clone(),
                },
            },
        )
    }

    fn open_pull_request(&self, task: TaskId, commit: CommitId) -> Result<()> {
        let task_record = self.task(&task)?;
        if task_record.state != TaskState::Validated || task_record.pull_request().is_some() {
            return Ok(());
        }
        let worktree = match self.lease_for(&task_record) {
            Ok(lease) => lease.path,
            Err(error) => {
                return self.record_delivery_failure(&task, &commit, &error.to_string());
            }
        };
        let config = self.store.project_config(&self.project)?;
        let base = config.pull_request.base.clone();
        let described = match self.describe_pull_request(&task_record, &worktree, &commit, &config)
        {
            Ok(outcome) => outcome,
            Err(error) => {
                return self.record_delivery_failure(&task, &commit, &error.to_string());
            }
        };
        match described {
            DescribeOutcome::Output(describe) => {
                let (title, body) = describe::assemble(Some(describe), &task_record, &commit);
                self.open_pull_request_with(&task_record, &worktree, &commit, &base, &title, &body)
            }
            DescribeOutcome::OptOut => {
                let (title, body) = describe::assemble(None, &task_record, &commit);
                self.open_pull_request_with(&task_record, &worktree, &commit, &base, &title, &body)
            }
            DescribeOutcome::HeldForUser => Ok(()),
        }
    }

    fn open_pull_request_with(
        &self,
        task_record: &Task,
        worktree: &Path,
        commit: &CommitId,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<()> {
        let task = task_record.id.clone();
        match self
            .delivery
            .open_pull_request(task_record, worktree, commit, base, title, body)
        {
            Ok((number, url)) => self.record(
                &event_key(&["pull_request_opened", task.as_str(), &number.to_string()]),
                Fact {
                    at: now(),
                    kind: FactKind::PullRequestOpened { task, number, url },
                },
            ),
            Err(error) => self.record_delivery_failure(&task, commit, &error.to_string()),
        }
    }

    fn describe_pull_request(
        &self,
        task: &Task,
        worktree: &Path,
        commit: &CommitId,
        config: &crate::config::ProjectConfig,
    ) -> Result<DescribeOutcome> {
        let Some(profile) = config
            .pull_request
            .describe_profile
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            return Ok(DescribeOutcome::OptOut);
        };
        let first = self.run_describer(task, worktree, config, profile);
        if let Ok(output) = first {
            return Ok(DescribeOutcome::Output(output));
        }
        log("describe_retry", task.id.as_str());
        match self.run_describer(task, worktree, config, profile) {
            Ok(output) => Ok(DescribeOutcome::Output(output)),
            Err(error) => {
                let reason = error.to_string();
                log("describe_failed", &reason);
                self.record(
                    &event_key(&[
                        "describe_failed",
                        task.id.as_str(),
                        commit.as_str(),
                        &now().millis().to_string(),
                    ]),
                    Fact {
                        at: now(),
                        kind: FactKind::DescribeFailed {
                            task: task.id.clone(),
                            reason,
                        },
                    },
                )?;
                Ok(DescribeOutcome::HeldForUser)
            }
        }
    }

    fn run_describer(
        &self,
        task: &Task,
        worktree: &Path,
        config: &crate::config::ProjectConfig,
        profile: &str,
    ) -> Result<describe::DescribeOutput> {
        let settings = self.store.home().load_settings()?;
        let spec = settings.profile_spec(depot_core::ProfileId::new(profile.to_owned()))?;
        let base = config.pull_request.base.clone();
        fetch_base(worktree, &base)?;
        let range = format!("origin/{base}...HEAD");
        let diff = git_output(worktree, &["diff", &range])?;
        let diffstat = git_output(worktree, &["diff", "--stat", &range])?;
        let describer = describe::SessionDescriber::new(
            &self.sessions,
            spec,
            Duration::from_secs(config.pull_request.describe_timeout_seconds),
        );
        describer.describe(&describe::DescribeInput {
            title: task.title.clone(),
            diff: describe::diff_section(&diff, &diffstat),
            style: config.pull_request.describe_style.clone(),
            directory: worktree.to_path_buf(),
            output_path: std::env::temp_dir().join(format!(
                "depot-describe-{}-{}.md",
                task.id,
                now().millis()
            )),
        })
    }

    fn release(&self, task: TaskId, lease: WorktreeLease) -> Result<()> {
        let repo = self.repository()?;
        let pool = self
            .worktrees
            .pool(&repo)
            .map_err(|error| Error::Project(error.to_string()))?;
        self.release_from(&task, &lease, &pool)
    }

    fn release_from(&self, task: &TaskId, lease: &WorktreeLease, pool: &[PoolEntry]) -> Result<()> {
        let entry = pool
            .iter()
            .find(|entry| entry.lease.as_ref() == Some(lease));
        let Some(entry) = entry else {
            return self.record_release(task, lease);
        };
        match self.worktrees.release(&Lease {
            lease: entry.lease.clone().expect("matched lease"),
            path: entry.path.clone(),
            holder: entry.holder.clone().unwrap_or_default(),
            acquired_at: String::new(),
        }) {
            Ok(()) => self.record_release(task, lease),
            Err(WorktreeError::UnlandedWork { reason, .. }) => {
                self.record_release_held(task, lease, &reason)
            }
            Err(error) => {
                log("worktree_release_failed", &error.to_string());
                self.record_release_held(task, lease, &error.to_string())
            }
        }
    }

    fn record_release(&self, task: &TaskId, lease: &WorktreeLease) -> Result<()> {
        let at = now();
        self.record(
            &event_key(&[
                "worktree_released",
                task.as_str(),
                lease.as_str(),
                &at.millis().to_string(),
            ]),
            Fact {
                at,
                kind: FactKind::WorktreeReleased {
                    task: task.clone(),
                    lease: lease.clone(),
                },
            },
        )
    }

    fn record_release_held(
        &self,
        task: &TaskId,
        lease: &WorktreeLease,
        reason: &str,
    ) -> Result<()> {
        let at = now();
        let attempt = self.task(task)?.attempts.len();
        self.record(
            &event_key(&[
                "worktree_release_held",
                task.as_str(),
                &attempt.to_string(),
                lease.as_str(),
                reason,
            ]),
            Fact {
                at,
                kind: FactKind::WorktreeReleaseHeld {
                    task: task.clone(),
                    lease: lease.clone(),
                    reason: reason.to_owned(),
                },
            },
        )
    }

    fn reconcile_leases(&self, sweep: bool) -> Result<()> {
        let tasks: Vec<Task> = self.store.tasks(&self.project.id)?.into_values().collect();
        let owed: Vec<&Task> = tasks
            .iter()
            .filter(|task| task.returns_its_worktree())
            .filter(|task| sweep || !task.release_pending.is_empty())
            .collect();
        if owed.is_empty() {
            return Ok(());
        }
        let repo = self.repository()?;
        let pool = match self.worktrees.pool(&repo) {
            Ok(pool) => pool,
            Err(error) => {
                log_project_error(&self.project.slug, &Error::Project(error.to_string()));
                return Ok(());
            }
        };
        for task in owed {
            let leases = if sweep {
                owed_leases(task, &pool)
            } else {
                task.release_pending.clone()
            };
            for lease in leases {
                if task.attempts.iter().any(|attempt| {
                    attempt.outcome.is_open() && attempt.worktree.as_ref() == Some(&lease)
                }) {
                    continue;
                }
                let held_recently = task.release_held.get(&lease).is_some_and(|hold| {
                    now().millis().saturating_sub(hold.at.millis()) < RELEASE_HOLD_BACKOFF_MILLIS
                });
                if held_recently {
                    continue;
                }
                if let Err(error) = self.release_from(&task.id, &lease, &pool) {
                    log_project_error(&self.project.slug, &error);
                }
            }
        }
        Ok(())
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
            let recorded = self.record(
                &key,
                Fact {
                    at: now(),
                    kind: FactKind::OnEventNotified {
                        task: task.id.clone(),
                        event: name.to_owned(),
                    },
                },
            );
            if let Err(error) = recorded {
                log_project_error(&self.project.slug, &error);
            }
        }
        Ok(())
    }

    fn reconcile_start(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.values() {
            let Some(attempt) = task.attempts.last() else {
                continue;
            };
            if task.state.in_flight() && attempt.outcome.is_open() && attempt.worktree.is_none() {
                self.deferring(&task.id, || {
                    self.admit(task.id.clone(), depot_core::worktree_baseline(task))
                })?;
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
                self.deferring(&task.id, || {
                    self.launch(task.id.clone(), attempt.profile.clone())
                })?;
            }
        }
        Ok(())
    }

    fn reconcile_resume(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.into_values() {
            let resumable = task
                .attempts
                .last()
                .is_some_and(|attempt| attempt.session.is_some())
                && matches!(
                    task.state,
                    TaskState::Running | TaskState::WaitingOnQuestion
                );
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
            self.deferring(&task.id, || self.resume(task.id.clone()))?;
        }
        Ok(())
    }

    fn session_turn_is_running(&self, task: &Task) -> Result<bool> {
        let session = self.session_for(task)?;
        Ok(matches!(
            self.sessions.status(&session),
            Ok(status) if status.state == crate::adapters::sessions::SessionState::Running
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

    fn lease_is_in_pool(&self, lease: &WorktreeLease) -> Result<bool> {
        let repo = self.repository()?;
        let pool = self
            .worktrees
            .pool(&repo)
            .map_err(|error| Error::Project(error.to_string()))?;
        Ok(pool
            .into_iter()
            .any(|entry| entry.lease.as_ref() == Some(lease)))
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

    fn answered_questions(&self, task: &TaskId) -> Result<Vec<Answered>> {
        let events = self.store.events(&self.project.id)?;
        let mut open: Vec<(u32, String, bool)> = Vec::new();
        let mut answered: Vec<Answered> = Vec::new();
        for event in events
            .iter()
            .filter(|event| event.task.as_ref() == Some(task))
        {
            match event.kind.as_str() {
                kind if kind == fact_tag_name(FactTag::QuestionAsked) => {
                    let occurrence = open.len() as u32;
                    open.push((occurrence, payload_field(&event.payload, "text")?, false));
                }
                kind if kind == fact_tag_name(FactTag::QuestionAnswered) => {
                    let answer = payload_field(&event.payload, "answer")?;
                    if let Some(question) =
                        open.iter_mut().rev().find(|(_, _, answered)| !*answered)
                    {
                        question.2 = true;
                        answered.push(Answered {
                            occurrence: question.0,
                            question: question.1.clone(),
                            answer,
                            event: event.id,
                        });
                    }
                }
                _ => {}
            }
        }
        answered.sort_by_key(|answered| answered.occurrence);
        Ok(answered)
    }

    fn answers_all(&self, task: &TaskId) -> Result<Vec<(String, String)>> {
        Ok(self
            .answered_questions(task)?
            .into_iter()
            .map(|answered| (answered.question, answered.answer))
            .collect())
    }

    fn answers_owed(&self, task: &TaskId) -> Result<Vec<(String, String)>> {
        let delivered = self.store.last_event_id(
            &self.project.id,
            task,
            fact_tag_name(FactTag::WorkerTurnStarted),
        )?;
        Ok(self
            .answered_questions(task)?
            .into_iter()
            .filter(|answered| delivered.is_none_or(|delivered| answered.event > delivered))
            .map(|answered| (answered.question, answered.answer))
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

    fn pending_redirect_event(&self, task: &TaskId) -> Result<Option<RecordedEvent>> {
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
        Ok(events
            .iter()
            .rev()
            .find(|event| {
                event.task.as_ref() == Some(task)
                    && event.kind == fact_tag_name(FactTag::WorkerRedirected)
            })
            .filter(|event| event.id > started)
            .cloned())
    }

    fn pending_redirect(&self, task: &TaskId) -> Result<Option<String>> {
        self.pending_redirect_event(task)?
            .map(|event| payload_field(&event.payload, "text"))
            .transpose()
    }

    fn reconcile_budget(&self) -> Result<()> {
        let run_duration = self.store.home().load_settings()?.run_duration();
        let at = now();
        for task in self.store.tasks(&self.project.id)?.into_values() {
            if !task.state.in_flight()
                || task.state == TaskState::WaitingOnQuestion
                || self.answer_owed(&task.id)?
                || self.pending_redirect(&task.id)?.is_some()
            {
                continue;
            }
            let Some(attempt) = task.attempts.last() else {
                continue;
            };
            if !attempt.outcome.is_open() {
                continue;
            }
            if at.millis().saturating_sub(attempt.started_at.millis())
                < run_duration.as_millis() as u64
            {
                continue;
            }
            self.record(
                &event_key(&[
                    "run_duration_exceeded",
                    task.id.as_str(),
                    &attempt.started_at.millis().to_string(),
                ]),
                Fact {
                    at,
                    kind: FactKind::RunDurationExceeded {
                        task: task.id.clone(),
                    },
                },
            )?;
        }
        Ok(())
    }

    fn reconcile_stops(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.into_values() {
            let Some(attempt) = task.attempts.last() else {
                continue;
            };
            if attempt.outcome != depot_core::AttemptOutcome::Stopped {
                continue;
            }
            let Some(session) = attempt.session.clone() else {
                continue;
            };
            if !self.session_is_running(&session)? {
                continue;
            }
            self.deferring(&task.id, || self.stop(task.id.clone()))?;
        }
        Ok(())
    }

    fn session_is_running(&self, session: &SessionId) -> Result<bool> {
        Ok(matches!(
            self.sessions.status(session),
            Ok(status) if status.state == crate::adapters::sessions::SessionState::Running
        ))
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
            let status = match self.sessions.status(&session) {
                Ok(status) => status,
                Err(error) => {
                    self.defer_turn(&task.id, &error.to_string())?;
                    continue;
                }
            };
            match status.state {
                crate::adapters::sessions::SessionState::Running => {
                    self.worker_liveness(task.id, Liveness::Live)?;
                }
                crate::adapters::sessions::SessionState::Failed if status.limit_hit => {
                    let profile = task
                        .attempts
                        .last()
                        .map(|attempt| attempt.profile.clone())
                        .ok_or_else(|| {
                            Error::Project(format!("task `{}` has no attempt", task.id))
                        })?;
                    self.record(
                        &event_key(&["provider_rate_limited", task.id.as_str(), session.as_str()]),
                        Fact {
                            at: now(),
                            kind: FactKind::ProviderRateLimited {
                                task: task.id.clone(),
                                profile,
                            },
                        },
                    )?;
                }
                crate::adapters::sessions::SessionState::Failed => {
                    let reason = status.error.or(status.capture_error).unwrap_or_else(|| {
                        "the worker session failed without reporting an error".to_owned()
                    });
                    self.record(
                        &event_key(&["worker_session_failed", task.id.as_str(), session.as_str()]),
                        Fact {
                            at: now(),
                            kind: FactKind::WorkerSessionFailed {
                                task: task.id.clone(),
                                reason,
                            },
                        },
                    )?;
                }
                _ => self.worker_liveness(task.id, Liveness::Gone)?,
            }
        }
        Ok(())
    }

    fn reconcile_validation(&self) -> Result<()> {
        for task in self.store.tasks(&self.project.id)?.into_values() {
            if task.state != TaskState::Validating {
                continue;
            }
            let commit = match self.submitted_commit(&task.id) {
                Ok(commit) => commit,
                Err(error) => {
                    let commit = task
                        .branch_head
                        .clone()
                        .or_else(|| task.validations.last().map(|record| record.commit.clone()))
                        .unwrap_or_else(|| CommitId::new(""));
                    self.record_validation_failure(&task.id, &commit, &error.to_string())?;
                    continue;
                }
            };
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
                    if task.hold_pr {
                        continue;
                    }
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

    fn reconcile_evidence(&self) -> Result<()> {
        let config = self.store.project_config(&self.project)?;
        let Some(command) = config
            .evidence
            .command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
        else {
            return Ok(());
        };
        let Ok(repo) = self.project_repo() else {
            return Ok(());
        };
        let state = self.store.project_state(&self.project)?;
        for task in state.tasks.values() {
            if task.state != TaskState::PrOpen || depot_core::publication_blocked(&state, &task.id)
            {
                continue;
            }
            let Some(commit) = task.validated_commit().cloned() else {
                continue;
            };
            let Some((number, _, _)) = task.pull_request() else {
                continue;
            };
            if self.evidence_recorded(&task.id, &commit)? {
                continue;
            }
            self.capture_evidence(task, &commit, number, &repo, command, &config)?;
        }
        Ok(())
    }

    fn evidence_recorded(&self, task: &TaskId, commit: &CommitId) -> Result<bool> {
        Ok(self
            .store
            .event(
                &self.project.id,
                &event_key(&["evidence", task.as_str(), commit.as_str()]),
            )?
            .is_some())
    }

    fn capture_evidence(
        &self,
        task: &Task,
        commit: &CommitId,
        number: u64,
        repo: &RepoSlug,
        command: &str,
        config: &crate::config::ProjectConfig,
    ) -> Result<()> {
        let key = event_key(&["evidence", task.id.as_str(), commit.as_str()]);
        let required = config.evidence.required;
        let result: std::result::Result<(), String> = (|| {
            let lease = self.lease_for(task).map_err(|error| error.to_string())?;
            let output = self
                .evidence
                .run(
                    command,
                    &lease.path,
                    &config.pull_request.base,
                    Duration::from_secs(config.evidence.timeout_seconds),
                )
                .map_err(|error| error.to_string())?;
            if output.exit_code != 0 {
                return Err(format!("the evidence command exited {}", output.exit_code));
            }
            self.publish_evidence(
                task,
                number,
                repo,
                &lease.path,
                &evidence::parse_manifest(&output.stdout),
                required,
            )
            .map_err(|error| error.to_string())
        })();
        match result {
            Ok(()) => self.record(
                &key,
                Fact {
                    at: now(),
                    kind: FactKind::EvidencePosted {
                        task: task.id.clone(),
                        commit: commit.clone(),
                    },
                },
            ),
            Err(reason) => {
                log("evidence_failed", &reason);
                self.record(
                    &key,
                    Fact {
                        at: now(),
                        kind: FactKind::EvidenceFailed {
                            task: task.id.clone(),
                            commit: commit.clone(),
                            reason,
                            required,
                        },
                    },
                )
            }
        }
    }

    fn publish_evidence(
        &self,
        task: &Task,
        number: u64,
        repo: &RepoSlug,
        worktree: &Path,
        artifacts: &[EvidenceArtifact],
        required: bool,
    ) -> Result<()> {
        let resolved = evidence::resolve_artifacts(worktree, artifacts);
        if required && !resolved.iter().any(ResolvedArtifact::is_url) {
            return Err(Error::Project(
                "the evidence command produced nothing attachable: a local file cannot be attached to a pull request, so publish each artifact at a URL"
                    .to_owned(),
            ));
        }
        let proof = evidence::render_proof(&resolved);
        let (title, existing) = self.delivery.pull_request_content(task, repo, number)?;
        let body = evidence::upsert_proof(&existing, &proof);
        self.delivery
            .update_pull_request(task, repo, number, &title, &body)
    }

    fn reconcile_forge(&self) -> Result<()> {
        let state = self.store.project_state(&self.project)?;
        let mut polled: BTreeMap<u64, Option<ObservedPullRequest>> = BTreeMap::new();
        let mut settled: BTreeSet<u64> = BTreeSet::new();
        let mut observed_open: Vec<(TaskId, u64, ObservedPullRequest)> = Vec::new();
        for task in state.tasks.values() {
            if !task.state.tracks_pull_request() {
                continue;
            }
            let Some((number, _, recorded)) = task.pull_request() else {
                continue;
            };
            let observed = match polled.get(&number) {
                Some(observed) => observed.clone(),
                None => {
                    let observed = match self
                        .project_repo()
                        .and_then(|repo| self.delivery.observe_pull_request(task, &repo))
                    {
                        Ok(observed) => observed,
                        Err(error) => {
                            log("forge_unavailable", &error.to_string());
                            None
                        }
                    };
                    polled.insert(number, observed.clone());
                    observed
                }
            };
            let Some(mut observed) = observed else {
                continue;
            };
            let at = now();
            match observed.state {
                PrState::Merged => {
                    if !settled.insert(number) {
                        continue;
                    }
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
                    if !settled.insert(number) {
                        continue;
                    }
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
                    if observed.mergeable == Some(false) {
                        match self.mergeability_against_fresh_base(task, &observed.commit) {
                            Ok((base, mergeable)) => {
                                observed.base = base;
                                observed.mergeable = Some(mergeable);
                            }
                            Err(error) => {
                                log("mergeability_check_failed", &error.to_string());
                                observed.mergeable = None;
                            }
                        }
                    }
                    if let Some(mergeable) = observed.mergeable {
                        let conflicting_base = (!mergeable).then(|| observed.base.clone());
                        if task.conflict_base != conflicting_base {
                            self.record(
                                &event_key(&[
                                    "pull_request_mergeability_changed",
                                    task.id.as_str(),
                                    if mergeable {
                                        "mergeable"
                                    } else {
                                        "conflicting"
                                    },
                                    observed.base.as_str(),
                                    &at.millis().to_string(),
                                ]),
                                Fact {
                                    at,
                                    kind: FactKind::PullRequestMergeabilityChanged {
                                        task: task.id.clone(),
                                        mergeable,
                                        base: observed.base.clone(),
                                    },
                                },
                            )?;
                        }
                    }
                    observed_open.push((task.id.clone(), number, observed));
                }
            }
        }
        self.file_review_tasks()?;
        self.schedule_base_merges(&observed_open)?;
        self.auto_merge(observed_open)
    }

    fn file_review_tasks(&self) -> Result<()> {
        let state = self.store.project_state(&self.project)?;
        if state.merge_policy != MergePolicy::AfterReview
            || !state.profiles.contains_key(&Role::Review)
        {
            return Ok(());
        }
        for task in state.tasks.values() {
            if task.state != TaskState::PrOpen {
                continue;
            }
            let Some(commit) = task.validated_commit() else {
                continue;
            };
            let already_filed = state.tasks.values().any(|other| {
                other.role == Role::Review
                    && other
                        .dependencies
                        .iter()
                        .any(|dependency| dependency.task == task.id)
            });
            if already_filed {
                continue;
            }
            let review = self.store.next_task_id(&self.project.id)?;
            let intent = format!(
                "Review the change of task `{}` on its delivery branch. Read the diff against the base branch and report findings on correctness, tests and risks.",
                task.id
            );
            let at = now();
            let applied = self.store.apply_facts(
                &self.project,
                &[
                    (
                        event_key(&["review_proposed", review.as_str(), commit.as_str()]),
                        Fact {
                            at,
                            kind: FactKind::TaskProposed {
                                task: review.clone(),
                                title: format!("Review {}", task.title),
                                intent,
                                role: Role::Review,
                                dispatch_profile: None,
                                dependencies: vec![Dependency {
                                    task: task.id.clone(),
                                    commit: commit.clone(),
                                }],
                                base_dependency: Some(task.id.clone()),
                                hold_pr: true,
                            },
                        },
                    ),
                    (
                        event_key(&["review_approved", review.as_str(), commit.as_str()]),
                        Fact {
                            at,
                            kind: FactKind::TaskApproved {
                                task: review.clone(),
                            },
                        },
                    ),
                ],
            )?;
            if applied.outcome == crate::store::EventOutcome::Recorded {
                log("review_filed", review.as_str());
                for action in applied.actions {
                    self.execute(action)?;
                }
            }
        }
        Ok(())
    }

    fn mergeability_against_fresh_base(
        &self,
        task: &Task,
        commit: &CommitId,
    ) -> Result<(CommitId, bool)> {
        let worktree = self.lease_for(task)?.path;
        let base = self.store.project_config(&self.project)?.pull_request.base;
        let fresh = fetch_base(&worktree, &base)?;
        let scratch = ScratchWorktree::add(&worktree, commit)?;
        let mergeable = merge_base(scratch.path(), &base)?.is_none();
        Ok((fresh, mergeable))
    }

    fn schedule_base_merges(
        &self,
        observed_open: &[(TaskId, u64, ObservedPullRequest)],
    ) -> Result<()> {
        let conflicting: Vec<&(TaskId, u64, ObservedPullRequest)> = observed_open
            .iter()
            .filter(|(_, _, observed)| observed.mergeable == Some(false))
            .collect();
        if conflicting.is_empty() {
            return Ok(());
        }
        let state = self.store.project_state(&self.project)?;
        for (id, _, observed) in conflicting {
            let Some(task) = state.tasks.get(id) else {
                continue;
            };
            let Some(profile) = depot_core::base_merge_due(&state, task, true) else {
                continue;
            };
            self.record(
                &event_key(&[
                    "rebase_scheduled",
                    id.as_str(),
                    observed.commit.as_str(),
                    observed.base.as_str(),
                ]),
                Fact {
                    at: now(),
                    kind: FactKind::RebaseScheduled {
                        task: id.clone(),
                        profile,
                        commit: observed.commit.clone(),
                        base: observed.base.clone(),
                    },
                },
            )?;
        }
        Ok(())
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
            if let Ok(repo) = self.project_repo()
                && let Err(error) = self.delivery.delete_branch(&repo, &observed.head_ref)
            {
                log("branch_delete_failed", &error.to_string());
            }
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
        crate::adapters::worktrees::resolve_lease(&self.worktrees, &self.repository()?, task)
            .map_err(|error| Error::Project(error.to_string()))
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

struct Answered {
    occurrence: u32,
    question: String,
    answer: String,
    event: u64,
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

fn owed_leases(task: &Task, pool: &[PoolEntry]) -> Vec<WorktreeLease> {
    let mut leases = task.release_pending.clone();
    let holder = format!("depot:{}", task.id);
    for entry in pool {
        if entry.holder.as_deref() == Some(holder.as_str())
            && let Some(lease) = &entry.lease
            && !leases.contains(lease)
        {
            leases.push(lease.clone());
        }
    }
    leases
}

fn log(kind: &str, value: &str) {
    eprintln!("{{\"kind\":\"{}\",\"value\":{:?}}}", kind, value);
}

pub(crate) fn logs_fact(kind: &FactKind) -> bool {
    fact_tag(kind) != FactTag::Polled
}

pub(crate) fn log_project_error(slug: &str, error: &Error) {
    log("project-error", &format!("{slug}: {error}"));
}

#[cfg(test)]
mod tests {
    use super::{
        EventHook, EventNotice, ShellEventHook, ShellValidation, ValidationRunner, delivery_branch,
        logs_fact, repo_slug,
    };
    use std::collections::BTreeMap;

    use depot_core::{
        CommitId, Fact, FactKind, Limits, MergePolicy, ProjectId, ProjectState, Role, Task, TaskId,
        Timestamp,
    };
    use tempfile::TempDir;

    #[test]
    fn polling_is_not_logged_and_every_other_fact_is() {
        assert!(!logs_fact(&FactKind::Polled));
        assert!(logs_fact(&FactKind::TaskApproved {
            task: TaskId::new("t-1"),
        }));
        assert!(logs_fact(&FactKind::TaskProposed {
            task: TaskId::new("t-1"),
            title: "a task".to_string(),
            intent: "why".to_string(),
            role: Role::Build,
            dispatch_profile: None,
            dependencies: Vec::new(),
            base_dependency: None,
            hold_pr: false,
        }));
    }

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

    fn task(id: &str, title: &str, intent: &str) -> Task {
        let state = ProjectState {
            project: ProjectId::new("test"),
            slug: "test".to_owned(),
            tasks: BTreeMap::new(),
            coordinator: None,
            profiles: BTreeMap::new(),
            fallback_profiles: Vec::new(),
            limits: Limits::default(),
            always_relay_questions: false,
            merge_policy: MergePolicy::Manual,
        };
        let (state, _) = depot_core::reduce(
            &state,
            &Fact {
                at: Timestamp::from_millis(0),
                kind: FactKind::TaskProposed {
                    task: TaskId::new(id),
                    title: title.to_owned(),
                    intent: intent.to_owned(),
                    role: Role::Build,
                    dispatch_profile: None,
                    dependencies: Vec::new(),
                    base_dependency: None,
                    hold_pr: false,
                },
            },
        );
        state
            .tasks
            .get(&TaskId::new(id))
            .cloned()
            .expect("the proposed task is in the state")
    }

    #[cfg(windows)]
    const PRINT_VALIDATED_AND_EXIT_7: &str = "<nul set /p=validated& exit 7";
    #[cfg(not(windows))]
    const PRINT_VALIDATED_AND_EXIT_7: &str = "printf validated; exit 7";

    #[test]
    fn validation_runs_at_the_submitted_commit_and_keeps_its_output() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("work");
        std::fs::create_dir_all(&path).expect("work directory");
        git(&path, &["init"]);
        git(&path, &["config", "user.email", "depot@example.test"]);
        git(&path, &["config", "user.name", "Depot"]);
        std::fs::write(path.join("answer"), "42").expect("fixture is written");
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", "fixture"]);
        let commit = CommitId::new(git(&path, &["rev-parse", "HEAD"]));
        let origin = temp.path().join("origin.git");
        git(
            temp.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                origin.to_str().expect("the origin is utf-8"),
            ],
        );
        git(
            &path,
            &[
                "remote",
                "add",
                "origin",
                origin.to_str().expect("the origin is utf-8"),
            ],
        );
        git(&path, &["push", "origin", "HEAD:main"]);
        let task = task("T1", "test", "test validation");
        let result = ShellValidation
            .validate(&task, &path, &commit, "main", PRINT_VALIDATED_AND_EXIT_7)
            .expect("validation runs");
        assert_eq!(result.exit_code, 7);
        assert_eq!(result.output_tail, "validated");
        assert_eq!(
            result.base_commit.as_ref().map(CommitId::as_str),
            Some(commit.as_str())
        );
        let wrong = CommitId::new("0000000000000000000000000000000000000000");
        assert!(
            ShellValidation
                .validate(&task, &path, &wrong, "main", "true")
                .is_err()
        );
    }

    #[test]
    fn a_detached_worktree_falls_back_to_a_conventional_delivery_branch() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path();
        git(path, &["init"]);
        git(path, &["config", "user.email", "depot@example.test"]);
        git(path, &["config", "user.name", "Depot"]);
        std::fs::write(path.join("answer"), "42").expect("fixture is written");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "fixture"]);
        git(path, &["checkout", "--detach", "HEAD"]);
        assert_eq!(git(path, &["branch", "--show-current"]), "");

        let task = task(
            "t-64",
            "Align branch naming",
            "Correct the branch name claims.",
        );

        let branch = delivery_branch(path, &task).expect("a detached worktree names a branch");
        assert_eq!(branch, "feat/align-branch-naming");
        assert!(
            !branch.starts_with("depot-"),
            "{branch} names the lease holder"
        );
        assert_eq!(git(path, &["branch", "--show-current"]), branch);
    }

    #[test]
    fn two_tasks_with_the_same_slug_get_distinct_delivery_branches() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path();
        git(path, &["init"]);
        git(path, &["config", "user.email", "depot@example.test"]);
        git(path, &["config", "user.name", "Depot"]);
        std::fs::write(path.join("answer"), "42").expect("fixture is written");
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "fixture"]);
        git(path, &["checkout", "--detach", "HEAD"]);

        let first = task(
            "t-64",
            "Align branch naming",
            "Correct the branch name claims.",
        );
        let branch = delivery_branch(path, &first).expect("the first task names a branch");
        assert_eq!(branch, "feat/align-branch-naming");

        git(path, &["checkout", "--detach", "HEAD"]);
        let second = task(
            "t-65",
            "Align branch naming",
            "Correct the branch name claims.",
        );
        let suffixed = delivery_branch(path, &second).expect("the second task names a branch");
        assert_eq!(suffixed, "feat/align-branch-naming-2");
        assert_ne!(branch, suffixed);
    }

    #[test]
    fn fetching_the_base_updates_a_stale_ref_without_a_local_branch() {
        let temp = TempDir::new().expect("temporary directory");
        let origin = temp.path().join("origin.git");
        git(
            temp.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                origin.to_str().expect("the origin is utf-8"),
            ],
        );
        let work = temp.path().join("work");
        std::fs::create_dir_all(&work).expect("work directory");
        git(&work, &["init", "--initial-branch=main"]);
        git(&work, &["config", "user.email", "depot@example.test"]);
        git(&work, &["config", "user.name", "Depot"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                origin.to_str().expect("the origin is utf-8"),
            ],
        );
        std::fs::write(work.join("base.txt"), "a").expect("the base file is written");
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "the first base"]);
        git(&work, &["push", "origin", "HEAD:main"]);
        git(&work, &["fetch", "origin", "main"]);
        git(&work, &["checkout", "--detach"]);
        git(&work, &["branch", "-D", "main"]);

        let second = temp.path().join("second");
        std::fs::create_dir_all(&second).expect("second directory");
        git(
            &second,
            &["clone", origin.to_str().expect("the origin is utf-8"), "."],
        );
        git(&second, &["config", "user.email", "depot@example.test"]);
        git(&second, &["config", "user.name", "Depot"]);
        std::fs::write(second.join("base.txt"), "b").expect("the base file is written");
        git(&second, &["add", "."]);
        git(&second, &["commit", "-m", "the base moves on"]);
        git(&second, &["push", "origin", "HEAD:main"]);
        let moved = git(&second, &["rev-parse", "HEAD"]);
        assert_ne!(
            git(&work, &["rev-parse", "refs/remotes/origin/main"]),
            moved,
            "the local remote-tracking ref starts stale"
        );

        let fetched = super::fetch_base(&work, "main").expect("the base is fetched");
        assert_eq!(fetched.as_str(), moved);
        assert_eq!(
            git(&work, &["rev-parse", "refs/remotes/origin/main"]),
            moved,
            "the fetch moves the remote-tracking ref to the current base"
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
