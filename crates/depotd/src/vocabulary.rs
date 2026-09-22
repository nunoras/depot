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
        TaskState::ReworkPending => "rework_pending",
        TaskState::Held => "held",
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
        "rework_pending" => Ok(TaskState::ReworkPending),
        "held" => Ok(TaskState::Held),
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
        AttemptOutcome::AwaitingAnswer => "awaiting_answer",
    }
}

pub fn outcome_from_name(name: &str) -> Result<AttemptOutcome> {
    match name {
        "in_flight" => Ok(AttemptOutcome::InFlight),
        "submitted" => Ok(AttemptOutcome::Submitted),
        "stopped" => Ok(AttemptOutcome::Stopped),
        "failed" => Ok(AttemptOutcome::Failed),
        "unknown" => Ok(AttemptOutcome::Unknown),
        "awaiting_answer" => Ok(AttemptOutcome::AwaitingAnswer),
        other => Err(Error::Schema(format!("unknown attempt outcome `{other}`"))),
    }
}

pub fn checks_name(checks: Checks) -> &'static str {
    match checks {
        Checks::None => "none",
        Checks::Unknown => "unknown",
        Checks::Pending => "pending",
        Checks::Passing => "passing",
        Checks::Failing => "failing",
    }
}

pub fn checks_from_name(name: &str) -> Result<Checks> {
    match name {
        "none" => Ok(Checks::None),
        "unknown" => Ok(Checks::Unknown),
        "pending" => Ok(Checks::Pending),
        "passing" => Ok(Checks::Passing),
        "failing" => Ok(Checks::Failing),
        other => Err(Error::Schema(format!("unknown checks state `{other}`"))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FactTag {
    TaskDispatchJudged,
    TaskProposed,
    TaskReleased,
    TaskApproved,
    TaskCancelled,
    TaskAcknowledged,
    TaskRetried,
    TaskReworked,
    QuestionAsked,
    QuestionAnswered,
    WorktreeAcquireRequested,
    WorkerTurnLaunchRequested,
    WorkerTurnResumeRequested,
    WorkerRelaunchRequested,
    WorkerTurnUnresolved,
    WorkerTurnStarted,
    WorkerTurnEnded,
    WorkerRedirected,
    WorkerRedirectDelivered,
    WorkerLivenessChanged,
    WorkerSessionFailed,
    WorkerTurnDeferred,
    WorkerSubmissionRecorded,
    WorkerSubmitted,
    WorkerCommittedNothing,
    ValidationStarted,
    ProjectFileChanged,
    ValidationFinished,
    ValidationFailed,
    WorktreeAcquired,
    WorktreeBaselined,
    WorktreeReleased,
    WorktreeReleaseHeld,
    BranchPushed,
    PullRequestOpened,
    TaskLandedOnBase,
    PullRequestChecksChanged,
    PullRequestMerged,
    StaleMergeObserved,
    PullRequestClosedUnmerged,
    PullRequestMergeRefused,
    PullRequestMergeabilityChanged,
    DescribeFailed,
    EvidencePosted,
    EvidenceFailed,
    PushFailed,
    DeliveryFailed,
    RebaseScheduled,
    RunDurationExceeded,
    RetryExhausted,
    ProviderRateLimited,
    OnEventNotified,
    CoordinatorSessionStarted,
    CoordinatorContextMeasured,
    DaemonRestarted,
    Polled,
}

pub fn fact_tag(kind: &FactKind) -> FactTag {
    match kind {
        FactKind::TaskDispatchJudged { .. } => FactTag::TaskDispatchJudged,
        FactKind::TaskProposed { .. } => FactTag::TaskProposed,
        FactKind::TaskReleased { .. } => FactTag::TaskReleased,
        FactKind::TaskApproved { .. } => FactTag::TaskApproved,
        FactKind::TaskCancelled { .. } => FactTag::TaskCancelled,
        FactKind::TaskRetried { .. } => FactTag::TaskRetried,
        FactKind::TaskReworked { .. } => FactTag::TaskReworked,
        FactKind::TaskAcknowledged { .. } => FactTag::TaskAcknowledged,
        FactKind::QuestionAsked { .. } => FactTag::QuestionAsked,
        FactKind::QuestionAnswered { .. } => FactTag::QuestionAnswered,
        FactKind::WorktreeAcquireRequested { .. } => FactTag::WorktreeAcquireRequested,
        FactKind::WorkerTurnLaunchRequested { .. } => FactTag::WorkerTurnLaunchRequested,
        FactKind::WorkerTurnResumeRequested { .. } => FactTag::WorkerTurnResumeRequested,
        FactKind::WorkerRelaunchRequested { .. } => FactTag::WorkerRelaunchRequested,
        FactKind::WorkerTurnUnresolved { .. } => FactTag::WorkerTurnUnresolved,
        FactKind::WorkerTurnStarted { .. } => FactTag::WorkerTurnStarted,
        FactKind::WorkerTurnEnded { .. } => FactTag::WorkerTurnEnded,
        FactKind::WorkerRedirected { .. } => FactTag::WorkerRedirected,
        FactKind::WorkerRedirectDelivered { .. } => FactTag::WorkerRedirectDelivered,
        FactKind::WorkerLivenessChanged { .. } => FactTag::WorkerLivenessChanged,
        FactKind::WorkerSessionFailed { .. } => FactTag::WorkerSessionFailed,
        FactKind::WorkerTurnDeferred { .. } => FactTag::WorkerTurnDeferred,
        FactKind::WorkerSubmissionRecorded { .. } => FactTag::WorkerSubmissionRecorded,
        FactKind::WorkerSubmitted { .. } => FactTag::WorkerSubmitted,
        FactKind::WorkerCommittedNothing { .. } => FactTag::WorkerCommittedNothing,
        FactKind::ValidationStarted { .. } => FactTag::ValidationStarted,
        FactKind::ProjectFileChanged { .. } => FactTag::ProjectFileChanged,
        FactKind::ValidationFinished { .. } => FactTag::ValidationFinished,
        FactKind::ValidationFailed { .. } => FactTag::ValidationFailed,
        FactKind::WorktreeAcquired { .. } => FactTag::WorktreeAcquired,
        FactKind::WorktreeBaselined { .. } => FactTag::WorktreeBaselined,
        FactKind::WorktreeReleased { .. } => FactTag::WorktreeReleased,
        FactKind::WorktreeReleaseHeld { .. } => FactTag::WorktreeReleaseHeld,
        FactKind::BranchPushed { .. } => FactTag::BranchPushed,
        FactKind::PullRequestOpened { .. } => FactTag::PullRequestOpened,
        FactKind::TaskLandedOnBase { .. } => FactTag::TaskLandedOnBase,
        FactKind::PullRequestChecksChanged { .. } => FactTag::PullRequestChecksChanged,
        FactKind::PullRequestMerged { .. } => FactTag::PullRequestMerged,
        FactKind::StaleMergeObserved { .. } => FactTag::StaleMergeObserved,
        FactKind::PullRequestClosedUnmerged { .. } => FactTag::PullRequestClosedUnmerged,
        FactKind::PullRequestMergeRefused { .. } => FactTag::PullRequestMergeRefused,
        FactKind::PullRequestMergeabilityChanged { .. } => FactTag::PullRequestMergeabilityChanged,
        FactKind::DescribeFailed { .. } => FactTag::DescribeFailed,
        FactKind::EvidencePosted { .. } => FactTag::EvidencePosted,
        FactKind::EvidenceFailed { .. } => FactTag::EvidenceFailed,
        FactKind::PushFailed { .. } => FactTag::PushFailed,
        FactKind::DeliveryFailed { .. } => FactTag::DeliveryFailed,
        FactKind::RebaseScheduled { .. } => FactTag::RebaseScheduled,
        FactKind::RunDurationExceeded { .. } => FactTag::RunDurationExceeded,
        FactKind::RetryExhausted { .. } => FactTag::RetryExhausted,
        FactKind::ProviderRateLimited { .. } => FactTag::ProviderRateLimited,
        FactKind::OnEventNotified { .. } => FactTag::OnEventNotified,
        FactKind::CoordinatorSessionStarted { .. } => FactTag::CoordinatorSessionStarted,
        FactKind::CoordinatorContextMeasured { .. } => FactTag::CoordinatorContextMeasured,
        FactKind::DaemonRestarted => FactTag::DaemonRestarted,
        FactKind::Polled => FactTag::Polled,
    }
}

pub fn fact_tag_name(tag: FactTag) -> &'static str {
    match tag {
        FactTag::TaskDispatchJudged => "task_dispatch_judged",
        FactTag::TaskProposed => "task_proposed",
        FactTag::TaskReleased => "task_released",
        FactTag::TaskApproved => "task_approved",
        FactTag::TaskCancelled => "task_cancelled",
        FactTag::TaskRetried => "task_retried",
        FactTag::TaskReworked => "task_reworked",
        FactTag::TaskAcknowledged => "task_acknowledged",
        FactTag::QuestionAsked => "question_asked",
        FactTag::QuestionAnswered => "question_answered",
        FactTag::WorktreeAcquireRequested => "worktree_acquire_requested",
        FactTag::WorkerTurnLaunchRequested => "worker_turn_launch_requested",
        FactTag::WorkerTurnResumeRequested => "worker_turn_resume_requested",
        FactTag::WorkerRelaunchRequested => "worker_relaunch_requested",
        FactTag::WorkerTurnUnresolved => "worker_turn_unresolved",
        FactTag::WorkerTurnStarted => "worker_turn_started",
        FactTag::WorkerTurnEnded => "worker_turn_ended",
        FactTag::WorkerRedirected => "worker_redirected",
        FactTag::WorkerRedirectDelivered => "worker_redirect_delivered",
        FactTag::WorkerLivenessChanged => "worker_liveness_changed",
        FactTag::WorkerSessionFailed => "worker_session_failed",
        FactTag::WorkerTurnDeferred => "worker_turn_deferred",
        FactTag::WorkerSubmissionRecorded => "worker_submission_recorded",
        FactTag::WorkerSubmitted => "worker_submitted",
        FactTag::WorkerCommittedNothing => "worker_committed_nothing",
        FactTag::ValidationStarted => "validation_started",
        FactTag::ProjectFileChanged => "project_file_changed",
        FactTag::ValidationFinished => "validation_finished",
        FactTag::ValidationFailed => "validation_failed",
        FactTag::WorktreeAcquired => "worktree_acquired",
        FactTag::WorktreeBaselined => "worktree_baselined",
        FactTag::WorktreeReleased => "worktree_released",
        FactTag::WorktreeReleaseHeld => "worktree_release_held",
        FactTag::BranchPushed => "branch_pushed",
        FactTag::PullRequestOpened => "pull_request_opened",
        FactTag::TaskLandedOnBase => "task_landed_on_base",
        FactTag::PullRequestChecksChanged => "pull_request_checks_changed",
        FactTag::PullRequestMerged => "pull_request_merged",
        FactTag::StaleMergeObserved => "stale_merge_observed",
        FactTag::PullRequestClosedUnmerged => "pull_request_closed_unmerged",
        FactTag::PullRequestMergeRefused => "pull_request_merge_refused",
        FactTag::PullRequestMergeabilityChanged => "pull_request_mergeability_changed",
        FactTag::DescribeFailed => "describe_failed",
        FactTag::EvidencePosted => "evidence_posted",
        FactTag::EvidenceFailed => "evidence_failed",
        FactTag::PushFailed => "push_failed",
        FactTag::DeliveryFailed => "delivery_failed",
        FactTag::RebaseScheduled => "rebase_scheduled",
        FactTag::RunDurationExceeded => "run_duration_exceeded",
        FactTag::RetryExhausted => "retry_exhausted",
        FactTag::ProviderRateLimited => "provider_rate_limited",
        FactTag::OnEventNotified => "on_event_notified",
        FactTag::CoordinatorSessionStarted => "coordinator_session_started",
        FactTag::CoordinatorContextMeasured => "coordinator_context_measured",
        FactTag::DaemonRestarted => "daemon_restarted",
        FactTag::Polled => "polled",
    }
}

pub fn fact_tag_from_name(name: &str) -> Result<FactTag> {
    let tag = match name {
        "task_dispatch_judged" => FactTag::TaskDispatchJudged,
        "task_proposed" => FactTag::TaskProposed,
        "task_released" => FactTag::TaskReleased,
        "task_approved" => FactTag::TaskApproved,
        "task_cancelled" => FactTag::TaskCancelled,
        "task_retried" => FactTag::TaskRetried,
        "task_reworked" => FactTag::TaskReworked,
        "task_acknowledged" => FactTag::TaskAcknowledged,
        "question_asked" => FactTag::QuestionAsked,
        "question_answered" => FactTag::QuestionAnswered,
        "worktree_acquire_requested" => FactTag::WorktreeAcquireRequested,
        "worker_turn_launch_requested" => FactTag::WorkerTurnLaunchRequested,
        "worker_turn_resume_requested" => FactTag::WorkerTurnResumeRequested,
        "worker_turn_unresolved" => FactTag::WorkerTurnUnresolved,
        "worker_turn_started" => FactTag::WorkerTurnStarted,
        "worker_turn_ended" => FactTag::WorkerTurnEnded,
        "worker_redirected" => FactTag::WorkerRedirected,
        "worker_redirect_delivered" => FactTag::WorkerRedirectDelivered,
        "worker_liveness_changed" => FactTag::WorkerLivenessChanged,
        "worker_session_failed" => FactTag::WorkerSessionFailed,
        "worker_turn_deferred" => FactTag::WorkerTurnDeferred,
        "worker_relaunch_requested" => FactTag::WorkerRelaunchRequested,
        "worker_submission_recorded" => FactTag::WorkerSubmissionRecorded,
        "worker_submitted" => FactTag::WorkerSubmitted,
        "worker_committed_nothing" => FactTag::WorkerCommittedNothing,
        "validation_started" => FactTag::ValidationStarted,
        "project_file_changed" => FactTag::ProjectFileChanged,
        "validation_finished" => FactTag::ValidationFinished,
        "validation_failed" => FactTag::ValidationFailed,
        "worktree_acquired" => FactTag::WorktreeAcquired,
        "worktree_baselined" => FactTag::WorktreeBaselined,
        "worktree_released" => FactTag::WorktreeReleased,
        "worktree_release_held" => FactTag::WorktreeReleaseHeld,
        "branch_pushed" => FactTag::BranchPushed,
        "pull_request_opened" => FactTag::PullRequestOpened,
        "task_landed_on_base" => FactTag::TaskLandedOnBase,
        "pull_request_checks_changed" => FactTag::PullRequestChecksChanged,
        "pull_request_merged" => FactTag::PullRequestMerged,
        "stale_merge_observed" => FactTag::StaleMergeObserved,
        "pull_request_closed_unmerged" => FactTag::PullRequestClosedUnmerged,
        "pull_request_merge_refused" => FactTag::PullRequestMergeRefused,
        "pull_request_mergeability_changed" => FactTag::PullRequestMergeabilityChanged,
        "describe_failed" => FactTag::DescribeFailed,
        "evidence_posted" => FactTag::EvidencePosted,
        "evidence_failed" => FactTag::EvidenceFailed,
        "push_failed" => FactTag::PushFailed,
        "delivery_failed" => FactTag::DeliveryFailed,
        "rebase_scheduled" => FactTag::RebaseScheduled,
        "run_duration_exceeded" => FactTag::RunDurationExceeded,
        "retry_exhausted" => FactTag::RetryExhausted,
        "provider_rate_limited" => FactTag::ProviderRateLimited,
        "on_event_notified" => FactTag::OnEventNotified,
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
        FactKind::TaskDispatchJudged { task, .. }
        | FactKind::TaskProposed { task, .. }
        | FactKind::TaskReleased { task }
        | FactKind::TaskApproved { task }
        | FactKind::TaskCancelled { task }
        | FactKind::TaskRetried { task }
        | FactKind::TaskReworked { task, .. }
        | FactKind::TaskAcknowledged { task }
        | FactKind::QuestionAsked { task, .. }
        | FactKind::QuestionAnswered { task, .. }
        | FactKind::WorktreeAcquireRequested { task }
        | FactKind::WorkerTurnLaunchRequested { task }
        | FactKind::WorkerTurnResumeRequested { task }
        | FactKind::WorkerRelaunchRequested { task }
        | FactKind::WorkerTurnUnresolved { task, .. }
        | FactKind::WorkerTurnStarted { task, .. }
        | FactKind::WorkerTurnEnded { task }
        | FactKind::WorkerRedirected { task, .. }
        | FactKind::WorkerRedirectDelivered { task, .. }
        | FactKind::WorkerLivenessChanged { task, .. }
        | FactKind::WorkerSessionFailed { task, .. }
        | FactKind::WorkerTurnDeferred { task, .. }
        | FactKind::WorkerSubmissionRecorded { task, .. }
        | FactKind::WorkerSubmitted { task, .. }
        | FactKind::WorkerCommittedNothing { task, .. }
        | FactKind::ValidationStarted { task, .. }
        | FactKind::ProjectFileChanged { task, .. }
        | FactKind::ValidationFinished { task, .. }
        | FactKind::ValidationFailed { task, .. }
        | FactKind::WorktreeAcquired { task, .. }
        | FactKind::WorktreeBaselined { task, .. }
        | FactKind::WorktreeReleased { task, .. }
        | FactKind::WorktreeReleaseHeld { task, .. }
        | FactKind::BranchPushed { task, .. }
        | FactKind::PullRequestOpened { task, .. }
        | FactKind::TaskLandedOnBase { task, .. }
        | FactKind::PullRequestChecksChanged { task, .. }
        | FactKind::PullRequestMerged { task, .. }
        | FactKind::StaleMergeObserved { task, .. }
        | FactKind::PullRequestMergeRefused { task, .. }
        | FactKind::PullRequestMergeabilityChanged { task, .. }
        | FactKind::PullRequestClosedUnmerged { task }
        | FactKind::DescribeFailed { task, .. }
        | FactKind::EvidencePosted { task, .. }
        | FactKind::EvidenceFailed { task, .. }
        | FactKind::PushFailed { task, .. }
        | FactKind::DeliveryFailed { task, .. }
        | FactKind::RebaseScheduled { task, .. }
        | FactKind::RunDurationExceeded { task }
        | FactKind::RetryExhausted { task }
        | FactKind::ProviderRateLimited { task, .. }
        | FactKind::OnEventNotified { task, .. } => task.clone(),
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
