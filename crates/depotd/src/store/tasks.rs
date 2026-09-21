use std::collections::BTreeMap;
use std::time::Duration;

use depot_core::{
    Answer, Artifact, Attempt, Checks, CommitId, Dependency, Link, ProfileId, ProjectId, Question,
    Retry, SessionId, Submission, Task, TaskId, Timestamp, ValidationRecord, WorktreeLease,
};
use rusqlite::{OptionalExtension, Row, Transaction, params};

use crate::checklist::render_checklist;
use crate::error::{Error, Result};
use crate::store::Store;
use crate::vocabulary::{
    answered_by_from_name, answered_by_name, artifact_kind_from_name, artifact_kind_name,
    checks_from_name, checks_name, outcome_from_name, outcome_name, role_from_name, role_name,
    state_from_name, state_name,
};

impl Store {
    pub fn put_task(&self, task: &Task) -> Result<()> {
        let project = self.project(&task.project)?.ok_or_else(|| {
            Error::NotFound(format!(
                "no project `{}` is registered",
                task.project.as_str()
            ))
        })?;
        let mut state = self.project_state(&project)?;
        state.tasks.insert(task.id.clone(), task.clone());
        let checklist = render_checklist(&state, false);
        let project_home = self.home().project_home(&project.slug);
        project_home.ensure()?;
        let checklist_path = project_home.checklist_path();

        let transaction = self.connection().unchecked_transaction()?;
        write_task(&transaction, task)?;
        std::fs::write(&checklist_path, checklist)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn task(&self, project: &ProjectId, id: &TaskId) -> Result<Option<Task>> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE project_id = ?1 AND id = ?2"
        ))?;
        let mut rows =
            statement.query_map(params![project.as_str(), id.as_str()], RawTask::read)?;
        match rows.next() {
            Some(raw) => Ok(Some(raw?.into_task(self)?)),
            None => Ok(None),
        }
    }

    pub fn next_task_id(&self, project: &ProjectId) -> Result<TaskId> {
        let next: Option<i64> = self
            .connection()
            .query_row(
                "SELECT next_number FROM task_counters WHERE project_id = ?1",
                params![project.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        let number = match next {
            Some(number) => number,
            None => {
                self.tasks(project)?
                    .keys()
                    .filter_map(|id| task_id_number(id.as_str()))
                    .max()
                    .unwrap_or(0)
                    + 1
            }
        };
        self.connection().execute(
            "INSERT INTO task_counters (project_id, next_number) VALUES (?1, ?2)
             ON CONFLICT(project_id) DO UPDATE SET next_number = ?2",
            params![project.as_str(), number + 1],
        )?;
        Ok(TaskId::new(format!("t-{number}")))
    }

    pub fn tasks(&self, project: &ProjectId) -> Result<BTreeMap<TaskId, Task>> {
        let mut statement = self.connection().prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE project_id = ?1"
        ))?;
        let raw = statement
            .query_map(params![project.as_str()], RawTask::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut tasks = BTreeMap::new();
        for row in raw {
            let task = row.into_task(self)?;
            tasks.insert(task.id.clone(), task);
        }
        Ok(tasks)
    }
}

const TASK_COLUMNS: &str = "project_id, id, title, intent, role, state, base_dependency, \
     branch_head, retry_profile, retry_not_before, submission_summary, merge_refused, failure, redirect_text, redirect_delivered, created_at, updated_at, dispatch_profile, acknowledged_at, hold_pr, rework_of, release_pending, release_held";

