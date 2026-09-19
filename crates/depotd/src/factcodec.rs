use std::fmt::Display;

use depot_core::{Baseline, FactKind, Liveness};

use crate::error::{Error, Result};
use crate::vocabulary::{answered_by_name, checks_name, fact_tag, fact_tag_name, role_name};

pub fn kind_name(kind: &FactKind) -> &'static str {
    fact_tag_name(fact_tag(kind))
}

pub fn encode_payload(kind: &FactKind) -> String {
    match kind {
        FactKind::TaskDispatchJudged {
            task,
            chosen_rule,
            confidence,
            model,
            model_version,
            rules_hash,
            rules_snapshot,
            resolution,
        } => {
            let outcome = serde_json::json!({"role": role_name(resolution.role), "profile": resolution.profile.as_str()});
            serde_json::json!({
                "task": task.as_str(), "source": "model_judgement", "chosen_rule": chosen_rule,
                "confidence": confidence.value(), "model": model, "model_version": model_version,
                "rules_hash": rules_hash, "rules_snapshot": rules_snapshot, "resolution": outcome
            })
            .to_string()
        }
        FactKind::TaskProposed {
            task,
            title,
            intent,
            role,
            dispatch_profile,
            dependencies,
            base_dependency,
            hold_pr,
        } => object(vec![
            ("task", quoted(task.as_str())),
            ("title", quoted(title)),
            ("intent", quoted(intent)),
            ("role", quoted(role_name(*role))),
            (
                "dispatch_profile",
                optional(
                    dispatch_profile
                        .as_ref()
                        .map(|profile| quoted(profile.as_str())),
                ),
            ),
            (
                "dependencies",
                array(
                    dependencies
                        .iter()
                        .map(|dependency| {
                            object(vec![
                                ("task", quoted(dependency.task.as_str())),
                                ("commit", quoted(dependency.commit.as_str())),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "base_dependency",
                optional(base_dependency.as_ref().map(|task| quoted(task.as_str()))),
            ),
            ("hold_pr", boolean(*hold_pr)),
        ]),
        FactKind::TaskReleased { task } => task_field(task.as_str()),
        FactKind::TaskApproved { task } => task_field(task.as_str()),
        FactKind::TaskCancelled { task } => task_field(task.as_str()),
        FactKind::TaskAcknowledged { task } => task_field(task.as_str()),
        FactKind::QuestionAsked { task, text, relay } => object(vec![
            ("task", quoted(task.as_str())),
            ("text", quoted(text)),
            ("relay", boolean(*relay)),
        ]),
        FactKind::QuestionAnswered { task, answer, by } => object(vec![
            ("task", quoted(task.as_str())),
            ("answer", quoted(answer)),
            ("by", quoted(answered_by_name(*by))),
        ]),
        FactKind::WorktreeAcquireRequested { task } => task_field(task.as_str()),
        FactKind::WorkerTurnLaunchRequested { task } => task_field(task.as_str()),
        FactKind::WorkerTurnResumeRequested { task } => task_field(task.as_str()),
        FactKind::WorkerTurnUnresolved { task } => task_field(task.as_str()),
        FactKind::WorkerTurnStarted { task, session } => object(vec![
            ("task", quoted(task.as_str())),
            ("session", quoted(session.as_str())),
        ]),
        FactKind::WorkerTurnEnded { task } => task_field(task.as_str()),
        FactKind::WorkerRedirected { task, text } => object(vec![
            ("task", quoted(task.as_str())),
            ("text", quoted(text)),
        ]),
        FactKind::WorkerLivenessChanged { task, liveness } => object(vec![
            ("task", quoted(task.as_str())),
            ("liveness", quoted(liveness_name(*liveness))),
        ]),
        FactKind::WorkerSubmissionRecorded {
            task,
            summary,
            artifacts,
        } => object(vec![
            ("task", quoted(task.as_str())),
            ("summary", quoted(summary)),
            (
                "artifacts",
                format!(
                    "[{}]",
                    artifacts
                        .iter()
                        .map(|artifact| quoted(artifact))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ),
        ]),
        FactKind::WorkerSubmitted { task, commit } => object(vec![
            ("task", quoted(task.as_str())),
            ("commit", quoted(commit.as_str())),
        ]),
        FactKind::ValidationStarted { task, commit } => object(vec![
            ("task", quoted(task.as_str())),
            ("commit", quoted(commit.as_str())),
        ]),
        FactKind::ValidationFinished {
            task,
            command,
            commit,
            exit_code,
            duration,
            output_tail,
        } => object(vec![
            ("task", quoted(task.as_str())),
            ("command", quoted(command)),
            ("commit", quoted(commit.as_str())),
            ("exit_code", numeric(exit_code)),
            ("duration_millis", numeric(duration.as_millis())),
            ("output_tail", quoted(output_tail)),
        ]),
        FactKind::WorktreeAcquired {
            task,
            lease,
            baseline,
            included,
        } => object(vec![
            ("task", quoted(task.as_str())),
            ("lease", quoted(lease.as_str())),
            (
                "baseline",
                match baseline {
                    Baseline::DefaultBranchHead => quoted("default_branch_head"),
                    Baseline::PinnedCommit(commit) => {
                        object(vec![("pinned_commit", quoted(commit.as_str()))])
                    }
                },
            ),
            (
                "included",
                array(
                    included
                        .iter()
                        .map(|dependency| {
                            object(vec![
                                ("task", quoted(dependency.task.as_str())),
                                ("commit", quoted(dependency.commit.as_str())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        FactKind::BranchPushed { task, commit } => object(vec![
            ("task", quoted(task.as_str())),
            ("commit", quoted(commit.as_str())),
        ]),
        FactKind::PullRequestOpened { task, number, url } => object(vec![
            ("task", quoted(task.as_str())),
            ("number", numeric(number)),
            ("url", quoted(url)),
        ]),
        FactKind::PullRequestChecksChanged { task, checks } => object(vec![
            ("task", quoted(task.as_str())),
            ("checks", quoted(checks_name(*checks))),
        ]),
        FactKind::PullRequestMerged { task, commit } => object(vec![
            ("task", quoted(task.as_str())),
            ("commit", quoted(commit.as_str())),
        ]),
        FactKind::PullRequestClosedUnmerged { task } => task_field(task.as_str()),
        FactKind::PullRequestMergeRefused {
            task,
            commit,
            base,
            checks,
            reason,
        } => object(vec![
            ("task", quoted(task.as_str())),
            ("commit", quoted(commit.as_str())),
            ("base", quoted(base.as_str())),
            ("checks", quoted(checks_name(*checks))),
            ("reason", quoted(reason)),
        ]),
        FactKind::PushFailed {
            task,
            commit,
            reason,
        } => object(vec![
            ("task", quoted(task.as_str())),
            ("commit", quoted(commit.as_str())),
            ("reason", quoted(reason)),
        ]),
        FactKind::RunDurationExceeded { task } => task_field(task.as_str()),
        FactKind::RetryExhausted { task } => task_field(task.as_str()),
        FactKind::ProviderRateLimited { task, profile } => object(vec![
            ("task", quoted(task.as_str())),
            ("profile", quoted(profile.as_str())),
        ]),
        FactKind::OnEventNotified { task, event } => object(vec![
            ("task", quoted(task.as_str())),
            ("event", quoted(event)),
        ]),
        FactKind::CoordinatorSessionStarted { session } => {
            object(vec![("session", quoted(session.as_str()))])
        }
        FactKind::CoordinatorContextMeasured { tokens } => {
            object(vec![("tokens", numeric(tokens))])
        }
        FactKind::DaemonRestarted => object(Vec::new()),
        FactKind::Polled => object(Vec::new()),
    }
}

fn task_field(task: &str) -> String {
    object(vec![("task", quoted(task))])
}

pub fn payload_field(payload: &str, field: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(payload).map_err(|error| Error::Schema(error.to_string()))?;
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Schema(format!("a fact payload carries no {field}")))
}

pub fn liveness_name(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Live => "live",
        Liveness::Gone => "gone",
    }
}

fn object(fields: Vec<(&str, String)>) -> String {
    let body = fields
        .into_iter()
        .map(|(name, value)| format!("{}:{}", quoted(name), value))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

fn array(items: Vec<String>) -> String {
    format!("[{}]", items.join(","))
}

fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            plain => out.push(plain),
        }
    }
    out.push('"');
    out
}

fn numeric(value: impl Display) -> String {
    value.to_string()
}

fn boolean(value: bool) -> String {
    value.to_string()
}

fn optional(value: Option<String>) -> String {
    value.unwrap_or_else(|| "null".to_string())
}
