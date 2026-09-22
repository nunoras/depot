use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use depot_core::{AnsweredBy, CommitId, Dependency, Fact, FactKind, Role, Task, TaskId, TaskState};

use crate::adapters::process::Program;
use crate::adapters::sessions::{Boxr, Sessions};
use crate::adapters::worktrees::Worktrees;
use crate::clock::now;
use crate::config::PROJECT_CONFIG_FILE_NAME;
use crate::documents::write_document;
use crate::error::{Error, Result};
use crate::home::DepotHome;
use crate::inbox::{inbox_entries, render_inbox};
use crate::project::Project;
use crate::projects::{resolve_task, select_project};
use crate::store::{Applied, EventOutcome, Store, event_key};
use crate::vocabulary::{ROLE_NAMES, answered_by_from_name, role_from_name, role_name, state_name};

pub struct TaskRequest {
    pub title: String,
    pub intent: String,
    pub role: String,
    pub dependencies: Vec<String>,
    pub base_dependency: Option<String>,
    pub hold_pr: bool,
}

pub fn add_task(home: &DepotHome, selection: Option<&str>, request: &TaskRequest) -> Result<Task> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    let explicit_role = if request.role.is_empty() {
        None
    } else {
        let role = parse_role(&request.role)?;
        ensure_role_is_mapped(&store, &project, role)?;
        Some(role)
    };
    let dependencies = parse_dependencies(&request.dependencies)?;
    let base_dependency =
        resolve_base_dependency(request.base_dependency.as_deref(), &dependencies)?;
    let id = store.next_task_id(&project.id)?;
    let (role, dispatch_profile, judgement) = match explicit_role {
        Some(role) => (role, None, None),
        None => {
            let (resolution, judgement) = crate::dispatch::judge(&store, &project, &id, request)?;
            (resolution.role, Some(resolution.profile), Some(judgement))
        }
    };

    let fact = Fact {
        at: now(),
        kind: FactKind::TaskProposed {
            task: id.clone(),
            title: request.title.clone(),
            intent: request.intent.clone(),
            role,
            dispatch_profile,
            dependencies,
            base_dependency,
            hold_pr: request.hold_pr,
        },
    };
    let mut facts = Vec::new();
    if let Some(judgement) = judgement {
        facts.push((event_key(&["task_dispatch_judged", id.as_str()]), judgement));
    }
    facts.push((event_key(&["task_proposed", id.as_str()]), fact));
    if store.apply_facts(&project, &facts)?.outcome == EventOutcome::Duplicate {
        return Err(Error::Project(
            "another task was created concurrently; retry task creation".into(),
        ));
    }
    task(&store, &project, &id)
}

pub fn approve_tasks(
    home: &DepotHome,
    selection: Option<&str>,
    ids: &[String],
) -> Result<Vec<Task>> {
    let store = Store::open(home)?;
    let mut resolved = Vec::new();
    for id in ids {
        resolved.push(resolve_task(&store, selection, id)?);
    }
    let Some((project, _)) = resolved.first() else {
        return Err(Error::Project("no task ids were given".to_string()));
    };
    let project = project.clone();
    if resolved.iter().any(|(other, _)| other.id != project.id) {
        return Err(Error::Project(
            "approve tasks from one project at a time".to_string(),
        ));
    }
    ensure_profiles_resolve(home, &store, &project)?;
    let mut ready = Vec::new();
    for id in resolved.into_iter().map(|(_, id)| id) {
        let current = task(&store, &project, &id)?;
        let plan = prepare_approve(&current)?;
        ready.push((current, plan));
    }
    let mut approved = Vec::new();
    for (current, plan) in ready {
        let id = current.id.clone();
        if plan == Prepared::Apply {
            let fact = Fact {
                at: now(),
                kind: FactKind::TaskApproved { task: id.clone() },
            };
            apply(
                &store,
                &project,
                &["task_approved", id.as_str()],
                &fact,
                &format!(
                    "the `task_approved` fact is already recorded for task `{id}`; nothing changed"
                ),
            )?;
        }
        approved.push(task(&store, &project, &id)?);
    }
    Ok(approved)
}