struct RawTask {
    project_id: String,
    id: String,
    title: String,
    intent: String,
    role: String,
    dispatch_profile: Option<String>,
    state: String,
    base_dependency: Option<String>,
    branch_head: Option<String>,
    retry_profile: Option<String>,
    retry_not_before: Option<i64>,
    submission_summary: Option<String>,
    merge_refused: Option<String>,
    failure: Option<String>,
    redirect_text: Option<String>,
    redirect_delivered: bool,
    acknowledged_at: Option<i64>,
    hold_pr: bool,
    rework_of: Option<String>,
    release_pending: Option<String>,
    release_held: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl RawTask {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            project_id: row.get("project_id")?,
            id: row.get("id")?,
            title: row.get("title")?,
            intent: row.get("intent")?,
            role: row.get("role")?,
            dispatch_profile: row.get("dispatch_profile")?,
            state: row.get("state")?,
            base_dependency: row.get("base_dependency")?,
            branch_head: row.get("branch_head")?,
            retry_profile: row.get("retry_profile")?,
            retry_not_before: row.get("retry_not_before")?,
            submission_summary: row.get("submission_summary")?,
            merge_refused: row.get("merge_refused")?,
            failure: row.get("failure")?,
            redirect_text: row.get("redirect_text")?,
            redirect_delivered: row.get::<_, i64>("redirect_delivered")? != 0,
            acknowledged_at: row.get("acknowledged_at")?,
            hold_pr: row.get("hold_pr")?,
            rework_of: row.get("rework_of")?,
            release_pending: row.get("release_pending")?,
            release_held: row.get("release_held")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    fn into_task(self, store: &Store) -> Result<Task> {
        let project = ProjectId::new(self.project_id);
        let role = role_from_name(&self.role).ok_or_else(|| {
            Error::Schema(format!(
                "task `{}` holds unknown role `{}`",
                self.id, self.role
            ))
        })?;
        let retry = match (&self.retry_profile, self.retry_not_before) {
            (Some(profile), Some(not_before)) => Some(Retry {
                profile: ProfileId::new(profile.clone()),
                not_before: millis(not_before)?,
            }),
            (None, None) => None,
            _ => {
                return Err(Error::Schema(format!(
                    "task `{}` holds half a retry record",
                    self.id
                )));
            }
        };

        Ok(Task {
            id: TaskId::new(self.id.clone()),
            project: project.clone(),
            title: self.title,
            intent: self.intent,
            role,
            dispatch_profile: self.dispatch_profile.map(ProfileId::new),
            state: state_from_name(&self.state)?,
            dependencies: store.dependencies(&project, &self.id)?,
            base_dependency: self.base_dependency.map(TaskId::new),
            attempts: store.attempts(&project, &self.id)?,
            questions: store.questions(&project, &self.id)?,
            validations: store.validations(&project, &self.id)?,
            submission: self
                .submission_summary
                .map(|summary| {
                    store
                        .submission_artifacts(&project, &self.id)
                        .map(|artifacts| Submission { summary, artifacts })
                })
                .transpose()?,
            artifacts: store.artifacts(&project, &self.id)?,
            links: store.links(&project, &self.id)?,
            branch_head: self.branch_head.map(CommitId::new),
            merge_refused: self.merge_refused,
            failure: self.failure,
            redirect_text: self.redirect_text,
            redirect_delivered: self.redirect_delivered,
            acknowledged_at: self
                .acknowledged_at
                .map(|value| value as u64)
                .map(Timestamp::from_millis),
            rework_of: self.rework_of.map(TaskId::new),
            hold_pr: self.hold_pr,
            release_pending: self.release_pending.map(WorktreeLease::new),
            release_held: self.release_held,
            retry,
            created_at: millis(self.created_at)?,
            updated_at: millis(self.updated_at)?,
        })
    }
}

impl Store {
    fn dependencies(&self, project: &ProjectId, task: &str) -> Result<Vec<Dependency>> {
        let mut statement = self.connection().prepare(
            "SELECT prerequisite, commit_id FROM task_dependencies
             WHERE project_id = ?1 AND task_id = ?2 ORDER BY position",
        )?;
        let rows = statement.query_map(params![project.as_str(), task], |row| {
            Ok((
                row.get::<_, String>("prerequisite")?,
                row.get::<_, String>("commit_id")?,
            ))
        })?;
        let mut dependencies = Vec::new();
        for row in rows {
            let (prerequisite, commit) = row?;
            dependencies.push(Dependency {
                task: TaskId::new(prerequisite),
                commit: CommitId::new(commit),
            });
        }
        Ok(dependencies)
    }

