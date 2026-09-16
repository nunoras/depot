use depot_core::{
    AnsweredBy, ArtifactKind, AttemptOutcome, Checks, FactKind, Role, TaskId, TaskState,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FactTag {
    TaskProposed,
    TaskApproved,
    TaskCancelled,
    QuestionAsked,
    QuestionAnswered,
    WorkerTurnStarted,
    WorkerTurnEnded,
    WorkerLivenessChanged,
    WorkerSubmissionRecorded,
    WorkerSubmitted,
    ValidationStarted,
    ValidationFinished,
    WorktreeAcquired,
    BranchPushed,
    PullRequestOpened,
    PullRequestChecksChanged,
    PullRequestMerged,
    PullRequestClosedUnmerged,
    RunDurationExceeded,
    RetryExhausted,
    ProviderRateLimited,
    CoordinatorSessionStarted,
    CoordinatorContextMeasured,
    DaemonRestarted,
    Polled,
}

pub fn fact_tag(kind: &FactKind) -> FactTag {
    match kind {
        FactKind::TaskProposed { .. } => FactTag::TaskProposed,
        FactKind::TaskApproved { .. } => FactTag::TaskApproved,
        FactKind::TaskCancelled { .. } => FactTag::TaskCancelled,
        FactKind::QuestionAsked { .. } => FactTag::QuestionAsked,
        FactKind::QuestionAnswered { .. } => FactTag::QuestionAnswered,
        FactKind::WorkerTurnStarted { .. } => FactTag::WorkerTurnStarted,
        FactKind::WorkerTurnEnded { .. } => FactTag::WorkerTurnEnded,
        FactKind::WorkerLivenessChanged { .. } => FactTag::WorkerLivenessChanged,
        FactKind::WorkerSubmissionRecorded { .. } => FactTag::WorkerSubmissionRecorded,
        FactKind::WorkerSubmitted { .. } => FactTag::WorkerSubmitted,
        FactKind::ValidationStarted { .. } => FactTag::ValidationStarted,
        FactKind::ValidationFinished { .. } => FactTag::ValidationFinished,
        FactKind::WorktreeAcquired { .. } => FactTag::WorktreeAcquired,
        FactKind::BranchPushed { .. } => FactTag::BranchPushed,
        FactKind::PullRequestOpened { .. } => FactTag::PullRequestOpened,
        FactKind::PullRequestChecksChanged { .. } => FactTag::PullRequestChecksChanged,
        FactKind::PullRequestMerged { .. } => FactTag::PullRequestMerged,
        FactKind::PullRequestClosedUnmerged { .. } => FactTag::PullRequestClosedUnmerged,
        FactKind::RunDurationExceeded { .. } => FactTag::RunDurationExceeded,
        FactKind::RetryExhausted { .. } => FactTag::RetryExhausted,
        FactKind::ProviderRateLimited { .. } => FactTag::ProviderRateLimited,
        FactKind::CoordinatorSessionStarted { .. } => FactTag::CoordinatorSessionStarted,
        FactKind::CoordinatorContextMeasured { .. } => FactTag::CoordinatorContextMeasured,
        FactKind::DaemonRestarted => FactTag::DaemonRestarted,
        FactKind::Polled => FactTag::Polled,
    }
}

pub fn fact_tag_name(tag: FactTag) -> &'static str {
    match tag {
        FactTag::TaskProposed => "task_proposed",
        FactTag::TaskApproved => "task_approved",
        FactTag::TaskCancelled => "task_cancelled",
        FactTag::QuestionAsked => "question_asked",
        FactTag::QuestionAnswered => "question_answered",
        FactTag::WorkerTurnStarted => "worker_turn_started",
        FactTag::WorkerTurnEnded => "worker_turn_ended",
        FactTag::WorkerLivenessChanged => "worker_liveness_changed",
        FactTag::WorkerSubmissionRecorded => "worker_submission_recorded",
        FactTag::WorkerSubmitted => "worker_submitted",
        FactTag::ValidationStarted => "validation_started",
        FactTag::ValidationFinished => "validation_finished",
        FactTag::WorktreeAcquired => "worktree_acquired",
        FactTag::BranchPushed => "branch_pushed",
        FactTag::PullRequestOpened => "pull_request_opened",
        FactTag::PullRequestChecksChanged => "pull_request_checks_changed",
        FactTag::PullRequestMerged => "pull_request_merged",
        FactTag::PullRequestClosedUnmerged => "pull_request_closed_unmerged",
        FactTag::RunDurationExceeded => "run_duration_exceeded",
        FactTag::RetryExhausted => "retry_exhausted",
        FactTag::ProviderRateLimited => "provider_rate_limited",
        FactTag::CoordinatorSessionStarted => "coordinator_session_started",
        FactTag::CoordinatorContextMeasured => "coordinator_context_measured",
        FactTag::DaemonRestarted => "daemon_restarted",
        FactTag::Polled => "polled",
    }
}

