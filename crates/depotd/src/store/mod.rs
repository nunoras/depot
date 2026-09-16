mod migrations;
mod tasks;

use std::path::Path;
use std::time::Duration;

use depot_core::{Fact, ProjectId, ProjectState, Timestamp};
use rusqlite::{Connection, params};

use crate::config::ProjectConfig;
use crate::error::{Error, Result};
use crate::factcodec;
use crate::home::DepotHome;
use crate::project::{LocationKind, Project};

pub use migrations::SCHEMA_VERSION;

pub struct Store {
    connection: Connection,
    home: DepotHome,
}

impl Store {
    pub fn open(home: &DepotHome) -> Result<Self> {
        home.ensure()?;
        let connection = Connection::open(home.database_path())?;
        connection.busy_timeout(Duration::from_secs(5))?;
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
            profiles: config.profiles()?,
            fallback_profiles: settings.profile_fallbacks(),
            limits: settings.limits(),
            always_relay_questions: config.questions.always_relay,
        })
    }

    pub fn record_event(
        &self,
        project: &ProjectId,
        key: &str,
        fact: &Fact,
    ) -> Result<EventOutcome> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO events (project_id, key, at, kind, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                project.as_str(),
                key,
                fact.at.millis() as i64,
                factcodec::kind_name(&fact.kind),
                factcodec::encode_payload(&fact.kind),
            ],
        )?;
        if inserted > 0 {
            Ok(EventOutcome::Recorded)
        } else {
            Ok(EventOutcome::Duplicate)
        }
    }

    pub fn event(&self, project: &ProjectId, key: &str) -> Result<Option<RecordedEvent>> {
        let mut statement = self.connection.prepare(
            "SELECT project_id, key, at, kind, payload FROM events
             WHERE project_id = ?1 AND key = ?2",
        )?;
        let mut rows = statement.query_map(params![project.as_str(), key], RawEvent::read)?;
        match rows.next() {
            Some(raw) => Ok(Some(raw?.into_event()?)),
            None => Ok(None),
        }
    }

    pub fn events(&self, project: &ProjectId) -> Result<Vec<RecordedEvent>> {
        let mut statement = self.connection.prepare(
            "SELECT project_id, key, at, kind, payload FROM events
             WHERE project_id = ?1 ORDER BY id",
        )?;
        let raw = statement
            .query_map(params![project.as_str()], RawEvent::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter().map(RawEvent::into_event).collect()
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
    pub project: ProjectId,
    pub key: String,
    pub at: Timestamp,
    pub kind: String,
    pub payload: String,
}

pub fn event_key(parts: &[&str]) -> String {
    parts.join(":")
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
    project_id: String,
    key: String,
    at: i64,
    kind: String,
    payload: String,
}

impl RawEvent {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            project_id: row.get("project_id")?,
            key: row.get("key")?,
            at: row.get("at")?,
            kind: row.get("kind")?,
            payload: row.get("payload")?,
        })
    }

    fn into_event(self) -> Result<RecordedEvent> {
        let at = u64::try_from(self.at)
            .map(Timestamp::from_millis)
            .map_err(|_| Error::Schema(format!("{} is not a millisecond count", self.at)))?;
        Ok(RecordedEvent {
            project: ProjectId::new(self.project_id),
            key: self.key,
            at,
            kind: self.kind,
            payload: self.payload,
        })
    }
}