    fn attempts(&self, project: &ProjectId, task: &str) -> Result<Vec<Attempt>> {
        let mut statement = self.connection().prepare(
            "SELECT session, profile, worktree, started_at, finished_at, outcome, rebase, last_seen_at
             FROM task_attempts WHERE project_id = ?1 AND task_id = ?2 ORDER BY position",
        )?;
        let rows = statement.query_map(params![project.as_str(), task], |row| {
            Ok(RawAttempt {
                session: row.get("session")?,
                profile: row.get("profile")?,
                worktree: row.get("worktree")?,
                started_at: row.get("started_at")?,
                finished_at: row.get("finished_at")?,
                outcome: row.get("outcome")?,
                rebase: row.get::<_, i64>("rebase")? != 0,
                last_seen_at: row.get("last_seen_at")?,
            })
        })?;
        let mut attempts = Vec::new();
        for row in rows {
            let raw = row?;
            attempts.push(Attempt {
                session: raw.session.map(SessionId::new),
                profile: ProfileId::new(raw.profile),
                worktree: raw.worktree.map(WorktreeLease::new),
                started_at: millis(raw.started_at)?,
                finished_at: raw.finished_at.map(millis).transpose()?,
                outcome: outcome_from_name(&raw.outcome)?,
                rebase: raw.rebase,
                last_seen_at: raw.last_seen_at.map(millis).transpose()?,
            });
        }
        Ok(attempts)
    }

    fn questions(&self, project: &ProjectId, task: &str) -> Result<Vec<Question>> {
        let mut statement = self.connection().prepare(
            "SELECT text, asked_at, answer_text, answered_by, answered_at
             FROM task_questions WHERE project_id = ?1 AND task_id = ?2 ORDER BY position",
        )?;
        let rows = statement.query_map(params![project.as_str(), task], |row| {
            Ok(RawQuestion {
                text: row.get("text")?,
                asked_at: row.get("asked_at")?,
                answer_text: row.get("answer_text")?,
                answered_by: row.get("answered_by")?,
                answered_at: row.get("answered_at")?,
            })
        })?;
        let mut questions = Vec::new();
        for row in rows {
            let raw = row?;
            let answer = match (raw.answer_text, raw.answered_by, raw.answered_at) {
                (None, None, None) => None,
                (Some(text), Some(by), Some(at)) => Some(Answer {
                    text,
                    by: answered_by_from_name(&by)
                        .ok_or_else(|| Error::Schema(format!("unknown answerer `{by}`")))?,
                    at: millis(at)?,
                }),
                _ => {
                    return Err(Error::Schema(format!(
                        "question on task `{task}` holds half an answer"
                    )));
                }
            };
            questions.push(Question {
                text: raw.text,
                asked_at: millis(raw.asked_at)?,
                answer,
            });
        }
        Ok(questions)
    }

    fn validations(&self, project: &ProjectId, task: &str) -> Result<Vec<ValidationRecord>> {
        let mut statement = self.connection().prepare(
            "SELECT command, commit_id, base_commit, exit_code, duration_millis, output_tail
             FROM task_validations WHERE project_id = ?1 AND task_id = ?2 ORDER BY position",
        )?;
        let rows = statement.query_map(params![project.as_str(), task], |row| {
            Ok(RawValidation {
                command: row.get("command")?,
                commit: row.get("commit_id")?,
                base_commit: row.get("base_commit")?,
                exit_code: row.get("exit_code")?,
                duration_millis: row.get("duration_millis")?,
                output_tail: row.get("output_tail")?,
            })
        })?;
        let mut validations = Vec::new();
        for row in rows {
            let raw = row?;
            validations.push(ValidationRecord {
                command: raw.command,
                commit: CommitId::new(raw.commit),
                base_commit: raw.base_commit.map(CommitId::new),
                exit_code: raw.exit_code,
                duration: Duration::from_millis(raw.duration_millis.max(0) as u64),
                output_tail: raw.output_tail,
            });
        }
        Ok(validations)
    }