pub fn fact_tag_from_name(name: &str) -> Result<FactTag> {
    let tag = match name {
        "task_proposed" => FactTag::TaskProposed,
        "task_approved" => FactTag::TaskApproved,
        "task_cancelled" => FactTag::TaskCancelled,
        "question_asked" => FactTag::QuestionAsked,
        "question_answered" => FactTag::QuestionAnswered,
        "worker_turn_started" => FactTag::WorkerTurnStarted,
        "worker_turn_ended" => FactTag::WorkerTurnEnded,
        "worker_liveness_changed" => FactTag::WorkerLivenessChanged,
        "worker_submission_recorded" => FactTag::WorkerSubmissionRecorded,
        "worker_submitted" => FactTag::WorkerSubmitted,
        "validation_started" => FactTag::ValidationStarted,
        "validation_finished" => FactTag::ValidationFinished,
        "worktree_acquired" => FactTag::WorktreeAcquired,
        "branch_pushed" => FactTag::BranchPushed,
        "pull_request_opened" => FactTag::PullRequestOpened,
        "pull_request_checks_changed" => FactTag::PullRequestChecksChanged,
        "pull_request_merged" => FactTag::PullRequestMerged,
        "pull_request_closed_unmerged" => FactTag::PullRequestClosedUnmerged,
        "run_duration_exceeded" => FactTag::RunDurationExceeded,
        "retry_exhausted" => FactTag::RetryExhausted,
        "provider_rate_limited" => FactTag::ProviderRateLimited,
        "coordinator_session_started" => FactTag::CoordinatorSessionStarted,
        "coordinator_context_measured" => FactTag::CoordinatorContextMeasured,
        "daemon_restarted" => FactTag::DaemonRestarted,
        "polled" => FactTag::Polled,
        other => {
            return Err(Error::Schema(format!("unknown fact kind `{other}`")));
        }
    };
    Ok(tag)
}

pub fn fact_task(kind: &FactKind) -> Option<TaskId> {
    let task = match kind {
        FactKind::TaskProposed { task, .. }
        | FactKind::TaskApproved { task }
        | FactKind::TaskCancelled { task }
        | FactKind::QuestionAsked { task, .. }
        | FactKind::QuestionAnswered { task, .. }
        | FactKind::WorkerTurnStarted { task, .. }
        | FactKind::WorkerTurnEnded { task }
        | FactKind::WorkerLivenessChanged { task, .. }
        | FactKind::WorkerSubmissionRecorded { task, .. }
        | FactKind::WorkerSubmitted { task, .. }
        | FactKind::ValidationStarted { task, .. }
        | FactKind::ValidationFinished { task, .. }
        | FactKind::WorktreeAcquired { task, .. }
        | FactKind::BranchPushed { task, .. }
        | FactKind::PullRequestOpened { task, .. }
        | FactKind::PullRequestChecksChanged { task, .. }
        | FactKind::PullRequestMerged { task, .. }
        | FactKind::PullRequestClosedUnmerged { task }
        | FactKind::RunDurationExceeded { task }
        | FactKind::RetryExhausted { task }
        | FactKind::ProviderRateLimited { task, .. } => task.clone(),
        FactKind::CoordinatorSessionStarted { .. }
        | FactKind::CoordinatorContextMeasured { .. }
        | FactKind::DaemonRestarted
        | FactKind::Polled => return None,
    };
    Some(task)
}

pub fn answered_by_name(by: AnsweredBy) -> &'static str {
    match by {
        AnsweredBy::Coordinator => "coordinator",
        AnsweredBy::User => "user",
    }
}

pub fn answered_by_from_name(name: &str) -> Option<AnsweredBy> {
    match name {
        "coordinator" => Some(AnsweredBy::Coordinator),
        "user" => Some(AnsweredBy::User),
        _ => None,
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
