use std::path::PathBuf;
use std::process::Command;

use depot_core::{AnsweredBy, CommitId, Dependency, Fact, FactKind, Role, Task, TaskId, TaskState};

use crate::clock::now;
use crate::config::PROJECT_CONFIG_FILE_NAME;
use crate::documents::write_document;
use crate::error::{Error, Result};
use crate::home::DepotHome;
use crate::inbox::{inbox_entries, render_inbox};
use crate::project::Project;
use crate::projects::select_project;
use crate::store::{Store, event_key};
use crate::vocabulary::{ROLE_NAMES, answered_by_from_name, role_from_name, role_name, state_name};

pub struct TaskRequest {
    pub title: String,
    pub intent: String,
    pub role: String,
    pub dependencies: Vec<String>,
    pub base_dependency: Option<String>,
}

pub fn add_task(home: &DepotHome, selection: Option<&str>, request: &TaskRequest) -> Result<Task> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    let role = parse_role(&request.role)?;
    ensure_role_is_mapped(&store, &project, role)?;
    let dependencies = parse_dependencies(&request.dependencies)?;
    let base_dependency =
        resolve_base_dependency(request.base_dependency.as_deref(), &dependencies)?;
    let id = store.next_task_id(&project.id)?;

    let fact = Fact {
        at: now(),
        kind: FactKind::TaskProposed {
            task: id.clone(),
            title: request.title.clone(),
            intent: request.intent.clone(),
            role,
            dependencies,
            base_dependency,
        },
    };
    apply(&store, &project, &["task_proposed", id.as_str()], &fact)?;
    task(&store, &project, &id)
}

pub fn approve_tasks(
    home: &DepotHome,
    selection: Option<&str>,
    ids: &[String],
) -> Result<Vec<Task>> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    let mut ready = Vec::new();
    for id in ids {
        let id = TaskId::new(id);
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
            apply(&store, &project, &["task_approved", id.as_str()], &fact)?;
        }
        approved.push(task(&store, &project, &id)?);
    }
    Ok(approved)
}

pub fn worker_ask(home: &DepotHome, text: &str) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, current) = worker_task(&store)?;
    if current.state != TaskState::Running {
        return Err(transition_refused(&current, "asked"));
    }
    let fact = Fact {
        at: now(),
        kind: FactKind::QuestionAsked {
            task: current.id.clone(),
            text: text.to_owned(),
            relay: true,
        },
    };
    apply(
        &store,
        &project,
        &["question_asked", current.id.as_str()],
        &fact,
    )?;
    task(&store, &project, &current.id)
}

pub fn worker_submit(home: &DepotHome, summary: &str, artifacts: Vec<String>) -> Result<Task> {
    let store = Store::open(home)?;
    let (project, current) = worker_task(&store)?;
    if current.state != TaskState::Running {
        return Err(transition_refused(&current, "submitted"));
    }
    let commit = worker_commit()?;
    let recorded = Fact {
        at: now(),
        kind: FactKind::WorkerSubmissionRecorded {
            task: current.id.clone(),
            summary: summary.to_owned(),
            artifacts,
        },
    };
    apply(
        &store,
        &project,
        &["worker_submission_recorded", current.id.as_str()],
        &recorded,
    )?;
    let submitted = Fact {
        at: now(),
        kind: FactKind::WorkerSubmitted {
            task: current.id.clone(),
            commit,
        },
    };
    apply(
        &store,
        &project,
        &["worker_submitted", current.id.as_str()],
        &submitted,
    )?;
    task(&store, &project, &current.id)
}

pub fn answer_question(
    home: &DepotHome,
    selection: Option<&str>,
    id: &str,
    answer: &str,
    by: &str,
) -> Result<Task> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    let id = TaskId::new(id);
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
    )?;
    task(&store, &project, &id)
}

pub fn stop_task(home: &DepotHome, selection: Option<&str>, id: &str) -> Result<Task> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    let id = TaskId::new(id);
    let current = task(&store, &project, &id)?;
    if prepare_stop(&current)? == Prepared::Apply {
        let fact = Fact {
            at: now(),
            kind: FactKind::TaskCancelled { task: id.clone() },
        };
        apply(&store, &project, &["task_cancelled", id.as_str()], &fact)?;
    }
    task(&store, &project, &id)
}

pub fn read_inbox(home: &DepotHome, selection: Option<&str>) -> Result<String> {
    let store = Store::open(home)?;
    let project = select_project(&store, selection)?;
    let cursor = store.inbox_cursor(&project.id)?;
    let events = store.events_since(&project.id, cursor)?;
    let next = events.last().map(|event| event.id).unwrap_or(cursor);
    let entries = inbox_entries(&store.tasks(&project.id)?, &events)?;
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

fn worker_task(store: &Store) -> Result<(Project, Task)> {
    let task_id = std::env::var("DEPOT_TASK_ID")
        .map_err(|_| Error::Project("worker context is missing DEPOT_TASK_ID".to_string()))?;
    let attempt_id = std::env::var("DEPOT_ATTEMPT_ID")
        .map_err(|_| Error::Project("worker context is missing DEPOT_ATTEMPT_ID".to_string()))?;
    let id = TaskId::new(task_id);
    let matches = store
        .projects()?
        .into_iter()
        .filter_map(|project| {
            store
                .task(&project.id, &id)
                .ok()
                .flatten()
                .and_then(|task| {
                    task.attempts
                        .iter()
                        .any(|attempt| {
                            attempt
                                .worktree
                                .as_ref()
                                .is_some_and(|lease| lease.as_str() == attempt_id)
                        })
                        .then_some((project, task))
                })
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [(project, task)] => Ok((project.clone(), task.clone())),
        [] => Err(Error::NotFound(format!(
            "worker context names unknown task `{id}` or attempt `{attempt_id}`"
        ))),
        _ => Err(Error::Project(format!(
            "worker context for task `{id}` and attempt `{attempt_id}` is ambiguous"
        ))),
    }
}

fn worker_commit() -> Result<CommitId> {
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !output.status.success() {
        return Err(Error::Project(
            "worker submit needs a commit at HEAD".to_string(),
        ));
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if commit.is_empty() {
        return Err(Error::Project(
            "worker submit needs a commit at HEAD".to_string(),
        ));
    }
    Ok(CommitId::new(commit))
}

fn prepare_approve(task: &Task) -> Result<Prepared> {
    match task.state {
        TaskState::Proposed => Ok(Prepared::Apply),
        TaskState::Approved => Ok(Prepared::AlreadyDone),
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

fn apply(store: &Store, project: &Project, parts: &[&str], fact: &Fact) -> Result<()> {
    store.apply_fact(project, &event_key(parts), fact)?;
    Ok(())
}

fn task(store: &Store, project: &Project, id: &TaskId) -> Result<Task> {
    store
        .task(&project.id, id)?
        .ok_or_else(|| Error::NotFound(format!("no task `{id}` in project `{}`", project.slug)))
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