pub fn ask_question(
    home: &DepotHome,
    selection: Option<&str>,
    id: &str,
    text: &str,
    relay: bool,
) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    if !matches!(
        current.state,
        TaskState::Running | TaskState::WaitingOnQuestion
    ) {
        return Err(transition_refused(&current, "asked a question"));
    }
    let relay = relay || store.project_config(&project)?.questions.always_relay;
    let at = now();
    let fact = Fact {
        at,
        kind: FactKind::QuestionAsked {
            task: id.clone(),
            text: text.to_owned(),
            relay,
        },
    };
    let applied = apply_repeatable(&store, &project, &["question_asked", id.as_str()], &fact)?;
    if applied
        .actions
        .iter()
        .any(|action| matches!(action, depot_core::Action::Notify { task } if task == &id))
    {
        eprintln!("depot: task {id} is waiting on a question");
    }
    task(&store, &project, &id)
}

pub fn submit_task(
    home: &DepotHome,
    selection: Option<&str>,
    id: &str,
    worktrees: &impl Worktrees,
) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    if current.state != TaskState::Running {
        return Err(transition_refused(&current, "submitted"));
    }
    let worktree = leased_worktree_path(&store, worktrees, &project, &current)?;
    let worktree = std::fs::canonicalize(&worktree).map_err(Error::Io)?;
    let here = std::env::current_dir()
        .map_err(Error::Io)
        .and_then(|here| std::fs::canonicalize(&here).map_err(Error::Io))?;
    if !here.starts_with(&worktree) {
        return Err(Error::Project(format!(
            "depot submit must run in the task's leased worktree at {}; the current directory is {}",
            worktree.display(),
            here.display()
        )));
    }
    let commit = CommitId::new(git_in(&worktree, &["rev-parse", "HEAD"])?);
    check_descends_from_base(&store, &project, &current, &worktree, &commit)?;
    let base = submission_base(&store, &project, &worktree)?;
    let fact = Fact {
        at: now(),
        kind: FactKind::WorkerSubmitted {
            task: id.clone(),
            commit: commit.clone(),
            base,
        },
    };
    apply(
        &store,
        &project,
        &["worker_submitted", id.as_str(), commit.as_str()],
        &fact,
        &format!(
            "the `worker_submitted` fact is already recorded for task `{id}` and commit `{commit}`; nothing changed"
        ),
    )?;
    task(&store, &project, &id)
}

pub fn answer_question(
    home: &DepotHome,
    selection: Option<&str>,
    id: &str,
    answer: &str,
    by: &str,
) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    let position = prepare_answer(&current)?;
    let by = parse_answered_by(by)?;
    let fact = Fact {
        at: now(),
        kind: FactKind::QuestionAnswered {
            task: id.clone(),
            answer: answer.to_string(),
            by,
        },
    };
    apply(
        &store,
        &project,
        &["question_answered", id.as_str(), &position.to_string()],
        &fact,
        &format!(
            "the `question_answered` fact is already recorded for task `{id}`; nothing changed"
        ),
    )?;
    task(&store, &project, &id)
}

pub fn acknowledge_task(home: &DepotHome, selection: Option<&str>, id: &str) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    match (current.state, current.acknowledged_at) {
        (TaskState::Failed | TaskState::Cancelled, Some(_)) => return Ok(current),
        (TaskState::Failed | TaskState::Cancelled, None) => {}
        _ => return Err(transition_refused(&current, "acknowledged")),
    }
    let fact = Fact {
        at: now(),
        kind: FactKind::TaskAcknowledged { task: id.clone() },
    };
    apply_repeatable(&store, &project, &["task_acknowledged", id.as_str()], &fact)?;
    task(&store, &project, &id)
}

pub fn retry_task(home: &DepotHome, selection: Option<&str>, id: &str) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    if !matches!(current.state, TaskState::Failed | TaskState::Cancelled) {
        return Err(transition_refused(&current, "retried"));
    }
    let fact = Fact {
        at: now(),
        kind: FactKind::TaskRetried { task: id.clone() },
    };
    apply_repeatable(&store, &project, &["task_retried", id.as_str()], &fact)?;
    task(&store, &project, &id)
}

