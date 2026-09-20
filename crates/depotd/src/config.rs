use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use depot_core::{ProfileId, Role};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::vocabulary::{ROLE_NAMES, role_from_name};

pub const PROJECT_CONFIG_FILE_NAME: &str = ".depot.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub base_branch: String,
    #[serde(default = "default_max_concurrent_tasks")]
    pub max_concurrent_tasks: usize,
    pub profiles: BTreeMap<String, String>,
    pub validation: ValidationConfig,
    pub pull_request: PullRequestConfig,
    pub questions: QuestionsConfig,
    pub dispatch: Option<toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchConfig {
    #[serde(default = "default_confidence_floor")]
    pub confidence_floor: f64,
    pub rules: Vec<DispatchRuleConfig>,
}

pub fn default_confidence_floor() -> f64 {
    0.8
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRuleConfig {
    pub when: String,
    pub role: String,
    #[serde(default)]
    pub candidates: Vec<String>,
}

impl DispatchConfig {
    pub fn validated_rules(&self) -> Result<Vec<depot_core::DispatchRule>> {
        if depot_core::Confidence::new(self.confidence_floor).is_none() {
            return Err(Error::Config(
                ".depot.toml dispatch.confidence_floor must be between 0 and 1".into(),
            ));
        }
        if self.rules.is_empty() {
            return Err(Error::Config(
                ".depot.toml dispatch.rules is empty; configure at least one rule or supply --role"
                    .into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        self.rules.iter().map(|rule| {
            if rule.when.trim().is_empty() || rule.when == crate::adapters::typesafe::NO_MATCH || !seen.insert(&rule.when) {
                return Err(Error::Config(".depot.toml dispatch.rules requires unique, nonempty when strings distinct from the neutral option".into()));
            }
            let role = role_from_name(&rule.role).ok_or_else(|| Error::Config(format!("unknown dispatch rule role `{}` in .depot.toml", rule.role)))?;
            if rule.candidates.iter().any(|candidate| candidate.trim().is_empty()) {
                return Err(Error::Config(".depot.toml dispatch rule contains an empty candidate profile".into()));
            }
            Ok(depot_core::DispatchRule { when: rule.when.clone(), role, candidates: rule.candidates.iter().map(ProfileId::new).collect() })
        }).collect()
    }
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

fn default_max_concurrent_tasks() -> usize {
    1
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            base_branch: "main".to_string(),
            max_concurrent_tasks: default_max_concurrent_tasks(),
            profiles: BTreeMap::new(),
            validation: ValidationConfig::default(),
            pull_request: PullRequestConfig::default(),
            questions: QuestionsConfig::default(),
            dispatch: None,
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
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if project_dir.is_dir() {
                    Ok(Self::default())
                } else {
                    Err(Error::Project(format!(
                        "project path `{}` does not exist",
                        project_dir.display()
                    )))
                }
            }
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
