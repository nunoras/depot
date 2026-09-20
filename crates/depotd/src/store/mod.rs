mod apply;
mod coordinators;
mod migrations;
mod tasks;

use std::path::Path;
use std::time::Duration;

use depot_core::{Fact, Limits, ProjectId, ProjectState, Timestamp};
use rusqlite::{Connection, params};

use crate::config::ProjectConfig;
use crate::error::{Error, Result};
use crate::factcodec;
use crate::home::DepotHome;
use crate::project::{LocationKind, Project};

pub use apply::Applied;
pub use migrations::SCHEMA_VERSION;

const BUSY_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_RETRY_ATTEMPTS: u32 = 5;
const LOCK_RETRY_BACKOFF: Duration = Duration::from_millis(100);

pub(crate) fn with_lock_retry<T>(operation: impl Fn() -> Result<T>) -> Result<T> {
    for attempt in 1..LOCK_RETRY_ATTEMPTS {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if error.is_lock_contention() => {
                std::thread::sleep(LOCK_RETRY_BACKOFF * attempt);
            }
            Err(error) => return Err(error),
        }
    }
    operation()
}

pub struct Store {
    connection: Connection,
    home: DepotHome,
}

impl Store {
    pub fn open(home: &DepotHome) -> Result<Self> {
        home.ensure()?;
        let connection = Connection::open(home.database_path())?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        })?;
        migrations::migrate(&connection)?;
        Ok(Self {
            connection,
            home: home.clone(),
        })
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    pub(crate) fn home(&self) -> &DepotHome {
        &self.home
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    pub fn put_project(&self, project: &Project) -> Result<bool> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO projects (id, kind, slug, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                project.id.as_str(),
                project.kind.name(),
                project.slug,
                project.created_at.millis() as i64,
            ],
        )?;
        Ok(inserted > 0)
    }

    pub fn project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.first_project(
            "SELECT id, kind, slug, created_at FROM projects WHERE id = ?1",
            params![id.as_str()],
        )
    }

    pub fn project_by_slug(&self, slug: &str) -> Result<Option<Project>> {
        self.first_project(
            "SELECT id, kind, slug, created_at FROM projects WHERE slug = ?1",
            params![slug],
        )
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut statement = self
            .connection
            .prepare("SELECT id, kind, slug, created_at FROM projects ORDER BY id")?;
        let raw = statement
            .query_map([], RawProject::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter().map(RawProject::into_project).collect()
    }

    pub fn taken_slugs(&self) -> Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare("SELECT slug FROM projects ORDER BY slug")?;
        let slugs = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(slugs)
    }

    pub fn project_config(&self, project: &Project) -> Result<ProjectConfig> {
        match project.kind {
            LocationKind::Path => ProjectConfig::load(Path::new(project.id.as_str())),
            LocationKind::Url => Ok(ProjectConfig::default()),
        }
    }

    pub fn project_state(&self, project: &Project) -> Result<ProjectState> {
        let settings = self.home.load_settings()?;
        let config = self.project_config(project)?;
        Ok(ProjectState {
            project: project.id.clone(),
            tasks: self.tasks(&project.id)?,
            coordinator: self.coordinator_session(&project.id)?,
            profiles: config.profiles()?,
            fallback_profiles: settings.profile_fallbacks(),
            limits: Limits {
                max_concurrent_tasks: config.max_concurrent_tasks.min(settings.concurrency),
                ..settings.limits()
            },
            always_relay_questions: config.questions.always_relay,
            auto_merge: config.pull_request.auto_merge,
        })
    }

    pub fn record_event(
        &self,
        project: &ProjectId,
        key: &str,
        fact: &Fact,
    ) -> Result<EventOutcome> {
        if write_event(self.connection(), project, key, fact)? > 0 {
            Ok(EventOutcome::Recorded)
        } else {
            Ok(EventOutcome::Duplicate)
        }
    }

    pub fn event(&self, project: &ProjectId, key: &str) -> Result<Option<RecordedEvent>> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, key, at, kind, payload, task_id FROM events
             WHERE project_id = ?1 AND key = ?2",
        )?;
        let mut rows = statement.query_map(params![project.as_str(), key], RawEvent::read)?;
        match rows.next() {
            Some(raw) => Ok(Some(raw?.into_event()?)),
            None => Ok(None),
        }
    }

    pub fn events(&self, project: &ProjectId) -> Result<Vec<RecordedEvent>> {
        self.events_since(project, 0)
    }

    pub fn events_since(&self, project: &ProjectId, after: u64) -> Result<Vec<RecordedEvent>> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, key, at, kind, payload, task_id FROM events
             WHERE project_id = ?1 AND id > ?2 ORDER BY id",
        )?;
        let raw = statement
            .query_map(params![project.as_str(), after as i64], RawEvent::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter().map(RawEvent::into_event).collect()
    }

    pub fn last_event_id(
        &self,
        project: &ProjectId,
        task: &depot_core::TaskId,
        kind: &str,
    ) -> Result<Option<u64>> {
        let id: Option<i64> = self.connection.query_row(
            "SELECT MAX(id) FROM events WHERE project_id = ?1 AND task_id = ?2 AND kind = ?3",
            params![project.as_str(), task.as_str(), kind],
            |row| row.get(0),
        )?;
        Ok(id.and_then(|id| u64::try_from(id).ok()))
    }

    fn first_project(
        &self,
        sql: &str,
        parameters: impl rusqlite::Params,
    ) -> Result<Option<Project>> {
        let mut statement = self.connection.prepare(sql)?;
        let mut rows = statement.query_map(parameters, RawProject::read)?;
        match rows.next() {
            Some(raw) => Ok(Some(raw?.into_project()?)),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventOutcome {
    Recorded,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedEvent {
    pub id: u64,
    pub project: ProjectId,
    pub key: String,
    pub at: Timestamp,
    pub kind: String,
    pub payload: String,
    pub task: Option<depot_core::TaskId>,
}

pub fn event_key(parts: &[&str]) -> String {
    parts.join(":")
}

pub(crate) fn write_event(
    connection: &Connection,
    project: &ProjectId,
    key: &str,
    fact: &Fact,
) -> Result<usize> {
    let task = crate::vocabulary::fact_task(&fact.kind).map(|task| task.as_str().to_string());
    Ok(connection.execute(
        "INSERT OR IGNORE INTO events (project_id, key, at, kind, payload, task_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project.as_str(),
            key,
            fact.at.millis() as i64,
            factcodec::kind_name(&fact.kind),
            factcodec::encode_payload(&fact.kind),
            task,
        ],
    )?)
}

struct RawProject {
    id: String,
    kind: String,
    slug: String,
    created_at: i64,
}

impl RawProject {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            kind: row.get("kind")?,
            slug: row.get("slug")?,
            created_at: row.get("created_at")?,
        })
    }

    fn into_project(self) -> Result<Project> {
        Ok(Project {
            id: ProjectId::new(self.id),
            kind: LocationKind::from_name(&self.kind)?,
            slug: self.slug,
            created_at: u64::try_from(self.created_at)
                .map(Timestamp::from_millis)
                .map_err(|_| {
                    Error::Schema(format!("{} is not a millisecond count", self.created_at))
                })?,
        })
    }
}