pub fn rework_task(
    home: &DepotHome,
    selection: Option<&str>,
    id: &str,
    text: &str,
) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    if current.state != TaskState::PrOpen {
        return Err(transition_refused(&current, "reworked"));
    }
    ensure_role_is_mapped(&store, &project, Role::Fix)?;
    let fix = store.next_task_id(&project.id)?;
    let fact = Fact {
        at: now(),
        kind: FactKind::TaskReworked {
            task: id.clone(),
            fix: fix.clone(),
            text: text.to_owned(),
        },
    };
    apply(
        &store,
        &project,
        &["task_reworked", id.as_str(), fix.as_str()],
        &fact,
        &format!("the `task_reworked` fact is already recorded for task `{id}`; nothing changed"),
    )?;
    task(&store, &project, &id)
}

pub fn stop_task(home: &DepotHome, selection: Option<&str>, id: &str) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    if prepare_stop(&current)? == Prepared::Apply {
        let fact = Fact {
            at: now(),
            kind: FactKind::TaskCancelled { task: id.clone() },
        };
        apply_repeatable(&store, &project, &["task_cancelled", id.as_str()], &fact)?;
    }
    task(&store, &project, &id)
}

pub fn redirect_task(
    home: &DepotHome,
    selection: Option<&str>,
    id: &str,
    text: &str,
    queue: bool,
) -> Result<(Task, bool)> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    if current.state != TaskState::Running {
        return Err(transition_refused(&current, "redirected"));
    }
    let turn_running = turn_is_running(&current);
    if !turn_running && !queue {
        return Err(Error::Project(format!(
            "task `{}` has no running worker turn; pass `--queue` to deliver it at the next turn",
            current.id
        )));
    }
    let fact = Fact {
        at: now(),
        kind: FactKind::WorkerRedirected {
            task: id.clone(),
            text: text.to_owned(),
        },
    };
    apply_repeatable(&store, &project, &["worker_redirected", id.as_str()], &fact)?;
    Ok((task(&store, &project, &id)?, turn_running))
}

pub fn release_task(home: &DepotHome, selection: Option<&str>, id: &str) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let current = task(&store, &project, &id)?;
    if !current.hold_pr && matches!(current.state, TaskState::PrOpen | TaskState::Landed) {
        return Ok(current);
    }
    if current.state != TaskState::Validated || !current.hold_pr {
        return Err(transition_refused(&current, "released"));
    }
    if current.hold_pr {
        let fact = Fact {
            at: now(),
            kind: FactKind::TaskReleased { task: id.clone() },
        };
        apply_repeatable(&store, &project, &["task_released", id.as_str()], &fact)?;
    }
    task(&store, &project, &id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waited {
    pub task: Task,
    pub timed_out: bool,
}

pub const MIN_WAIT_POLL: Duration = Duration::from_secs(1);

pub fn wait_for_task(
    home: &DepotHome,
    selection: Option<&str>,
    id: &str,
    timeout: Option<Duration>,
    poll: Duration,
) -> Result<Waited> {
    let store = Store::open(home)?;
    let (project, id) = resolve_task(&store, selection, id)?;
    let poll = poll.max(MIN_WAIT_POLL);
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    loop {
        let found = task(&store, &project, &id)?;
        if found.state.settles_a_wait() {
            return Ok(Waited {
                task: found,
                timed_out: false,
            });
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(Waited {
                task: found,
                timed_out: true,
            });
        }
        std::thread::sleep(poll);
    }
}

pub fn read_inbox(home: &DepotHome, selection: Option<&str>) -> Result<String> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    let cursor = store.inbox_cursor(&project.id)?;
    let events = store.events_since(&project.id, cursor)?;
    let next = events.last().map(|event| event.id).unwrap_or(cursor);
    let entries = inbox_entries(&project.slug, &store.tasks(&project.id)?, &events)?;
    store.set_inbox_cursor(&project.id, next)?;
    Ok(render_inbox(&entries))
}

