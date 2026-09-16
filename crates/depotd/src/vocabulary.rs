use depot_core::{AnsweredBy, ArtifactKind, AttemptOutcome, Checks, Role, TaskState};

use crate::error::{Error, Result};

pub const ROLE_NAMES: [&str; 4] = ["plan", "build", "review", "fix"];

pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Plan => "plan",
        Role::Build => "build",
        Role::Review => "review",
        Role::Fix => "fix",
    }
}

pub fn role_from_name(name: &str) -> Option<Role> {
    match name {
        "plan" => Some(Role::Plan),
        "build" => Some(Role::Build),
        "review" => Some(Role::Review),
        "fix" => Some(Role::Fix),
        _ => None,
    }
}

pub fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Proposed => "proposed",
        TaskState::Approved => "approved",
        TaskState::Running => "running",
        TaskState::WaitingOnQuestion => "waiting_on_question",
        TaskState::Validating => "validating",
        TaskState::Validated => "validated",
        TaskState::PrOpen => "pr_open",
        TaskState::Landed => "landed",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
    }
}

pub fn state_from_name(name: &str) -> Result<TaskState> {
    match name {
        "proposed" => Ok(TaskState::Proposed),
        "approved" => Ok(TaskState::Approved),
        "running" => Ok(TaskState::Running),
        "waiting_on_question" => Ok(TaskState::WaitingOnQuestion),
        "validating" => Ok(TaskState::Validating),
        "validated" => Ok(TaskState::Validated),
        "pr_open" => Ok(TaskState::PrOpen),
        "landed" => Ok(TaskState::Landed),
        "failed" => Ok(TaskState::Failed),
        "cancelled" => Ok(TaskState::Cancelled),
        other => Err(Error::Schema(format!("unknown task state `{other}`"))),
    }
}

pub fn outcome_name(outcome: AttemptOutcome) -> &'static str {
    match outcome {
        AttemptOutcome::InFlight => "in_flight",
        AttemptOutcome::Submitted => "submitted",
        AttemptOutcome::Stopped => "stopped",
        AttemptOutcome::Failed => "failed",
        AttemptOutcome::Unknown => "unknown",
    }
}

pub fn outcome_from_name(name: &str) -> Result<AttemptOutcome> {
    match name {
        "in_flight" => Ok(AttemptOutcome::InFlight),
        "submitted" => Ok(AttemptOutcome::Submitted),
        "stopped" => Ok(AttemptOutcome::Stopped),
        "failed" => Ok(AttemptOutcome::Failed),
        "unknown" => Ok(AttemptOutcome::Unknown),
        other => Err(Error::Schema(format!("unknown attempt outcome `{other}`"))),
    }
}

pub fn checks_name(checks: Checks) -> &'static str {
    match checks {
        Checks::Unknown => "unknown",
        Checks::Pending => "pending",
        Checks::Passing => "passing",
        Checks::Failing => "failing",
    }
}

pub fn checks_from_name(name: &str) -> Result<Checks> {
    match name {
        "unknown" => Ok(Checks::Unknown),
        "pending" => Ok(Checks::Pending),
        "passing" => Ok(Checks::Passing),
        "failing" => Ok(Checks::Failing),
        other => Err(Error::Schema(format!("unknown checks state `{other}`"))),
    }
}

pub fn answered_by_name(by: AnsweredBy) -> &'static str {
    match by {
        AnsweredBy::Coordinator => "coordinator",
        AnsweredBy::User => "user",
    }
}

pub fn answered_by_from_name(name: &str) -> Result<AnsweredBy> {
    match name {
        "coordinator" => Ok(AnsweredBy::Coordinator),
        "user" => Ok(AnsweredBy::User),
        other => Err(Error::Schema(format!("unknown answerer `{other}`"))),
    }
}

pub fn artifact_kind_name(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Brief => "brief",
        ArtifactKind::Evidence => "evidence",
        ArtifactKind::Media => "media",
    }
}

pub fn artifact_kind_from_name(name: &str) -> Result<ArtifactKind> {
    match name {
        "brief" => Ok(ArtifactKind::Brief),
        "evidence" => Ok(ArtifactKind::Evidence),
        "media" => Ok(ArtifactKind::Media),
        other => Err(Error::Schema(format!("unknown artifact kind `{other}`"))),
    }
}
