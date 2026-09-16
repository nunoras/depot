use std::fmt::Display;

use depot_core::{Baseline, FactKind, Liveness};

use crate::vocabulary::{answered_by_name, checks_name, role_name};

pub fn kind_name(kind: &FactKind) -> &'static str {
    match kind {
        FactKind::TaskProposed { .. } => "task_proposed",
        FactKind::TaskApproved { .. } => "task_approved",
        FactKind::TaskCancelled { .. } => "task_cancelled",
        FactKind::QuestionAsked { .. } => "question_asked",
        FactKind::QuestionAnswered { .. } => "question_answered",
        FactKind::WorkerTurnStarted { .. } => "worker_turn_started",
        FactKind::WorkerTurnEnded { .. } => "worker_turn_ended",
        FactKind::WorkerLivenessChanged { .. } => "worker_liveness_changed",
        FactKind::WorkerSubmitted { .. } => "worker_submitted",
        FactKind::ValidationStarted { .. } => "validation_started",
        FactKind::ValidationFinished { .. } => "validation_finished",
        FactKind::WorktreeAcquired { .. } => "worktree_acquired",
        FactKind::BranchPushed { .. } => "branch_pushed",
        FactKind::PullRequestOpened { .. } => "pull_request_opened",
        FactKind::PullRequestChecksChanged { .. } => "pull_request_checks_changed",
        FactKind::PullRequestMerged { .. } => "pull_request_merged",
        FactKind::PullRequestClosedUnmerged { .. } => "pull_request_closed_unmerged",
        FactKind::RunDurationExceeded { .. } => "run_duration_exceeded",
        FactKind::RetryExhausted { .. } => "retry_exhausted",
        FactKind::ProviderRateLimited { .. } => "provider_rate_limited",
        FactKind::DaemonRestarted => "daemon_restarted",
        FactKind::Polled => "polled",
    }
}

pub fn encode_payload(kind: &FactKind) -> String {
    match kind {
        FactKind::TaskProposed {
            task,
            title,
            intent,
            role,
            dependencies,
            base_dependency,
        } => object(vec![
            ("task", quoted(task.as_str())),
            ("title", quoted(title)),
            ("intent", quoted(intent)),
            ("role", quoted(role_name(*role))),
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
        ]),
        FactKind::TaskApproved { task } => task_field(task.as_str()),
        FactKind::TaskCancelled { task } => task_field(task.as_str()),
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
        FactKind::WorkerTurnStarted { task, session } => object(vec![
            ("task", quoted(task.as_str())),
            ("session", quoted(session.as_str())),
        ]),
        FactKind::WorkerTurnEnded { task } => task_field(task.as_str()),
        FactKind::WorkerLivenessChanged { task, liveness } => object(vec![
            ("task", quoted(task.as_str())),
            ("liveness", quoted(liveness_name(*liveness))),
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
        FactKind::RunDurationExceeded { task } => task_field(task.as_str()),
        FactKind::RetryExhausted { task } => task_field(task.as_str()),
        FactKind::ProviderRateLimited { task, profile } => object(vec![
            ("task", quoted(task.as_str())),
            ("profile", quoted(profile.as_str())),
        ]),
        FactKind::DaemonRestarted => object(Vec::new()),
        FactKind::Polled => object(Vec::new()),
    }
}

fn task_field(task: &str) -> String {
    object(vec![("task", quoted(task))])
}

fn liveness_name(liveness: Liveness) -> &'static str {
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
