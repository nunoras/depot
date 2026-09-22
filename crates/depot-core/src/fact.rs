use std::time::Duration;

use crate::model::{
    AnsweredBy, Checks, CommitId, Dependency, ProfileId, Role, SessionId, TaskId, Timestamp,
    WorktreeLease,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Liveness {
    Live,
    Gone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub at: Timestamp,
    pub kind: FactKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactKind {
    TaskDispatchJudged {
        task: TaskId,
        chosen_rule: Option<usize>,
        confidence: crate::Confidence,
        model: String,
        model_version: String,
        rules_hash: String,
        rules_snapshot: String,
        resolution: crate::DispatchResolution,
    },
    TaskProposed {
        task: TaskId,
        title: String,
        intent: String,
        role: Role,
        dispatch_profile: Option<ProfileId>,
        dependencies: Vec<Dependency>,
        base_dependency: Option<TaskId>,
        hold_pr: bool,
    },
    TaskReleased {
        task: TaskId,
    },
    TaskApproved {
        task: TaskId,
    },
    TaskCancelled {
        task: TaskId,
    },
    TaskAcknowledged {
        task: TaskId,
    },
    TaskRetried {
        task: TaskId,
    },
    TaskReworked {
        task: TaskId,
        fix: TaskId,
        text: String,
    },
    QuestionAsked {
        task: TaskId,
        text: String,
        relay: bool,
    },
    QuestionAnswered {
        task: TaskId,
        answer: String,
        by: AnsweredBy,
    },
    WorktreeAcquireRequested {
        task: TaskId,
    },
    WorkerTurnLaunchRequested {
        task: TaskId,
    },
    WorkerTurnResumeRequested {
        task: TaskId,
    },
    WorkerRelaunchRequested {
        task: TaskId,
    },
    WorkerTurnUnresolved {
        task: TaskId,
        reason: String,
    },
    WorkerTurnStarted {
        task: TaskId,
        session: SessionId,
    },
    WorkerTurnEnded {
        task: TaskId,
    },
    WorkerRedirected {
        task: TaskId,
        text: String,
    },
    WorkerRedirectDelivered {
        task: TaskId,
        redirect: String,
    },
    WorkerLivenessChanged {
        task: TaskId,
        liveness: Liveness,
    },
    WorkerSessionFailed {
        task: TaskId,
        reason: String,
    },
    WorkerTurnDeferred {
        task: TaskId,
        reason: String,
    },
    WorkerSubmissionRecorded {
        task: TaskId,
        summary: String,
        artifacts: Vec<String>,
    },
    WorkerSubmitted {
        task: TaskId,
        commit: CommitId,
    },
    ValidationStarted {
        task: TaskId,
        commit: CommitId,
    },
    ProjectFileChanged {
        task: TaskId,
        commit: CommitId,
        files: Vec<String>,
    },
    ValidationFinished {
        task: TaskId,
        command: String,
        commit: CommitId,
        base_commit: Option<CommitId>,
        exit_code: i32,
        duration: Duration,
        output_tail: String,
    },
    ValidationFailed {
        task: TaskId,
        commit: CommitId,
        reason: String,
    },
    WorktreeAcquired {
        task: TaskId,
        lease: WorktreeLease,
        baseline: crate::action::Baseline,
        included: Vec<Dependency>,
    },
    WorktreeReleased {
        task: TaskId,
        lease: WorktreeLease,
    },
    WorktreeReleaseHeld {
        task: TaskId,
        lease: WorktreeLease,
        reason: String,
    },
    BranchPushed {
        task: TaskId,
        commit: CommitId,
    },
    PushFailed {
        task: TaskId,
        commit: CommitId,
        reason: String,
    },
    DeliveryFailed {
        task: TaskId,
        commit: CommitId,
        reason: String,
    },
    TaskLandedOnBase {
        task: TaskId,
        commit: CommitId,
    },
    PullRequestOpened {
        task: TaskId,
        number: u64,
        url: String,
    },
    DescribeFailed {
        task: TaskId,
        reason: String,
    },
    PullRequestChecksChanged {
        task: TaskId,
        checks: Checks,
    },
    PullRequestMerged {
        task: TaskId,
        commit: CommitId,
    },
    PullRequestClosedUnmerged {
        task: TaskId,
    },
    PullRequestMergeRefused {
        task: TaskId,
        commit: CommitId,
        base: CommitId,
        checks: Checks,
        reason: String,
    },
    PullRequestMergeabilityChanged {
        task: TaskId,
        mergeable: bool,
        base: CommitId,
    },
    EvidencePosted {
        task: TaskId,
        commit: CommitId,
    },
    EvidenceFailed {
        task: TaskId,
        commit: CommitId,
        reason: String,
        required: bool,
    },
    RebaseScheduled {
        task: TaskId,
        profile: ProfileId,
        commit: CommitId,
        base: CommitId,
    },
    RunDurationExceeded {
        task: TaskId,
    },
    RetryExhausted {
        task: TaskId,
    },
    ProviderRateLimited {
        task: TaskId,
        profile: ProfileId,
    },
    OnEventNotified {
        task: TaskId,
        event: String,
    },
    CoordinatorSessionStarted {
        session: SessionId,
    },
    CoordinatorContextMeasured {
        tokens: u64,
    },
    DaemonRestarted,
    Polled,
}
