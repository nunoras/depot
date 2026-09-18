use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use depot_core::Confidence;
use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Result};

pub const DEFAULT_API_BASE: &str = "https://api.typesafe.ai";
pub const MODEL: &str = "jev-latest";
pub const KEY_FILE: &str = "typesafe-key";
pub const NO_MATCH: &str = "This work matches none of the conditions above.";

pub struct ChoiceAnswer {
    pub chosen: Option<usize>,
    pub confidence: Confidence,
    pub model: String,
    pub model_version: String,
}

pub trait RuleMatcher {
    fn choose(
        &self,
        project: &str,
        title: &str,
        intent: &str,
        conditions: &[String],
    ) -> Result<ChoiceAnswer>;
}

pub struct Typesafe {
    api: String,
    key: String,
    agent: ureq::Agent,
}

impl Typesafe {
    pub fn new(api: &str, key: String) -> Self {
        Self {
            api: api.trim_end_matches('/').into(),
            key,
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .into(),
        }
    }
}

impl RuleMatcher for Typesafe {
    fn choose(
        &self,
        project: &str,
        title: &str,
        intent: &str,
        conditions: &[String],
    ) -> Result<ChoiceAnswer> {
        let criteria: BTreeMap<&str, Option<&str>> = conditions
            .iter()
            .map(|condition| (condition.as_str(), None))
            .chain([(NO_MATCH, None)])
            .collect();
        let body = json!({
            "model": MODEL,
            "state": {"project": project, "title": title, "intent": intent},
            "questions": {"dispatch": {"type": "choice", "instructions": "Which condition best matches this work?", "criteria": criteria}}
        });
        let mut response = self
            .agent
            .post(format!("{}/v1/systemone", self.api))
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|_| failure("request failed (connection, timeout, or redirect)"))?;
        if !response.status().is_success() {
            return Err(failure(&format!("HTTP {}", response.status().as_u16())));
        }
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|_| failure("unreadable response"))?;
        let response: Response =
            serde_json::from_str(&text).map_err(|_| failure("malformed response"))?;
        let answer = response
            .answers
            .get("dispatch")
            .ok_or_else(|| failure("missing dispatch answer"))?;
        if answer.kind != "choice" || response.model.trim().is_empty() {
            return Err(failure("malformed choice or missing model version"));
        }
        let confidence =
            Confidence::new(answer.confidence).ok_or_else(|| failure("invalid confidence"))?;
        let chosen = if answer.choice == NO_MATCH {
            None
        } else {
            Some(
                conditions
                    .iter()
                    .position(|condition| condition == &answer.choice)
                    .ok_or_else(|| failure("unknown choice"))?,
            )
        };
        if answer.probabilities.len() != criteria.len()
            || criteria
                .keys()
                .any(|key| !answer.probabilities.contains_key(*key))
            || answer
                .probabilities
                .values()
                .any(|value| Confidence::new(*value).is_none())
            || (answer.probabilities.values().sum::<f64>() - 1.0).abs() > 0.0001
        {
            return Err(failure("invalid choice probabilities"));
        }
        Ok(ChoiceAnswer {
            chosen,
            confidence,
            model: MODEL.into(),
            model_version: response.model,
        })
    }
}

#[derive(Deserialize)]
struct Response {
    model: String,
    answers: BTreeMap<String, Answer>,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(rename = "type")]
    kind: String,
    choice: String,
    confidence: f64,
    probabilities: BTreeMap<String, f64>,
}

pub fn read_key(home: &Path) -> Result<String> {
    let path = home.join(KEY_FILE);
    crate::adapters::forge::check_owner_only(&path)
        .map_err(|error| failure(&format!("key file {}: {error}", path.display())))?;
    let key = std::fs::read_to_string(&path)
        .map_err(|error| failure(&format!("cannot read key file {}: {error}", path.display())))?;
    if key.trim().is_empty() {
        return Err(failure(&format!("key file {} is empty", path.display())));
    }
    Ok(key.trim().into())
}

fn failure(reason: &str) -> Error {
    Error::Config(format!(
        "Typesafe dispatch refused: {reason}; supply --role to override"
    ))
}
