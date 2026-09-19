use std::path::{Path, PathBuf};

use depot_core::ProjectId;

use crate::checklist::render_checklist;
use crate::clock::now;
use crate::commands::ensure_profiles_resolve;
use crate::config::ProjectConfig;
use crate::error::{Error, Result};
use crate::home::{DepotHome, ProjectHome, slug_for};
use crate::project::{LocationKind, Project};
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub project: Project,
    pub created: bool,
    pub config_path: Option<PathBuf>,
    pub home: ProjectHome,
}

pub fn add_project(home: &DepotHome, target: &str) -> Result<Added> {
    let resolved = resolve_target(target)?;
    let store = Store::open(home)?;

    let config_path = ensure_project_config(&resolved)?;
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
        config_path,
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

pub fn render_status(
    home: &DepotHome,
    selection: &StatusSelection,
    history: bool,
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
    for (index, project) in projects.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&render_checklist(&store.project_state(project)?, history));
    }
    Ok(out)
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

fn ensure_project_config(resolved: &Resolved) -> Result<Option<PathBuf>> {
    let Some(directory) = &resolved.directory else {
        return Ok(None);
    };
    let path = ProjectConfig::path_in(directory);
    if !path.exists() {
        let mut config = ProjectConfig::load(directory)?;
        config.base_branch = detect_base_branch(directory);
        config.write(directory)?;
    }
    Ok(Some(path))
}

fn detect_base_branch(directory: &Path) -> String {
    git_branch(
        directory,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .map(|branch| {
        branch
            .strip_prefix("origin/")
            .unwrap_or(&branch)
            .to_string()
    })
    .or_else(|| git_branch(directory, &["symbolic-ref", "--short", "HEAD"]))
    .unwrap_or_else(|| "main".to_string())
}

fn git_branch(directory: &Path, arguments: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
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
