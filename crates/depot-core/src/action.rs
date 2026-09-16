use crate::model::{CommitId, ProfileId, TaskId, Timestamp, WorktreeLease};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Baseline {
    DefaultBranchHead,
    PinnedCommit(CommitId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    LaunchSession {
        task: TaskId,
        profile: ProfileId,
    },
    ResumeSession {
        task: TaskId,
    },
    StopSession {
        task: TaskId,
    },
    AcquireWorktree {
        task: TaskId,
        baseline: Baseline,
    },
    ReleaseWorktree {
        task: TaskId,
        lease: WorktreeLease,
    },
    RunValidation {
        task: TaskId,
        commit: CommitId,
    },
    Push {
        task: TaskId,
        commit: CommitId,
    },
    OpenPullRequest {
        task: TaskId,
        commit: CommitId,
    },
    RenderChecklist,
    Notify {
        task: TaskId,
    },
    Queue {
        task: TaskId,
        not_before: Option<Timestamp>,
    },
    HoldForUser {
        task: TaskId,
    },
}
