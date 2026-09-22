use std::path::PathBuf;

use depot_core::{ProjectState, Role, Task};

use crate::config::ProjectConfig;
use crate::documents::write_document;
use crate::error::{Error, Result};
use crate::home::ProjectHome;
use crate::project::Project;
use crate::project_file::ProjectFile;
use crate::store::Store;
use crate::vocabulary::role_name;

pub const COORDINATOR_POLICY: &str = include_str!("../../../assets/coordinator-policy.md");
pub const COORDINATOR_KICKOFF_TEMPLATE: &str =
    include_str!("../../../assets/coordinator-kickoff.md");
pub const BRIEF_TEMPLATE: &str = include_str!("../../../assets/brief-template.md");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub policy: &'static str,
    pub kickoff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLaunch {
    pub brief_path: PathBuf,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoordinatorContext {
    pub project: Project,
    pub home: ProjectHome,
    pub config: ProjectConfig,
    pub project_file: Option<ProjectFile>,
    pub state: ProjectState,
}

impl Store {
    pub fn coordinator_context(&self, project: &Project) -> Result<CoordinatorContext> {
        let project_file = match self.project_path(project)? {
            Some(repo) => Some(ProjectFile::read_for_base(&repo)?),
            None => None,
        };
        Ok(CoordinatorContext {
            project: project.clone(),
            home: self.home().project_home(&project.slug),
            config: self.project_config(project)?,
            project_file,
            state: self.project_state(project)?,
        })
    }
}

impl CoordinatorContext {
    pub fn checklist_path(&self) -> PathBuf {
        self.home.checklist_path()
    }

    pub fn context_document_path(&self) -> PathBuf {
        self.home.context_document_path()
    }

    pub fn launch(&self) -> Result<Launch> {
        let project = self.project.slug.clone();
        let store = display(self.home.root());
        let checklist = display(&self.checklist_path());
        let context_document = display(&self.context_document_path());

        Ok(Launch {
            policy: COORDINATOR_POLICY,
            kickoff: render_template(
                COORDINATOR_KICKOFF_TEMPLATE,
                &[
                    ("project", &project),
                    ("store", &store),
                    ("checklist", &checklist),
                    ("context_document", &context_document),
                ],
            )?,
        })
    }

    pub fn worker_launch(&self, task: &Task) -> Result<WorkerLaunch> {
        let prompt = self.brief(task)?;
        let brief_path = write_document(
            &self.home,
            &format!("brief-{}.md", task.id.as_str()),
            &prompt,
        )?;
        Ok(WorkerLaunch { brief_path, prompt })
    }

    pub fn brief(&self, task: &Task) -> Result<String> {
        self.render_brief(task, &task.title, &task.intent)
    }

    pub fn conflict_brief(&self, task: &Task) -> Result<String> {
        let title = format!("Resolve conflicts: {}", task.title);
        let intent = "The task's open pull request conflicts with the project base branch. In \
             your worktree, fetch the base branch and merge it into the delivery branch, resolve \
             the conflicts, and submit the merged result. Merge the base in, never rebase, and \
             never force push: the pull request is open, and rewriting its history discards the \
             commits already published there. Keep the change as it was submitted; do not \
             extend it."
            .to_string();
        self.render_brief(task, &title, &intent)
    }

    fn render_brief(&self, task: &Task, title: &str, intent: &str) -> Result<String> {
        let output = output_destination(task, &self.home);
        let done = done_criteria(task.role);
        let dependencies = dependency_lines(&self.state, task);
        let validation = validation_note(self.project_file.as_ref());
        let worker_context = worker_context(task)?;
        let store = display(self.home.root());
        let checklist = display(&self.checklist_path());
        let scratch = display(&self.home.scratch_dir());
        let ask = format!(
            "depot ask --task {} --project {} --relay \"<question>\"",
            task.id, self.project.id
        );
        let submit = format!(
            "depot submit --task {} --project {}",
            task.id, self.project.id
        );

        render_template(
            BRIEF_TEMPLATE,
            &[
                ("title", title),
                ("task", task.id.as_str()),
                ("project", self.project.id.as_str()),
                ("role", role_name(task.role)),
                ("intent", intent),
                ("output", &output),
                ("done", done),
                ("dependencies", &dependencies),
                ("validation", &validation),
                ("worker_context", &worker_context),
                ("store", &store),
                ("checklist", &checklist),
                ("scratch", &scratch),
                ("ask", &ask),
                ("submit", &submit),
            ],
        )
    }
}

pub fn render_template(template: &str, values: &[(&str, &str)]) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let close = after.find("}}").ok_or_else(|| {
            Error::Template(format!(
                "a placeholder is opened at byte {open} and never closed"
            ))
        })?;
        let name = after[..close].trim();
        let value = values
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .ok_or_else(|| {
                Error::Template(format!("the template names `{name}`, which nothing fills"))
            })?;
        out.push_str(value.1);
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn worker_context(task: &Task) -> Result<String> {
    let attempt = task
        .attempts
        .last()
        .and_then(|attempt| attempt.worktree.as_ref())
        .ok_or_else(|| Error::Project(format!("task `{}` has no worker attempt", task.id)))?;
    Ok(format!(
        "DEPOT_TASK_ID={}\nDEPOT_ATTEMPT_ID={attempt}",
        task.id
    ))
}

fn output_destination(task: &Task, home: &ProjectHome) -> String {
    match task.role {
        Role::Plan | Role::Review => format!(
            "A document in the store, under `{}`.",
            display(&home.documents_dir())
        ),
        Role::Build | Role::Fix => {
            match task.attempts.last().and_then(|last| last.worktree.as_ref()) {
                Some(lease) => {
                    format!("This task's worktree, lease `{lease}`: commit the change there.")
                }
                None => "This task's worktree: commit the change there.".to_string(),
            }
        }
    }
}

fn done_criteria(role: Role) -> &'static str {
    match role {
        Role::Plan => {
            "A plan written to the store that names the tasks to create, each with its role and \
             its dependencies, and the decisions behind them. No project code changes."
        }
        Role::Build => {
            "The change committed in the worktree, then the assignment declared finished. The \
             daemon validates the commit and publishes it; the change is not finished when the \
             code merely works on your machine."
        }
        Role::Review => {
            "A verdict written to the store that says whether the change is correct, what must \
             change, and the exact commit it reviewed. No project code changes."
        }
        Role::Fix => {
            "The fix committed in the worktree, with the project's validation command passing \
             locally, then the assignment declared finished."
        }
    }
}

fn dependency_lines(state: &ProjectState, task: &Task) -> String {
    if task.dependencies.is_empty() {
        return "Nothing pinned: start from the project's base branch.".to_string();
    }
    let mut out = String::new();
    for (index, dependency) in task.dependencies.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let title = state
            .tasks
            .get(&dependency.task)
            .map(|task| task.title.as_str())
            .unwrap_or("an unknown task");
        out.push_str(&format!(
            "- `{}` **{}** at `{}`",
            dependency.task, title, dependency.commit
        ));
    }
    out
}

fn validation_note(project_file: Option<&ProjectFile>) -> String {
    let command = project_file
        .map(|file| file.validation.command.as_str())
        .unwrap_or_default()
        .trim();
    if command.is_empty() {
        "This project configures no validation command, so the daemon has nothing to run against \
         your commit."
            .to_string()
    } else {
        format!("The command is `{command}`. Run it yourself before you submit.")
    }
}

fn display(path: &std::path::Path) -> String {
    path.to_string_lossy().to_string()
}