    fn submission_artifacts(&self, project: &ProjectId, task: &str) -> Result<Vec<String>> {
        let mut statement = self.connection().prepare(
            "SELECT path FROM task_submission_artifacts
             WHERE project_id = ?1 AND task_id = ?2 ORDER BY position",
        )?;
        statement
            .query_map(params![project.as_str(), task], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn artifacts(&self, project: &ProjectId, task: &str) -> Result<Vec<Artifact>> {
        let mut statement = self.connection().prepare(
            "SELECT kind, path FROM task_artifacts
             WHERE project_id = ?1 AND task_id = ?2 ORDER BY position",
        )?;
        let rows = statement.query_map(params![project.as_str(), task], |row| {
            Ok((row.get::<_, String>("kind")?, row.get::<_, String>("path")?))
        })?;
        let mut artifacts = Vec::new();
        for row in rows {
            let (kind, path) = row?;
            artifacts.push(Artifact {
                kind: artifact_kind_from_name(&kind)?,
                path,
            });
        }
        Ok(artifacts)
    }

    fn links(&self, project: &ProjectId, task: &str) -> Result<Vec<Link>> {
        let mut statement = self.connection().prepare(
            "SELECT kind, number, url, checks FROM task_links
             WHERE project_id = ?1 AND task_id = ?2 ORDER BY position",
        )?;
        let rows = statement.query_map(params![project.as_str(), task], |row| {
            Ok(RawLink {
                kind: row.get("kind")?,
                number: row.get("number")?,
                url: row.get("url")?,
                checks: row.get("checks")?,
            })
        })?;
        let mut links = Vec::new();
        for row in rows {
            let raw = row?;
            match raw.kind.as_str() {
                "issue" => links.push(Link::Issue { url: raw.url }),
                "pull_request" => {
                    let number = raw.number.ok_or_else(|| {
                        Error::Schema(format!("pull request link on task `{task}` has no number"))
                    })?;
                    let checks = match raw.checks {
                        Some(checks) => checks_from_name(&checks)?,
                        None => Checks::Unknown,
                    };
                    links.push(Link::PullRequest {
                        number: number.max(0) as u64,
                        url: raw.url,
                        checks,
                    });
                }
                other => {
                    return Err(Error::Schema(format!("unknown link kind `{other}`")));
                }
            }
        }
        Ok(links)
    }
}

struct RawAttempt {
    session: Option<String>,
    profile: String,
    worktree: Option<String>,
    started_at: i64,
    finished_at: Option<i64>,
    outcome: String,
    rebase: bool,
    last_seen_at: Option<i64>,
}

struct RawQuestion {
    text: String,
    asked_at: i64,
    answer_text: Option<String>,
    answered_by: Option<String>,
    answered_at: Option<i64>,
}

struct RawValidation {
    command: String,
    commit: String,
    base_commit: Option<String>,
    exit_code: i32,
    duration_millis: i64,
    output_tail: String,
}

struct RawLink {
    kind: String,
    number: Option<i64>,
    url: String,
    checks: Option<String>,
}

fn millis(value: i64) -> Result<Timestamp> {
    u64::try_from(value)
        .map(Timestamp::from_millis)
        .map_err(|_| Error::Schema(format!("{value} is not a millisecond count")))
}

pub(super) fn write_task(transaction: &Transaction<'_>, task: &Task) -> Result<()> {
    transaction.execute(
        "DELETE FROM tasks WHERE project_id = ?1 AND id = ?2",
        params![task.project.as_str(), task.id.as_str()],
    )?;
    transaction.execute(
        "INSERT INTO tasks (
                project_id, id, title, intent, role, state, base_dependency, branch_head,
                retry_profile, retry_not_before, submission_summary, merge_refused, failure, redirect_text, redirect_delivered, created_at, updated_at, dispatch_profile, acknowledged_at, hold_pr, rework_of, release_pending, release_held
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        params![
            task.project.as_str(),
            task.id.as_str(),
            task.title,
            task.intent,
            role_name(task.role),
            state_name(task.state),
            task.base_dependency.as_ref().map(TaskId::as_str),
            task.branch_head.as_ref().map(CommitId::as_str),
            task.retry.as_ref().map(|retry| retry.profile.as_str()),
            task.retry
                .as_ref()
                .map(|retry| retry.not_before.millis() as i64),
            task.submission
                .as_ref()
                .map(|submission| submission.summary.as_str()),
            task.merge_refused.as_deref(),
            task.failure.as_deref(),
            task.redirect_text.as_deref(),
            task.redirect_delivered as i64,
            task.created_at.millis() as i64,
            task.updated_at.millis() as i64,
            task.dispatch_profile.as_ref().map(ProfileId::as_str),
            task.acknowledged_at.map(|value| value.millis() as i64),
            task.hold_pr,
            task.rework_of.as_ref().map(TaskId::as_str),
            task.release_pending.as_ref().map(WorktreeLease::as_str),
            task.release_held.as_deref(),
        ],
    )?;

    for (position, dependency) in task.dependencies.iter().enumerate() {
        transaction.execute(
            "INSERT INTO task_dependencies (project_id, task_id, position, prerequisite, commit_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                task.project.as_str(),
                task.id.as_str(),
                position as i64,
                dependency.task.as_str(),
                dependency.commit.as_str(),
            ],
        )?;
    }

    for (position, attempt) in task.attempts.iter().enumerate() {
        transaction.execute(
            "INSERT INTO task_attempts (
                    project_id, task_id, position, session, profile, worktree, started_at,
                    finished_at, outcome, rebase, last_seen_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                task.project.as_str(),
                task.id.as_str(),
                position as i64,
                attempt.session.as_ref().map(SessionId::as_str),
                attempt.profile.as_str(),
                attempt.worktree.as_ref().map(WorktreeLease::as_str),
                attempt.started_at.millis() as i64,
                attempt.finished_at.map(|at| at.millis() as i64),
                outcome_name(attempt.outcome),
                attempt.rebase as i64,
                attempt.last_seen_at.map(|at| at.millis() as i64),
            ],
        )?;
    }