struct RawEvent {
    id: i64,
    project_id: String,
    key: String,
    at: i64,
    kind: String,
    payload: String,
    task_id: Option<String>,
}

impl RawEvent {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            key: row.get("key")?,
            at: row.get("at")?,
            kind: row.get("kind")?,
            payload: row.get("payload")?,
            task_id: row.get("task_id")?,
        })
    }

    fn into_event(self) -> Result<RecordedEvent> {
        let at = u64::try_from(self.at)
            .map(Timestamp::from_millis)
            .map_err(|_| Error::Schema(format!("{} is not a millisecond count", self.at)))?;
        let id = u64::try_from(self.id)
            .map_err(|_| Error::Schema(format!("{} is not an event id", self.id)))?;
        Ok(RecordedEvent {
            id,
            project: ProjectId::new(self.project_id),
            key: self.key,
            at,
            kind: self.kind,
            payload: self.payload,
            task: self.task_id.map(depot_core::TaskId::new),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::with_lock_retry;
    use crate::error::Error;

    fn busy() -> Error {
        Error::Database(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        ))
    }

    #[test]
    fn a_busy_error_is_lock_contention() {
        assert!(busy().is_lock_contention());
        assert!(!Error::NotFound("no".to_string()).is_lock_contention());
    }

    #[test]
    fn lock_contention_is_retried_until_the_operation_succeeds() {
        let attempts = Cell::new(0);
        let result = with_lock_retry(|| {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                Err(busy())
            } else {
                Ok(42)
            }
        })
        .expect("retry succeeds");
        assert_eq!(result, 42);
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn a_busy_error_that_outlasts_the_retries_is_returned() {
        assert!(
            with_lock_retry(|| -> crate::error::Result<()> { Err(busy()) })
                .expect_err("busy is returned")
                .is_lock_contention()
        );
    }

    #[test]
    fn a_non_lock_error_is_returned_without_retrying() {
        let attempts = Cell::new(0);
        let error = with_lock_retry(|| {
            attempts.set(attempts.get() + 1);
            Err::<(), _>(Error::NotFound("no".to_string()))
        })
        .expect_err("other errors surface");
        assert_eq!(error.to_string(), "no");
        assert_eq!(attempts.get(), 1);
    }
}
