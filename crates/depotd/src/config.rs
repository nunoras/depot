use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use depot_core::{ProfileId, Role};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::vocabulary::{ROLE_NAMES, role_from_name};

pub const PROJECT_CONFIG_FILE_NAME: &str = ".depot.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub base_branch: String,
    pub profiles: BTreeMap<String, String>,
    pub validation: ValidationConfig,
    pub pull_request: PullRequestConfig,
    pub questions: QuestionsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ValidationConfig {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PullRequestConfig {
    pub base: String,
    pub auto_merge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuestionsConfig {
    pub always_relay: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            base_branch: "main".to_string(),
            profiles: BTreeMap::new(),
            validation: ValidationConfig::default(),
            pull_request: PullRequestConfig::default(),
            questions: QuestionsConfig::default(),
        }
    }
}

impl Default for PullRequestConfig {
    fn default() -> Self {
        Self {
            base: "main".to_string(),
            auto_merge: false,
        }
    }
}

impl ProjectConfig {
    pub fn path_in(project_dir: &Path) -> PathBuf {
        project_dir.join(PROJECT_CONFIG_FILE_NAME)
    }

    pub fn load(project_dir: &Path) -> Result<Self> {
        let path = Self::path_in(project_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn from_toml(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn write(&self, project_dir: &Path) -> Result<PathBuf> {
        let path = Self::path_in(project_dir);
        std::fs::write(&path, self.to_toml()?)?;
        Ok(path)
    }

    pub fn profiles(&self) -> Result<BTreeMap<Role, ProfileId>> {
        let mut profiles = BTreeMap::new();
        for (name, profile) in &self.profiles {
            let role = role_from_name(name).ok_or_else(|| {
                Error::Config(format!(
                    "unknown role `{name}` in {PROJECT_CONFIG_FILE_NAME}: expected one of {}",
                    ROLE_NAMES.join(", ")
                ))
            })?;
            if profile.trim().is_empty() {
                return Err(Error::Config(format!(
                    "role `{name}` in {PROJECT_CONFIG_FILE_NAME} names an empty profile"
                )));
            }
            profiles.insert(role, ProfileId::new(profile.clone()));
        }
        Ok(profiles)
    }
}
