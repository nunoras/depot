use depot_core::{CoordinatorSession, ProjectId, SessionId, Timestamp};
use rusqlite::{Connection, params};

use crate::error::{Error, Result};
use crate::store::Store;

impl Store {
    pub fn coordinator_session(&self, project: &ProjectId) -> Result<Option<CoordinatorSession>> {
        let mut statement = self.connection().prepare(
            "SELECT session, started_at, context_tokens FROM coordinators WHERE project_id = ?1",
        )?;
        let mut rows = statement.query_map(params![project.as_str()], |row| {
            Ok((
                row.get::<_, Option<String>>("session")?,
                row.get::<_, Option<i64>>("started_at")?,
                row.get::<_, Option<i64>>("context_tokens")?,
            ))
        })?;
        let Some(raw) = rows.next() else {
            return Ok(None);
        };
        let (session, started_at, context_tokens) = raw?;
        match (session, started_at, context_tokens) {
            (Some(session), Some(started_at), Some(context_tokens)) => {
                Ok(Some(CoordinatorSession {
                    session: SessionId::new(session),
                    started_at: millis(started_at)?,
                    context_tokens: tokens(context_tokens)?,
                }))
            }
            (None, None, None) => Ok(None),
            _ => Err(Error::Schema(
                "the coordinator record holds half a session".to_string(),
            )),
        }
    }

    pub fn put_coordinator_session(
        &self,
        project: &ProjectId,
        session: &CoordinatorSession,
    ) -> Result<()> {
        let transaction = self.connection().unchecked_transaction()?;
        write_session(&transaction, project, session)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn clear_coordinator_session(&self, project: &ProjectId) -> Result<()> {
        let transaction = self.connection().unchecked_transaction()?;
        clear_session(&transaction, project)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn inbox_cursor(&self, project: &ProjectId) -> Result<u64> {
        let mut statement = self
            .connection()
            .prepare("SELECT inbox_cursor FROM coordinators WHERE project_id = ?1")?;
        let mut rows =
            statement.query_map(params![project.as_str()], |row| row.get::<_, i64>(0))?;
        match rows.next() {
            Some(raw) => u64::try_from(raw?)
                .map_err(|value| Error::Schema(format!("{value} is not an inbox cursor"))),
            None => Ok(0),
        }
    }

    pub fn set_inbox_cursor(&self, project: &ProjectId, cursor: u64) -> Result<()> {
        self.connection().execute(
            "INSERT INTO coordinators (project_id, session, started_at, context_tokens, inbox_cursor)
             VALUES (?1, NULL, NULL, NULL, ?2)
             ON CONFLICT (project_id) DO UPDATE SET inbox_cursor = excluded.inbox_cursor",
            params![project.as_str(), cursor as i64],
        )?;
        Ok(())
    }
}

pub(super) fn write_session(
    connection: &Connection,
    project: &ProjectId,
    session: &CoordinatorSession,
) -> Result<()> {
    connection.execute(
        "INSERT INTO coordinators (project_id, session, started_at, context_tokens, inbox_cursor)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT (project_id) DO UPDATE SET
             session = excluded.session,
             started_at = excluded.started_at,
             context_tokens = excluded.context_tokens",
        params![
            project.as_str(),
            session.session.as_str(),
            session.started_at.millis() as i64,
            session.context_tokens as i64,
        ],
    )?;
    Ok(())
}

pub(super) fn clear_session(connection: &Connection, project: &ProjectId) -> Result<()> {
    connection.execute(
        "UPDATE coordinators SET session = NULL, started_at = NULL, context_tokens = NULL
         WHERE project_id = ?1",
        params![project.as_str()],
    )?;
    Ok(())
}

fn millis(value: i64) -> Result<Timestamp> {
    u64::try_from(value)
        .map(Timestamp::from_millis)
        .map_err(|_| Error::Schema(format!("{value} is not a millisecond count")))
}

fn tokens(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Schema(format!("{value} is not a token count")))
}