    for (position, question) in task.questions.iter().enumerate() {
        transaction.execute(
            "INSERT INTO task_questions (
                    project_id, task_id, position, text, asked_at, answer_text, answered_by,
                    answered_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                task.project.as_str(),
                task.id.as_str(),
                position as i64,
                question.text,
                question.asked_at.millis() as i64,
                question.answer.as_ref().map(|answer| answer.text.as_str()),
                question
                    .answer
                    .as_ref()
                    .map(|answer| answered_by_name(answer.by)),
                question
                    .answer
                    .as_ref()
                    .map(|answer| answer.at.millis() as i64),
            ],
        )?;
    }

    for (position, record) in task.validations.iter().enumerate() {
        transaction.execute(
            "INSERT INTO task_validations (
                    project_id, task_id, position, command, commit_id, base_commit, exit_code,
                    duration_millis, output_tail
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                task.project.as_str(),
                task.id.as_str(),
                position as i64,
                record.command,
                record.commit.as_str(),
                record.base_commit.as_ref().map(CommitId::as_str),
                record.exit_code,
                record.duration.as_millis() as i64,
                record.output_tail,
            ],
        )?;
    }

    if let Some(submission) = &task.submission {
        for (position, path) in submission.artifacts.iter().enumerate() {
            transaction.execute(
                "INSERT INTO task_submission_artifacts (project_id, task_id, position, path)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    task.project.as_str(),
                    task.id.as_str(),
                    position as i64,
                    path,
                ],
            )?;
        }
    }

    for (position, artifact) in task.artifacts.iter().enumerate() {
        transaction.execute(
            "INSERT INTO task_artifacts (project_id, task_id, position, kind, path)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                task.project.as_str(),
                task.id.as_str(),
                position as i64,
                artifact_kind_name(artifact.kind),
                artifact.path,
            ],
        )?;
    }

    for (position, link) in task.links.iter().enumerate() {
        let (kind, number, url, checks) = match link {
            Link::Issue { url } => ("issue", None, url.as_str(), None),
            Link::PullRequest {
                number,
                url,
                checks,
            } => (
                "pull_request",
                Some(*number as i64),
                url.as_str(),
                Some(checks_name(*checks)),
            ),
        };
        transaction.execute(
            "INSERT INTO task_links (project_id, task_id, position, kind, number, url, checks)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                task.project.as_str(),
                task.id.as_str(),
                position as i64,
                kind,
                number,
                url,
                checks,
            ],
        )?;
    }
    Ok(())
}