pub fn write_narrative(
    home: &DepotHome,
    selection: Option<&str>,
    name: &str,
    content: &str,
) -> Result<PathBuf> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    write_document(&home.project_home(&project.slug), name, content)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prepared {
    Apply,
    AlreadyDone,
}

fn prepare_approve(task: &Task) -> Result<Prepared> {
    match task.state {
        TaskState::Proposed => Ok(Prepared::Apply),
        TaskState::Approved => Ok(Prepared::AlreadyDone),
        TaskState::Failed | TaskState::Cancelled => Err(Error::Project(format!(
            "task `{}` is {} and cannot be approved; run `depot task retry {}` to send it back to the approved queue",
            task.id,
            state_name(task.state),
            task.id
        ))),
        _ => Err(transition_refused(task, "approved")),
    }
}

fn prepare_stop(task: &Task) -> Result<Prepared> {
    match task.state {
        TaskState::Cancelled => Ok(Prepared::AlreadyDone),
        TaskState::Landed => Err(transition_refused(task, "stopped")),
        _ => Ok(Prepared::Apply),
    }
}

fn prepare_answer(task: &Task) -> Result<usize> {
    task.questions
        .iter()
        .rposition(|question| question.answer.is_none())
        .ok_or_else(|| {
            Error::Project(format!(
                "task `{}` is {} and has no unanswered question",
                task.id,
                state_name(task.state)
            ))
        })
}

fn transition_refused(task: &Task, action: &str) -> Error {
    Error::Project(format!(
        "task `{}` is {} and cannot be {action}",
        task.id,
        state_name(task.state)
    ))
}

fn leased_worktree_path(
    store: &Store,
    worktrees: &impl Worktrees,
    project: &Project,
    task: &Task,
) -> Result<PathBuf> {
    store.ensure_clone_origin(project)?;
    let repo = store.project_path(project)?.ok_or_else(|| {
        Error::Project(format!(
            "project `{}` has no local clone to submit from",
            project.slug
        ))
    })?;
    Ok(
        crate::adapters::worktrees::resolve_lease(worktrees, &repo, task)
            .map_err(|error| Error::Project(error.to_string()))?
            .path,
    )
}

fn submission_base(store: &Store, project: &Project, worktree: &Path) -> Result<Option<CommitId>> {
    let base = store.project_config(project)?.pull_request.base.clone();
    let remote = format!("refs/remotes/origin/{base}");
    Ok(git_in(worktree, &["rev-parse", "--verify", &remote])
        .ok()
        .map(CommitId::new))
}

fn check_descends_from_base(
    store: &Store,
    project: &Project,
    task: &Task,
    worktree: &Path,
    commit: &CommitId,
) -> Result<()> {
    match depot_core::worktree_baseline(task) {
        depot_core::Baseline::PinnedCommit(base) => {
            if !is_ancestor(worktree, base.as_str(), commit.as_str())? {
                return Err(Error::Project(format!(
                    "the submitted commit {commit} is not a descendant of the task's base {base}"
                )));
            }
        }
        depot_core::Baseline::DefaultBranchHead => {
            let base = store.project_config(project)?.pull_request.base;
            let remote = format!("origin/{base}");
            if git_in(worktree, &["rev-parse", "--verify", &remote]).is_ok()
                && git_in(worktree, &["merge-base", "HEAD", &remote]).is_err()
            {
                return Err(Error::Project(format!(
                    "the worktree {} shares no history with the task's base `{base}`",
                    worktree.display()
                )));
            }
        }
    }
    Ok(())
}

