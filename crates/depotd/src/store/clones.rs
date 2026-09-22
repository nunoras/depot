use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use depot_core::{ProjectId, TaskId};
use rusqlite::{OptionalExtension, params};

use crate::error::{Error, Result};
use crate::project::{Clone, LocationKind, Project};
use crate::store::Store;
use crate::store::migrations;

const PROJECT_TABLES: &[&str] = &[
    "tasks",
    "task_dependencies",
    "task_attempts",
    "task_questions",
    "task_validations",
    "task_artifacts",
    "task_links",
    "task_submission_artifacts",
    "events",
    "clones",
];

const TASK_ID_COLUMNS: &[(&str, &str)] = &[
    ("tasks", "id"),
    ("task_dependencies", "task_id"),
    ("task_dependencies", "prerequisite"),
    ("task_attempts", "task_id"),
    ("task_questions", "task_id"),
    ("task_validations", "task_id"),
    ("task_artifacts", "task_id"),
    ("task_links", "task_id"),
    ("task_submission_artifacts", "task_id"),
    ("events", "task_id"),
];

impl Store {
    pub fn put_clone(&self, clone: &Clone) -> Result<bool> {
        if let Some(existing) = self.clone_for_path(&clone.path)? {
            if existing.project != clone.project {
                let owner = self
                    .project(&existing.project)?
                    .map(|project| project.slug)
                    .unwrap_or_else(|| existing.project.to_string());
                return Err(Error::Project(format!(
                    "`{}` is already a clone of project `{owner}` (`{}`); run `depot project repoint {owner} --origin <url>` to follow a renamed origin there",
                    clone.path.display(),
                    existing.project,
                )));
            }
            if existing.origin != clone.origin {
                self.set_clone_origin(&clone.path, clone.origin.as_deref())?;
            }
            return Ok(false);
        }
        self.connection().execute(
            "INSERT INTO clones (path, project_id, origin) VALUES (?1, ?2, ?3)",
            params![
                clone.path.to_string_lossy(),
                clone.project.as_str(),
                clone.origin,
            ],
        )?;
        Ok(true)
    }

    pub fn clones_for_project(&self, project: &ProjectId) -> Result<Vec<Clone>> {
        let mut statement = self.connection().prepare(
            "SELECT path, project_id, origin FROM clones WHERE project_id = ?1 ORDER BY path",
        )?;
        let raw = statement
            .query_map(params![project.as_str()], read_clone)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(raw)
    }

    pub fn clone_for_project(&self, project: &ProjectId) -> Result<Option<Clone>> {
        Ok(self.clones_for_project(project)?.into_iter().next())
    }

    pub fn clone_for_path(&self, path: &Path) -> Result<Option<Clone>> {
        let mut statement = self
            .connection()
            .prepare("SELECT path, project_id, origin FROM clones WHERE path = ?1")?;
        let mut rows = statement.query_map(params![path.to_string_lossy()], read_clone)?;
        match rows.next() {
            Some(clone) => Ok(Some(clone?)),
            None => Ok(None),
        }
    }

    pub fn project_path(&self, project: &Project) -> Result<Option<PathBuf>> {
        Ok(self.clone_for_project(&project.id)?.map(|clone| clone.path))
    }

    pub(crate) fn local_clone(&self, project: &Project) -> Result<Clone> {
        self.clone_for_project(&project.id)?
            .ok_or_else(|| Error::Project(format!("project `{}` has no local clone", project.slug)))
    }

