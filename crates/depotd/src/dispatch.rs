use depot_core::{
    Confidence, DispatchRefusal, DispatchResolution, Fact, FactKind, TaskId, resolve_dispatch,
};
use sha2::{Digest, Sha256};

use crate::adapters::typesafe::{RuleMatcher, Typesafe, read_key};
use crate::commands::TaskRequest;
use crate::error::{Error, Result};
use crate::project::Project;
use crate::store::Store;

pub fn judge(
    store: &Store,
    project: &Project,
    id: &TaskId,
    request: &TaskRequest,
) -> Result<(DispatchResolution, Fact)> {
    let key = read_key(&store.home().secrets_dir())?;
    let config = store.project_config(project)?;
    let project_file = match project.kind {
        crate::project::LocationKind::Path => crate::project_file::ProjectFile::load_optional(
            std::path::Path::new(project.id.as_str()),
        )?,
        crate::project::LocationKind::Url => None,
    };
    let raw_dispatch = project_file
        .as_ref()
        .and_then(|file| file.dispatch.clone())
        .or_else(|| config.dispatch.clone())
        .ok_or_else(|| {
            Error::Config(
                "no dispatch.rules in .agni/project.toml or .depot.toml; configure [[dispatch.rules]] or supply --role"
                    .into(),
            )
        })?;
    let dispatch: crate::config::DispatchConfig = raw_dispatch.try_into().map_err(|error| {
        Error::Config(format!(
            "invalid dispatch.rules in .depot.toml: {error}; supply --role to override"
        ))
    })?;
    let rules = dispatch.validated_rules()?;
    let profiles = config.profiles()?;
    let settings = store.home().load_settings()?;
    for rule in &rules {
        if rule.candidates.is_empty() && !profiles.contains_key(&rule.role) {
            return Err(Error::Config(format!(
                "dispatch rule `{}` has no candidates and its role has no [profiles] mapping",
                rule.when
            )));
        }
        for candidate in &rule.candidates {
            settings.profile_spec(candidate.clone())?;
        }
    }
    let snapshot =
        serde_json::to_string(&dispatch).map_err(|error| Error::Config(error.to_string()))?;
    let hash = format!("{:x}", Sha256::digest(snapshot.as_bytes()));
    let conditions = rules
        .iter()
        .map(|rule| rule.when.clone())
        .collect::<Vec<_>>();
    let answer = Typesafe::new(&settings.typesafe_base_url, key).choose(
        &project.slug,
        &request.title,
        &request.intent,
        &conditions,
    )?;
    let floor = Confidence::new(dispatch.confidence_floor)
        .ok_or_else(|| Error::Config("invalid dispatch confidence floor".into()))?;
    let resolution = resolve_dispatch(&rules, answer.chosen, answer.confidence, floor, &profiles)
        .map_err(|reason| {
        Error::Config(match reason {
            DispatchRefusal::NoMatchingRule => {
                "dispatch refused: no matching rule; supply --role to override".into()
            }
            DispatchRefusal::BelowConfidenceFloor => format!(
                "dispatch refused: confidence {} below floor {}; supply --role to override",
                answer.confidence.value(),
                floor.value()
            ),
            DispatchRefusal::UnmappedRole(role) => {
                format!("dispatch refused: unmapped role {role:?}; configure [profiles]")
            }
        })
    })?;
    settings.profile_spec(resolution.profile.clone())?;
    let judgement = Fact {
        at: crate::clock::now(),
        kind: FactKind::TaskDispatchJudged {
            task: id.clone(),
            chosen_rule: answer.chosen,
            confidence: answer.confidence,
            model: answer.model,
            model_version: answer.model_version,
            rules_hash: hash,
            rules_snapshot: snapshot,
            resolution: resolution.clone(),
        },
    };
    Ok((resolution, judgement))
}
