use std::collections::BTreeMap;
use std::fmt;

use depot_core::{ProfileId, Role};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSpec {
    pub profile: ProfileId,
    pub harness: String,
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleEntry {
    pub role: Role,
    pub profiles: Vec<ProfileSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRole {
    pub role: Role,
    pub primary: ProfileSpec,
    pub fallbacks: Vec<ProfileSpec>,
}

pub trait ProfileResolver {
    fn resolve(&self, role: Role) -> Result<ResolvedRole, ProfileError>;
}

#[derive(Debug, Clone, Default)]
pub struct ConfiguredProfiles {
    roles: BTreeMap<Role, Vec<ProfileSpec>>,
}

impl ConfiguredProfiles {
    pub fn from_entries(
        entries: impl IntoIterator<Item = RoleEntry>,
    ) -> Result<Self, ProfileError> {
        let mut roles: BTreeMap<Role, Vec<ProfileSpec>> = BTreeMap::new();
        for entry in entries {
            if roles.contains_key(&entry.role) {
                return Err(ProfileError::DuplicateRole { role: entry.role });
            }
            if entry.profiles.is_empty() {
                return Err(ProfileError::NoProfiles { role: entry.role });
            }
            for spec in &entry.profiles {
                if let Some(field) = missing_field(spec) {
                    return Err(ProfileError::IncompleteProfile {
                        profile: spec.profile.clone(),
                        field,
                    });
                }
            }
            roles.insert(entry.role, entry.profiles);
        }
        Ok(Self { roles })
    }

    pub fn roles(&self) -> Vec<Role> {
        self.roles.keys().copied().collect()
    }
}

impl ProfileResolver for ConfiguredProfiles {
    fn resolve(&self, role: Role) -> Result<ResolvedRole, ProfileError> {
        let mut profiles = self
            .roles
            .get(&role)
            .cloned()
            .ok_or_else(|| ProfileError::UnmappedRole {
                role,
                configured: self.roles(),
            })?
            .into_iter();
        let primary = profiles.next().expect("a configured role holds a profile");
        Ok(ResolvedRole {
            role,
            primary,
            fallbacks: profiles.collect(),
        })
    }
}

fn missing_field(spec: &ProfileSpec) -> Option<&'static str> {
    if spec.harness.trim().is_empty() {
        return Some("harness");
    }
    if spec.model.trim().is_empty() {
        return Some("model");
    }
    if spec.effort.trim().is_empty() {
        return Some("effort");
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    DuplicateRole {
        role: Role,
    },
    NoProfiles {
        role: Role,
    },
    IncompleteProfile {
        profile: ProfileId,
        field: &'static str,
    },
    UnmappedRole {
        role: Role,
        configured: Vec<Role>,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::DuplicateRole { role } => {
                write!(
                    f,
                    "the role {} is configured more than once",
                    role_name(*role)
                )
            }
            ProfileError::NoProfiles { role } => write!(
                f,
                "the role {} is configured with no profile, so depot cannot resolve it",
                role_name(*role)
            ),
            ProfileError::IncompleteProfile { profile, field } => write!(
                f,
                "the profile {profile} declares no {field}, so depot cannot launch with it"
            ),
            ProfileError::UnmappedRole { role, configured } => {
                let configured = if configured.is_empty() {
                    "none".to_owned()
                } else {
                    configured
                        .iter()
                        .map(|role| role_name(*role))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                write!(
                    f,
                    "no profile is configured for the role {}, which is not a role depot guesses; configured roles: {configured}",
                    role_name(*role)
                )
            }
        }
    }
}

impl std::error::Error for ProfileError {}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Plan => "plan",
        Role::Build => "build",
        Role::Review => "review",
        Role::Fix => "fix",
    }
}
