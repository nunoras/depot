use std::path::{Path, PathBuf};

use depot_core::{ProjectId, ProjectState, TaskId, TaskState, Timestamp};

use crate::checklist::{render_checklist, render_checklist_observed};
use crate::clock::now;
use crate::commands::ensure_profiles_resolve;
use crate::config::PROJECT_CONFIG_FILE_NAME;
use crate::daemon::{daemon_build_mismatch, daemon_scope_covers};
use crate::error::{Error, Result};
use crate::home::{DepotHome, ProjectHome, slug_for};
use crate::project::{LocationKind, Project};
use crate::store::Store;
use crate::vocabulary::role_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub project: Project,
    pub created: bool,
    pub ignored_config: bool,
    pub home: ProjectHome,
}

pub fn add_project(home: &DepotHome, target: &str) -> Result<Added> {
    let resolved = resolve_target(target)?;
    let store = Store::open(home)?;

    let ignored_config = resolved
        .directory
        .as_deref()
        .map(ignore_project_config)
        .unwrap_or(false);
    let existing = store.project(&resolved.id)?;
    let project = match existing {
        Some(project) => project,
        None => Project {
            id: resolved.id.clone(),
            kind: resolved.kind,
            slug: unique_slug(&store.taken_slugs()?, &slug_for(resolved.id.as_str())),
            created_at: now(),
        },
    };

    let state = store.project_state(&project)?;
    ensure_profiles_resolve(home, &store, &project)?;
    let checklist = render_checklist(&state, false);
    let project_home = home.project_home(&project.slug);
    let created_home = !project_home.root().exists();

    let created = match (|| {
        project_home.ensure()?;
        std::fs::write(project_home.checklist_path(), &checklist)?;
        store.put_project(&project)
    })() {
        Ok(created) => created,
        Err(error) => {
            if created_home {
                let _ = std::fs::remove_dir_all(project_home.root());
            }
            return Err(error);
        }
    };

    Ok(Added {
        project,
        created,
        ignored_config,
        home: project_home,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusSelection {
    All,
    Project(String),
    CurrentDirectory,
}

pub fn select_project(store: &Store, name: Option<&str>) -> Result<Project> {
    match name {
        Some(name) => match match_project(store, name)? {
            Some(project) => Ok(project),
            None => Err(Error::NotFound(format!(
                "no project matches `{name}`: list them with `depot status --all`"
            ))),
        },
        None => {
            let cwd = std::env::current_dir()?;
            match project_for_directory(store, &cwd)? {
                Some(project) => Ok(project),
                None => Err(Error::NotFound(unmatched_directory_message(
                    &store.projects()?,
                ))),
            }
        }
    }
}

pub fn resolve_task(
    store: &Store,
    selection: Option<&str>,
    raw: &str,
) -> Result<(Project, TaskId)> {
    if let Some((slug, id)) = raw.split_once('/') {
        let project = match_project(store, slug)?.ok_or_else(|| {
            Error::NotFound(format!(
                "no project matches `{slug}`: list them with `depot status --all`"
            ))
        })?;
        return Ok((project, TaskId::new(id)));
    }
    let id = TaskId::new(raw);
    if let Some(name) = selection {
        let project = match_project(store, name)?.ok_or_else(|| {
            Error::NotFound(format!(
                "no project matches `{name}`: list them with `depot status --all`"
            ))
        })?;
        return Ok((project, id));
    }
    let mut owners = Vec::new();
    for project in store.projects()? {
        if store.task(&project.id, &id)?.is_some() {
            owners.push(project);
        }
    }
    match owners.len() {
        0 => {
            let project = select_project(store, None)?;
            Ok((project, id))
        }
        1 => Ok((owners.remove(0), id)),
        _ => {
            let slugs = owners
                .iter()
                .map(|project| project.slug.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(Error::Project(format!(
                "`{raw}` exists in several projects ({slugs}); qualify it as `<slug>/{raw}`"
            )))
        }
    }
}

pub fn render_projects(home: &DepotHome) -> Result<String> {
    let store = Store::open(home)?;
    let projects = store.projects()?;
    if projects.is_empty() {
        return Ok("No projects registered.\n".to_string());
    }
    let settings = home.load_settings()?;
    let mut out = String::new();
    for (index, project) in projects.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{}\n", project.slug));
        let location = match project.kind {
            LocationKind::Path => "path",
            LocationKind::Url => "url",
        };
        out.push_str(&format!("  {location}: {}\n", project.id));
        let profiles = store.project_config(project)?.profiles()?;
        if profiles.is_empty() {
            out.push_str("  profiles: none are mapped in .depot.toml\n");
            continue;
        }
        let listed = profiles
            .iter()
            .map(|(role, profile)| {
                let missing = if settings.profiles.contains_key(profile.as_str()) {
                    String::new()
                } else {
                    " (not defined in machine-local settings)".to_string()
                };
                format!("{}={profile}{missing}", role_name(*role))
            })
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("  profiles: {listed}\n"));
    }
    Ok(out)
}

pub fn render_status(
    home: &DepotHome,
    selection: &StatusSelection,
    history: bool,
) -> Result<String> {
    render_status_at(home, selection, history, crate::clock::now())
}

pub fn render_status_at(
    home: &DepotHome,
    selection: &StatusSelection,
    history: bool,
    now: Timestamp,
) -> Result<String> {
    let store = Store::open(home)?;
    let projects = match selection {
        StatusSelection::All => store.projects()?,
        StatusSelection::Project(name) => match match_project(&store, name)? {
            Some(project) => vec![project],
            None => {
                return Err(Error::NotFound(format!(
                    "no project matches `{name}`: list them with `depot status --all`"
                )));
            }
        },
        StatusSelection::CurrentDirectory => {
            let cwd = std::env::current_dir()?;
            match project_for_directory(&store, &cwd)? {
                Some(project) => vec![project],
                None => {
                    return Err(Error::NotFound(unmatched_directory_message(
                        &store.projects()?,
                    )));
                }
            }
        }
    };

    if projects.is_empty() {
        return Ok("No projects registered.\n".to_string());
    }

    let mut out = String::new();
    let settings = home.load_settings()?;
    let stale_after = settings.poll_interval().saturating_mul(3);
    if let Some(scope) = daemon_build_mismatch(home, now, stale_after) {
        out.push_str(&format!(
            "the running depotd (pid {}) was built from commit {}, but this depot is built from {}: restart it with `depot daemon restart`\n\n",
            scope.pid,
            scope.build_id,
            crate::BUILD_ID
        ));
    }
    for (index, project) in projects.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let state = store.project_state(project)?;
        let covered = daemon_scope_covers(home, &project.slug, now, stale_after)?;
        if !covered && needs_daemon(&state) {
            out.push_str("no daemon is driving this project\n\n");
        }
        out.push_str(&render_checklist_observed(&state, history, now));
    }
    Ok(out)
}

fn needs_daemon(state: &ProjectState) -> bool {
    state
        .tasks
        .values()
        .any(|task| task.state == TaskState::Approved || task.state.in_flight())
}

struct Resolved {
    id: ProjectId,
    kind: LocationKind,
    directory: Option<PathBuf>,
}

fn resolve_target(target: &str) -> Result<Resolved> {
    let target = target.trim();
    if is_url(target) {
        return Ok(Resolved {
            id: ProjectId::new(normalize_url(target)),
            kind: LocationKind::Url,
            directory: None,
        });
    }

    let path = PathBuf::from(target);
    let canonical = std::fs::canonicalize(&path)
        .map_err(|_| Error::Project(format!("project path `{}` does not exist", path.display())))?;
    if !canonical.is_dir() {
        return Err(Error::Project(format!(
            "project path `{}` is not a directory",
            canonical.display()
        )));
    }
    Ok(Resolved {
        id: ProjectId::new(canonical.to_string_lossy().to_string()),
        kind: LocationKind::Path,
        directory: Some(canonical),
    })
}

fn ignore_project_config(directory: &Path) -> bool {
    let exclude = directory.join(".git").join("info").join("exclude");
    if !exclude.is_file() {
        return false;
    }
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing
        .lines()
        .any(|line| line.trim() == PROJECT_CONFIG_FILE_NAME)
    {
        return false;
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(PROJECT_CONFIG_FILE_NAME);
    updated.push('\n');
    std::fs::write(&exclude, updated).is_ok()
}

fn unique_slug(taken: &[String], base: &str) -> String {
    if !taken.iter().any(|slug| slug == base) {
        return base.to_string();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !taken.iter().any(|slug| slug == &candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn match_project(store: &Store, name: &str) -> Result<Option<Project>> {
    if let Some(project) = store.project_by_slug(name)? {
        return Ok(Some(project));
    }
    if let Some(project) = store.project(&ProjectId::new(name))? {
        return Ok(Some(project));
    }
    let Ok(canonical) = std::fs::canonicalize(name) else {
        return Ok(None);
    };
    store.project(&ProjectId::new(canonical.to_string_lossy().to_string()))
}

fn project_for_directory(store: &Store, directory: &Path) -> Result<Option<Project>> {
    let directory = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    let projects = store.projects()?;

    if let Some(project) = projects.iter().find(|project| {
        let home = std::fs::canonicalize(store.home().project_root(&project.slug))
            .unwrap_or_else(|_| store.home().project_root(&project.slug));
        directory.starts_with(home)
    }) {
        return Ok(Some(project.clone()));
    }

    let mut matching: Vec<&Project> = projects
        .iter()
        .filter(|project| {
            project.kind == LocationKind::Path
                && directory.starts_with(Path::new(project.id.as_str()))
        })
        .collect();
    matching.sort_by_key(|project| std::cmp::Reverse(project.id.as_str().len()));
    Ok(matching.first().map(|project| (*project).clone()))
}

fn unmatched_directory_message(projects: &[Project]) -> String {
    if projects.is_empty() {
        return "no project matches this directory: no projects are registered; name one with `--project` after `depot project add`, or pass `--all`"
            .to_string();
    }
    let names = projects
        .iter()
        .map(|project| format!("`{}`", project.slug))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "no project matches this directory: registered projects are {names}; run from the project repository or its store, name one with `--project`, or pass `--all`"
    )
}

fn is_url(target: &str) -> bool {
    if target.contains("://") {
        return true;
    }
    match target.split_once(':') {
        Some((host, rest)) => host.contains('@') && !host.contains('/') && !rest.is_empty(),
        None => false,
    }
}

fn normalize_url(url: &str) -> String {
    let url = url.trim();
    let url = url.strip_suffix('/').unwrap_or(url);
    url.strip_suffix(".git").unwrap_or(url).to_string()
}
