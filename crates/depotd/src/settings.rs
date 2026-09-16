use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use depot_core::{Limits, ProfileId};
use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub concurrency: usize,
    pub run_duration_minutes: u64,
    pub poll_interval_seconds: u64,
    pub pool_root: Option<PathBuf>,
    pub fallback_profiles: Vec<String>,
    pub credentials: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            concurrency: 4,
            run_duration_minutes: 60,
            poll_interval_seconds: 30,
            pool_root: None,
            fallback_profiles: Vec::new(),
            credentials: BTreeMap::new(),
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
            ..Limits::default()
        }
    }

    pub fn profile_fallbacks(&self) -> Vec<ProfileId> {
        self.fallback_profiles.iter().map(ProfileId::new).collect()
    }
}