fn git_in(directory: &Path, arguments: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(Error::Io)?;
    if !output.status.success() {
        return Err(Error::Project(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn is_ancestor(directory: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(Error::Io)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::Project(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )),
    }
}

fn turn_is_running(task: &Task) -> bool {
    let Some(session) = task
        .attempts
        .last()
        .and_then(|attempt| attempt.session.clone())
    else {
        return false;
    };
    Boxr::new(Program::new("boxr"))
        .status(&session)
        .is_ok_and(|status| status.state == crate::adapters::sessions::SessionState::Running)
}

fn apply(
    store: &Store,
    project: &Project,
    parts: &[&str],
    fact: &Fact,
    duplicate: &str,
) -> Result<()> {
    let applied = store.apply_fact(project, &event_key(parts), fact)?;
    if applied.outcome == EventOutcome::Duplicate {
        return Err(Error::Project(duplicate.to_owned()));
    }
    Ok(())
}

fn apply_repeatable(
    store: &Store,
    project: &Project,
    parts: &[&str],
    fact: &Fact,
) -> Result<Applied> {
    let at = fact.at.millis().to_string();
    let mut key = parts.to_vec();
    key.push(&at);
    let applied = store.apply_fact(project, &event_key(&key), fact)?;
    if applied.outcome == EventOutcome::Duplicate {
        return Err(Error::Project(format!(
            "the `{}` fact was already recorded by another invocation",
            parts[0]
        )));
    }
    Ok(applied)
}

fn task(store: &Store, project: &Project, id: &TaskId) -> Result<Task> {
    store
        .task(&project.id, id)?
        .ok_or_else(|| Error::NotFound(format!("no task `{id}` in project `{}`", project.slug)))
}

pub fn ensure_profiles_resolve(home: &DepotHome, store: &Store, project: &Project) -> Result<()> {
    let settings = home.load_settings()?;
    let missing = settings.missing_profiles(&store.project_config(project)?)?;
    if missing.is_empty() {
        return Ok(());
    }
    let listed = missing
        .iter()
        .map(|(role, profile)| format!("role `{}` maps to profile `{profile}`", role_name(*role)))
        .collect::<Vec<_>>()
        .join("; ");
    Err(Error::Config(format!(
        "{listed}, and none of those profiles is defined in the machine-local settings at {}: define them under [profiles] there or map the roles to profiles that exist",
        home.config_path().display()
    )))
}

fn ensure_role_is_mapped(store: &Store, project: &Project, role: Role) -> Result<()> {
    if store
        .project_config(project)?
        .profiles()?
        .contains_key(&role)
    {
        return Ok(());
    }
    let name = role_name(role);
    Err(Error::Config(format!(
        "role `{name}` has no profile in {PROJECT_CONFIG_FILE_NAME} for project `{}`: add `{name} = \"<profile>\"` under [profiles]",
        project.slug
    )))
}

fn parse_role(name: &str) -> Result<Role> {
    role_from_name(name).ok_or_else(|| {
        Error::Config(format!(
            "unknown role `{name}`: expected one of {}",
            ROLE_NAMES.join(", ")
        ))
    })
}

fn parse_answered_by(name: &str) -> Result<AnsweredBy> {
    answered_by_from_name(name).ok_or_else(|| {
        Error::Config(format!(
            "unknown answerer `{name}`: expected `coordinator` or `user`"
        ))
    })
}

fn parse_dependencies(items: &[String]) -> Result<Vec<Dependency>> {
    items
        .iter()
        .map(|item| {
            let (task, commit) = item.split_once('@').ok_or_else(|| {
                Error::Config(format!(
                    "`{item}` is not a dependency: write it as `<task>@<commit>`"
                ))
            })?;
            Ok(Dependency {
                task: TaskId::new(task),
                commit: CommitId::new(commit),
            })
        })
        .collect()
}

fn resolve_base_dependency(
    base: Option<&str>,
    dependencies: &[Dependency],
) -> Result<Option<TaskId>> {
    match base {
        None if dependencies.len() > 1 => Err(Error::Config(
            "multiple --depends-on need --base-dependency <task-id> naming one of them".to_string(),
        )),
        None => Ok(None),
        Some(name) => {
            let base = TaskId::new(name);
            if dependencies
                .iter()
                .any(|dependency| dependency.task == base)
            {
                Ok(Some(base))
            } else {
                Err(Error::Config(format!(
                    "`--base-dependency {name}` must name one of the --depends-on tasks"
                )))
            }
        }
    }
}
