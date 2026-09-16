use std::env;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::settings::Settings;

pub const HOME_ENV: &str = "DEPOT_HOME";
pub const SETTINGS_FILE_NAME: &str = "config.toml";
pub const DATABASE_FILE_NAME: &str = "depot.db";
pub const PROJECTS_DIR_NAME: &str = "projects";
pub const CHECKLIST_FILE_NAME: &str = "checklist.md";
pub const ARCHIVE_DIR_NAME: &str = "archive";
pub const DOCUMENTS_DIR_NAME: &str = "docs";
pub const SCRATCH_DIR_NAME: &str = "scratch";
pub const MEDIA_DIR_NAME: &str = "media";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepotHome {
    root: PathBuf,
}

impl DepotHome {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn resolve() -> Result<Self> {
        if let Some(root) = env::var_os(HOME_ENV).filter(|value| !value.is_empty()) {
            return Ok(Self::at(PathBuf::from(root)));
        }
        let base = user_home_dir().ok_or_else(|| {
            Error::Home(format!(
                "cannot locate the depot home: set {HOME_ENV} to the directory depot should use"
            ))
        })?;
        Ok(Self::at(base.join(".depot")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(SETTINGS_FILE_NAME)
    }

    pub fn database_path(&self) -> PathBuf {
        self.root.join(DATABASE_FILE_NAME)
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.root.join(PROJECTS_DIR_NAME)
    }

    pub fn project_root(&self, slug: &str) -> PathBuf {
        self.projects_dir().join(slug)
    }

    pub fn project_home(&self, slug: &str) -> ProjectHome {
        ProjectHome::at(self.project_root(slug))
    }

    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(self.projects_dir())?;
        if !self.config_path().exists() {
            self.write_settings(&Settings::default())?;
        }
        Ok(())
    }

    pub fn load_settings(&self) -> Result<Settings> {
        let path = self.config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => Settings::from_toml(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn write_settings(&self, settings: &Settings) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.config_path();
        std::fs::write(&path, settings.to_toml()?)?;
        Ok(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHome {
    root: PathBuf,
}

impl ProjectHome {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn checklist_path(&self) -> PathBuf {
        self.root.join(CHECKLIST_FILE_NAME)
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.root.join(ARCHIVE_DIR_NAME)
    }

    pub fn documents_dir(&self) -> PathBuf {
        self.root.join(DOCUMENTS_DIR_NAME)
    }

    pub fn scratch_dir(&self) -> PathBuf {
        self.root.join(SCRATCH_DIR_NAME)
    }

    pub fn media_dir(&self) -> PathBuf {
        self.root.join(MEDIA_DIR_NAME)
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.archive_dir(),
            self.documents_dir(),
            self.scratch_dir(),
            self.media_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

pub fn slug_for(identity: &str) -> String {
    let tail = identity
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("project");
    let tail = tail.strip_suffix(".git").unwrap_or(tail);

    let slug: String = tail
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(64).collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug
    }
}

fn user_home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .find_map(|key| env::var_os(key).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}
