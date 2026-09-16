use std::path::PathBuf;

use depot_core::{AnsweredBy, CommitId, Dependency, Fact, FactKind, Role, Task, TaskId};

use crate::clock::now;
use crate::config::PROJECT_CONFIG_FILE_NAME;
use crate::documents::write_document;
use crate::error::{Error, Result};
use crate::home::DepotHome;
use crate::inbox::{inbox_entries, render_inbox};
use crate::project::Project;
use crate::projects::select_project;
use crate::store::{Store, event_key};
use crate::vocabulary::{ROLE_NAMES, answered_by_from_name, role_from_name, role_name};

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
    let mut approved = Vec::new();
    for id in ids {
        let id = TaskId::new(id);
        task(&store, &project, &id)?;
        let fact = Fact {
            at: now(),
            kind: FactKind::TaskApproved { task: id.clone() },
        };
        apply(&store, &project, &["task_approved", id.as_str()], &fact)?;
        approved.push(task(&store, &project, &id)?);
    }
    Ok(approved)
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
    if !current.questions.iter().any(|q| q.answer.is_none()) {
        return Err(Error::Project(format!(
            "task `{id}` has no unanswered question"
        )));
    }
    let by = parse_answered_by(by)?;
    let position = current.questions.len();
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
    task(&store, &project, &id)?;
    let fact = Fact {
        at: now(),
        kind: FactKind::TaskCancelled { task: id.clone() },
    };
    apply(&store, &project, &["task_cancelled", id.as_str()], &fact)?;
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
            "multiple --depends-on need --base-dependency <task-id> naming one of them"
                .to_string(),
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