    pub fn ensure_clone_origin(&self, project: &Project) -> Result<()> {
        if !self.project_has_in_flight(&project.id)? {
            return Ok(());
        }
        let clone = self.local_clone(project)?;
        if clone
            .origin
            .as_deref()
            .and_then(depot_core::remote_identity)
            .is_none()
        {
            return Ok(());
        }
        let live = crate::identity::read_origin(&clone.path);
        let live_identity = live.as_deref().and_then(depot_core::remote_identity);
        if live_identity.as_deref() == Some(project.id.as_str()) {
            return Ok(());
        }
        Err(Error::Project(format!(
            "clone `{}` has origin `{}` (identity `{}`) but project `{}` is `{}`; they disagree while a task is in flight, so depot will not act on this clone",
            clone.path.display(),
            live.as_deref().unwrap_or("no origin"),
            live_identity.as_deref().unwrap_or("none"),
            project.slug,
            project.id,
        )))
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.connection().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn rekey_projects(&self) -> Result<()> {
        if self.meta(migrations::REKEY_MARKER)?.is_some() {
            return Ok(());
        }
        let mut groups: BTreeMap<String, Vec<Project>> = BTreeMap::new();
        for project in self.projects()? {
            groups
                .entry(self.target_identity(&project)?)
                .or_default()
                .push(project);
        }
        for (identity, members) in groups {
            self.rekey_group(&identity, members)?;
        }
        self.set_meta(migrations::REKEY_MARKER, "done")?;
        Ok(())
    }

    fn target_identity(&self, project: &Project) -> Result<String> {
        Ok(match project.kind {
            LocationKind::Path => {
                match crate::identity::read_origin(Path::new(project.id.as_str())) {
                    Some(origin) => crate::identity::identity_for_origin(&origin)
                        .unwrap_or_else(|| project.id.to_string()),
                    None => project.id.to_string(),
                }
            }
            LocationKind::Url => depot_core::remote_identity(project.id.as_str())
                .unwrap_or_else(|| project.id.to_string()),
        })
    }

    fn rekey_group(&self, identity: &str, members: Vec<Project>) -> Result<()> {
        if members.len() == 1 {
            let project = &members[0];
            if project.id.as_str() != identity {
                self.rekey_project(&project.id, &ProjectId::new(identity))?;
            }
            self.record_clone_path(&ProjectId::new(identity), Path::new(project.id.as_str()))?;
            return Ok(());
        }

        let mut in_flight = Vec::new();
        for project in &members {
            if self.project_has_in_flight(&project.id)? {
                in_flight.push(project);
            }
        }
        if in_flight.len() > 1 {
            let names = in_flight
                .iter()
                .map(|project| format!("`{}` ({})", project.slug, project.id))
                .collect::<Vec<_>>()
                .join(" and ");
            return Err(Error::Project(format!(
                "cannot merge the projects that share origin `{identity}`: {names} both have tasks in flight; stop them and run `depot store migrate` again"
            )));
        }

        let winner = members
            .iter()
            .find(|project| project.id.as_str() == identity)
            .cloned()
            .unwrap_or_else(|| members[0].clone());
        for member in &members {
            if member.id != winner.id {
                self.merge_project(&winner, member)?;
            }
        }
        if winner.id.as_str() != identity {
            self.rekey_project(&winner.id, &ProjectId::new(identity))?;
        }
        for member in &members {
            self.record_clone_path(&ProjectId::new(identity), Path::new(member.id.as_str()))?;
        }
        Ok(())
    }

    pub fn project_has_in_flight(&self, project: &ProjectId) -> Result<bool> {
        let count: i64 = self.connection().query_row(
            "SELECT count(*) FROM tasks WHERE project_id = ?1 AND state IN ('running', 'waiting_on_question', 'validating')",
            params![project.as_str()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn record_clone_path(&self, identity: &ProjectId, path: &Path) -> Result<()> {
        if !path.is_dir() {
            return Ok(());
        }
        let origin = crate::identity::read_origin(path);
        self.put_clone(&Clone {
            project: identity.clone(),
            path: path.to_path_buf(),
            origin,
        })?;
        Ok(())
    }

    pub fn set_clone_origin(&self, path: &Path, origin: Option<&str>) -> Result<()> {
        self.connection().execute(
            "UPDATE clones SET origin = ?1 WHERE path = ?2",
            params![origin, path.to_string_lossy()],
        )?;
        Ok(())
    }

    pub(crate) fn rekey_project(&self, from: &ProjectId, to: &ProjectId) -> Result<()> {
        let transaction = self.connection().unchecked_transaction()?;
        transaction.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        for table in PROJECT_TABLES {
            transaction.execute(
                &format!("UPDATE {table} SET project_id = ?1 WHERE project_id = ?2"),
                params![to.as_str(), from.as_str()],
            )?;
        }
        for table in ["coordinators", "task_counters"] {
            transaction.execute(
                &format!("UPDATE {table} SET project_id = ?1 WHERE project_id = ?2"),
                params![to.as_str(), from.as_str()],
            )?;
        }
        transaction.execute(
            "UPDATE projects SET id = ?1 WHERE id = ?2",
            params![to.as_str(), from.as_str()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn merge_project(&self, winner: &Project, loser: &Project) -> Result<()> {
        let winner_tasks = self.tasks(&winner.id)?;
        let loser_tasks = self.tasks(&loser.id)?;
        let mut next = winner_tasks
            .keys()
            .chain(loser_tasks.keys())
            .filter_map(|id| task_number(id.as_str()))
            .max()
            .unwrap_or(0)
            + 1;
        let mut renames = BTreeMap::new();
        for id in loser_tasks.keys() {
            if winner_tasks.contains_key(id) {
                renames.insert(id.clone(), TaskId::new(format!("t-{next}")));
                next += 1;
            }
        }
        let counter = self.merged_counter(&winner.id, &loser.id, next)?;

        let transaction = self.connection().unchecked_transaction()?;
        transaction.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        for (old, new) in &renames {
            for (table, column) in TASK_ID_COLUMNS {
                transaction.execute(
                    &format!(
                        "UPDATE {table} SET {column} = ?1 WHERE project_id = ?2 AND {column} = ?3"
                    ),
                    params![new.as_str(), loser.id.as_str(), old.as_str()],
                )?;
            }
            transaction.execute(
                "UPDATE tasks SET base_dependency = ?1 WHERE project_id = ?2 AND base_dependency = ?3",
                params![new.as_str(), loser.id.as_str(), old.as_str()],
            )?;
            transaction.execute(
                "UPDATE tasks SET rework_of = ?1 WHERE project_id = ?2 AND rework_of = ?3",
                params![new.as_str(), loser.id.as_str(), old.as_str()],
            )?;
        }
        transaction.execute(
            "UPDATE events SET key = ?1 || key WHERE project_id = ?2",
            params![format!("{}:", loser.slug), loser.id.as_str()],
        )?;
        for table in PROJECT_TABLES {
            transaction.execute(
                &format!("UPDATE {table} SET project_id = ?1 WHERE project_id = ?2"),
                params![winner.id.as_str(), loser.id.as_str()],
            )?;
        }
        transaction.execute(
            "DELETE FROM task_counters WHERE project_id = ?1",
            params![loser.id.as_str()],
        )?;
        transaction.execute(
            "DELETE FROM coordinators WHERE project_id = ?1",
            params![loser.id.as_str()],
        )?;
        transaction.execute(
            "INSERT INTO task_counters (project_id, next_number) VALUES (?1, ?2)
             ON CONFLICT(project_id) DO UPDATE SET next_number = ?2",
            params![winner.id.as_str(), counter],
        )?;
        transaction.execute(
            "DELETE FROM projects WHERE id = ?1",
            params![loser.id.as_str()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn merged_counter(&self, winner: &ProjectId, loser: &ProjectId, floor: i64) -> Result<i64> {
        let read = |project: &ProjectId| -> Result<Option<i64>> {
            Ok(self
                .connection()
                .query_row(
                    "SELECT next_number FROM task_counters WHERE project_id = ?1",
                    params![project.as_str()],
                    |row| row.get(0),
                )
                .optional()?)
        };
        let winner_next = read(winner)?.unwrap_or(0);
        let loser_next = read(loser)?.unwrap_or(0);
        Ok(winner_next.max(loser_next).max(floor))
    }
}

fn read_clone(row: &rusqlite::Row<'_>) -> rusqlite::Result<Clone> {
    Ok(Clone {
        project: ProjectId::new(row.get::<_, String>(1)?),
        path: PathBuf::from(row.get::<_, String>(0)?),
        origin: row.get(2)?,
    })
}

fn task_number(id: &str) -> Option<i64> {
    id.strip_prefix("t-")?.parse().ok()
}
