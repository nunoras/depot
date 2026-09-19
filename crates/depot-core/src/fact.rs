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
    WorkerTurnUnresolved {
        task: TaskId,
    },
    WorkerTurnStarted {
        task: TaskId,
        session: SessionId,
    },
    WorkerTurnEnded {
        task: TaskId,
    },
    WorkerLivenessChanged {
        task: TaskId,
        liveness: Liveness,
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
    ValidationFinished {
        task: TaskId,
        command: String,
        commit: CommitId,
        exit_code: i32,
        duration: Duration,
        output_tail: String,
    },
    WorktreeAcquired {
        task: TaskId,
        lease: WorktreeLease,
        baseline: crate::action::Baseline,
        included: Vec<Dependency>,
    },
    BranchPushed {
        task: TaskId,
        commit: CommitId,
    },
    PullRequestOpened {
        task: TaskId,
        number: u64,
        url: String,
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
    CoordinatorSessionStarted {
        session: SessionId,
    },
    CoordinatorContextMeasured {
        tokens: u64,
    },
    DaemonRestarted,
    Polled,
}