fn task_id_number(id: &str) -> Option<i64> {
    id.strip_prefix("t-")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use depot_core::{ProjectId, Role, Task, TaskId, TaskState, Timestamp};
    use tempfile::tempdir;

    use crate::home::DepotHome;
    use crate::project::{LocationKind, Project};
    use crate::store::Store;

    fn store() -> (tempfile::TempDir, Store) {
        let temp = tempdir().expect("temporary directory");
        let home = DepotHome::at(temp.path().join("depot-home"));
        home.ensure().expect("depot home");
        let store = Store::open(&home).expect("store");
        (temp, store)
    }

    fn task(project: &ProjectId, id: &str) -> Task {
        Task {
            id: TaskId::new(id),
            project: project.clone(),
            title: format!("task {id}"),
            intent: String::new(),
            role: Role::Build,
            dispatch_profile: None,
            state: TaskState::Proposed,
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
            failure: None,
            redirect_text: None,
            redirect_delivered: false,
            acknowledged_at: None,
            rework_of: None,
            hold_pr: false,
            release_pending: None,
            release_held: None,
            retry: None,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(1),
        }
    }

    fn register(store: &Store, id: &str, slug: &str) -> ProjectId {
        let project = Project {
            id: ProjectId::new(id),
            kind: LocationKind::Url,
            slug: slug.to_string(),
            created_at: Timestamp::from_millis(1),
        };
        store.put_project(&project).expect("registered");
        project.id
    }

    #[test]
    fn a_deleted_id_is_never_handed_out_again() {
        let (_temp, store) = store();
        let project = register(&store, "/tmp/example", "example");

        for suffix in 1..=3 {
            store
                .put_task(&task(&project, &format!("t-{suffix}")))
                .expect("stored");
        }
        store
            .connection()
            .execute(
                "DELETE FROM tasks WHERE project_id = ?1 AND id = 't-2'",
                [project.as_str()],
            )
            .expect("deleted");

        assert_eq!(
            store.next_task_id(&project).expect("next id").as_str(),
            "t-4"
        );
        assert_eq!(
            store.next_task_id(&project).expect("next id").as_str(),
            "t-5"
        );
    }

    #[test]
    fn a_counter_is_seeded_from_the_highest_existing_id() {
        let (_temp, store) = store();
        let project = register(&store, "/tmp/example", "example");
        store.put_task(&task(&project, "t-7")).expect("stored");

        assert_eq!(
            store.next_task_id(&project).expect("next id").as_str(),
            "t-8"
        );
    }

    #[test]
    fn counters_are_per_project() {
        let (_temp, store) = store();
        let left = register(&store, "/tmp/left", "left");
        let right = register(&store, "/tmp/right", "right");

        assert_eq!(store.next_task_id(&left).expect("next id").as_str(), "t-1");
        assert_eq!(store.next_task_id(&right).expect("next id").as_str(), "t-1");
        assert_eq!(store.next_task_id(&left).expect("next id").as_str(), "t-2");
    }
}
