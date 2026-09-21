use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use depot_core::{Limits, ProfileId, Role};
use serde::{Deserialize, Serialize};

use crate::adapters::profiles::{ConfiguredProfiles, ProfileError, ProfileSpec, RoleEntry};
use crate::config::ProjectConfig;
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub typesafe_base_url: String,
    pub concurrency: usize,
    pub run_duration_minutes: u64,
    pub poll_interval_seconds: u64,
    pub pool_root: Option<PathBuf>,
    pub fallback_profiles: Vec<String>,
    pub coordinator_context_tokens: u64,
    pub credentials: BTreeMap<String, String>,
    pub profiles: BTreeMap<String, ProfileSettings>,
    pub on_event: Option<OnEventSettings>,
    pub artifacts: ArtifactsSettings,
}

pub const DEFAULT_ON_EVENTS: [&str; 3] = ["question", "failed", "merge_refused"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArtifactsSettings {
    pub publish_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnEventSettings {
    pub command: String,
    pub events: Option<Vec<String>>,
}

impl OnEventSettings {
    pub fn includes(&self, event: &str) -> bool {
        match &self.events {
            Some(events) => events.iter().any(|name| name == event),
            None => DEFAULT_ON_EVENTS.contains(&event),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSettings {
    pub harness: String,
    pub model: String,
    pub effort: String,
    pub account: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            typesafe_base_url: crate::adapters::typesafe::DEFAULT_API_BASE.into(),
            concurrency: 4,
            run_duration_minutes: 60,
            poll_interval_seconds: 30,
            pool_root: None,
            fallback_profiles: Vec::new(),
            coordinator_context_tokens: 120_000,
            credentials: BTreeMap::new(),
            profiles: BTreeMap::new(),
            on_event: None,
            artifacts: ArtifactsSettings::default(),
        }
    }
}

impl Settings {
    pub fn from_toml(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn run_duration(&self) -> Duration {
        Duration::from_secs(self.run_duration_minutes.saturating_mul(60))
    }

    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_seconds)
    }

    pub fn limits(&self) -> Limits {
        Limits {
            max_concurrent_tasks: self.concurrency,
            coordinator_context_tokens: self.coordinator_context_tokens,
            ..Limits::default()
        }
    }

    pub fn profile_fallbacks(&self) -> Vec<ProfileId> {
        self.fallback_profiles.iter().map(ProfileId::new).collect()
    }

    pub fn missing_profiles(&self, config: &ProjectConfig) -> Result<Vec<(Role, ProfileId)>> {
        let fallbacks = self.profile_fallbacks();
        let mut missing = Vec::new();
        for (role, profile) in config.profiles()? {
            let resolvable = std::iter::once(&profile)
                .chain(fallbacks.iter())
                .any(|name| self.profiles.contains_key(name.as_str()));
            if !resolvable {
                missing.push((role, profile));
            }
        }
        Ok(missing)
    }

    pub fn configured_profiles(&self, config: &ProjectConfig) -> Result<ConfiguredProfiles> {
        let entries = config
            .profiles()?
            .into_iter()
            .map(|(role, profile)| {
                let mut names = vec![profile];
                names.extend(self.profile_fallbacks());
                let profiles = names
                    .into_iter()
                    .map(|name| self.profile_spec(name))
                    .collect::<Result<Vec<_>>>()?;
                Ok(RoleEntry { role, profiles })
            })
            .collect::<Result<Vec<_>>>()?;
        ConfiguredProfiles::from_entries(entries).map_err(profile_error)
    }

    pub(crate) fn profile_spec(&self, name: ProfileId) -> Result<ProfileSpec> {
        let profile = self.profiles.get(name.as_str()).ok_or_else(|| {
            Error::Config(format!(
                "profile `{name}` is not defined in machine-local settings"
            ))
        })?;
        let fields = [
            ("harness", profile.harness.as_str()),
            ("model", profile.model.as_str()),
            ("effort", profile.effort.as_str()),
        ];
        if let Some((field, _)) = fields.iter().find(|(_, value)| value.trim().is_empty()) {
            return Err(Error::Config(format!(
                "profile `{name}` in machine-local settings has no {field}"
            )));
        }
        Ok(ProfileSpec {
            profile: name,
            harness: profile.harness.clone(),
            model: profile.model.clone(),
            effort: profile.effort.clone(),
            account: profile.account.clone(),
        })
    }
}

fn profile_error(error: ProfileError) -> Error {
    Error::Config(error.to_string())
}
