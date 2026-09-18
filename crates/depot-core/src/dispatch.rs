use std::collections::BTreeMap;

use crate::{ProfileId, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confidence(u64);

impl Confidence {
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(Self(if value == 0.0 {
            0.0f64.to_bits()
        } else {
            value.to_bits()
        }))
    }

    pub fn value(self) -> f64 {
        f64::from_bits(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRule {
    pub when: String,
    pub role: Role,
    pub candidates: Vec<ProfileId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchResolution {
    pub role: Role,
    pub profile: ProfileId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchRefusal {
    NoMatchingRule,
    BelowConfidenceFloor,
    UnmappedRole(Role),
}

pub fn resolve_dispatch(
    rules: &[DispatchRule],
    chosen: Option<usize>,
    confidence: Confidence,
    floor: Confidence,
    profiles: &BTreeMap<Role, ProfileId>,
) -> Result<DispatchResolution, DispatchRefusal> {
    let rule = chosen
        .and_then(|index| rules.get(index))
        .ok_or(DispatchRefusal::NoMatchingRule)?;
    if confidence.value() < floor.value() {
        return Err(DispatchRefusal::BelowConfidenceFloor);
    }
    let profile = rule
        .candidates
        .first()
        .or_else(|| profiles.get(&rule.role))
        .ok_or(DispatchRefusal::UnmappedRole(rule.role))?;
    Ok(DispatchResolution {
        role: rule.role,
        profile: profile.clone(),
    })
}
