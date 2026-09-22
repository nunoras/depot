use std::collections::BTreeMap;
use std::time::Duration;

macro_rules! string_id {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
            pub struct $name(String);

            impl $name {
                pub fn new(value: impl Into<String>) -> Self {
                    Self(value.into())
                }

                pub fn as_str(&self) -> &str {
                    &self.0
                }
            }

            impl From<&str> for $name {
                fn from(value: &str) -> Self {
                    Self(value.to_owned())
                }
            }

            impl From<String> for $name {
                fn from(value: String) -> Self {
                    Self(value)
                }
            }

            impl std::fmt::Display for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str(&self.0)
                }
            }
        )+
    };
}

string_id!(
    CommitId,
    ProfileId,
    ProjectId,
    SessionId,
    TaskId,
    WorktreeLease
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(u64);

impl Timestamp {
    pub fn from_millis(millis: u64) -> Self {
        Self(millis)
    }

    pub fn millis(self) -> u64 {
        self.0
    }

    pub fn plus(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration.as_millis() as u64))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Plan,
    Build,
    Review,
    Fix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskState {
    Proposed,
    Approved,
    Running,
    WaitingOnQuestion,
    Validating,
    Validated,
    PrOpen,
    ReworkPending,
    Landed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn in_flight(self) -> bool {
        matches!(
            self,
            TaskState::Running | TaskState::WaitingOnQuestion | TaskState::Validating
        )
    }

    pub fn settles_a_wait(self) -> bool {
        matches!(
            self,
            TaskState::WaitingOnQuestion
                | TaskState::PrOpen
                | TaskState::Landed
                | TaskState::Failed
                | TaskState::Cancelled
        )
    }

    pub fn tracks_pull_request(self) -> bool {
        matches!(self, TaskState::PrOpen | TaskState::ReworkPending)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttemptOutcome {
    InFlight,
    Submitted,
    Stopped,
    Failed,
    AwaitingAnswer,
    Unknown,
}

impl AttemptOutcome {
    pub fn is_open(self) -> bool {
        matches!(self, AttemptOutcome::InFlight | AttemptOutcome::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnsweredBy {
    Coordinator,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Checks {
    None,
    Unknown,
    Pending,
    Passing,
    Failing,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MergePolicy {
    #[default]
    Manual,
    AfterChecks,
    AfterReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtifactKind {
    Brief,
    Evidence,
    Media,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Issue {
        url: String,
    },
    PullRequest {
        number: u64,
        url: String,
        checks: Checks,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub task: TaskId,
    pub commit: CommitId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    pub profile: ProfileId,
    pub not_before: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub session: Option<SessionId>,
    pub profile: ProfileId,
    pub worktree: Option<WorktreeLease>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub outcome: AttemptOutcome,
    pub base_merge: bool,
    pub last_seen_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub by: AnsweredBy,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub text: String,
    pub asked_at: Timestamp,
    pub answer: Option<Answer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorSession {
    pub session: SessionId,
    pub started_at: Timestamp,
    pub context_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub summary: String,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationRecord {
    pub command: String,
    pub commit: CommitId,
    pub base_commit: Option<CommitId>,
    pub exit_code: i32,
    pub duration: Duration,
    pub output_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseHold {
    pub reason: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDeferral {
    pub count: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub project: ProjectId,
    pub title: String,
    pub intent: String,
    pub role: Role,
    pub dispatch_profile: Option<ProfileId>,
    pub state: TaskState,
    pub dependencies: Vec<Dependency>,
    pub base_dependency: Option<TaskId>,
    pub attempts: Vec<Attempt>,
    pub questions: Vec<Question>,
    pub validations: Vec<ValidationRecord>,
    pub submission: Option<Submission>,
    pub artifacts: Vec<Artifact>,
    pub links: Vec<Link>,
    pub branch_head: Option<CommitId>,
    pub merge_refused: Option<String>,
    pub conflict_base: Option<CommitId>,
    pub failure: Option<String>,
    pub redirect_text: Option<String>,
    pub redirect_delivered: bool,
    pub acknowledged_at: Option<Timestamp>,
    pub rework_of: Option<TaskId>,
    pub hold_pr: bool,
    pub release_pending: Vec<WorktreeLease>,
    pub release_held: BTreeMap<WorktreeLease, ReleaseHold>,
    pub turn_deferral: Option<TurnDeferral>,
    pub retry: Option<Retry>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Task {
    pub fn validated_commit(&self) -> Option<&CommitId> {
        let record = self.newest_passing_validation()?;
        match &self.branch_head {
            Some(head) if head != &record.commit => None,
            _ => Some(&record.commit),
        }
    }

    pub fn push_owed(&self) -> Option<&CommitId> {
        let record = self.newest_passing_validation()?;
        (self.branch_head.as_ref() != Some(&record.commit)).then_some(&record.commit)
    }

    fn newest_passing_validation(&self) -> Option<&ValidationRecord> {
        self.validations.iter().rev().find(|r| r.exit_code == 0)
    }

    pub fn unanswered_question(&mut self) -> Option<&mut Question> {
        self.questions.iter_mut().rev().find(|q| q.answer.is_none())
    }

    pub fn has_unanswered_question(&self) -> bool {
        self.questions.iter().any(|q| q.answer.is_none())
    }

    pub fn pull_request(&self) -> Option<(u64, &str, Checks)> {
        self.links.iter().find_map(|link| match link {
            Link::PullRequest {
                number,
                url,
                checks,
            } => Some((*number, url.as_str(), *checks)),
            Link::Issue { .. } => None,
        })
    }

    pub fn returns_its_worktree(&self) -> bool {
        match self.state {
            TaskState::Cancelled | TaskState::Landed => true,
            TaskState::Failed => self.acknowledged_at.is_some(),
            _ => false,
        }
    }
}

impl Default for Task {
    fn default() -> Self {
        Self {
            id: TaskId::new(String::new()),
            project: ProjectId::new(String::new()),
            title: String::new(),
            intent: String::new(),
            role: Role::Build,
            dispatch_profile: None,
            state: TaskState::Proposed,
            dependencies: Vec::new(),
            base_dependency: None,
            attempts: Vec::new(),
            questions: Vec::new(),
            validations: Vec::new(),
            submission: None,
            artifacts: Vec::new(),
            links: Vec::new(),
            branch_head: None,
            merge_refused: None,
            conflict_base: None,
            failure: None,
            redirect_text: None,
            redirect_delivered: false,
            acknowledged_at: None,
            rework_of: None,
            hold_pr: false,
            release_pending: Vec::new(),
            release_held: BTreeMap::new(),
            turn_deferral: None,
            retry: None,
            created_at: Timestamp::from_millis(0),
            updated_at: Timestamp::from_millis(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_concurrent_tasks: usize,
    pub max_attempts: u32,
    pub retry_backoff: Duration,
    pub coordinator_context_tokens: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_concurrent_tasks: 4,
            max_attempts: 3,
            retry_backoff: Duration::from_secs(30),
            coordinator_context_tokens: 120_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectState {
    pub project: ProjectId,
    pub slug: String,
    pub tasks: BTreeMap<TaskId, Task>,
    pub coordinator: Option<CoordinatorSession>,
    pub profiles: BTreeMap<Role, ProfileId>,
    pub fallback_profiles: Vec<ProfileId>,
    pub limits: Limits,
    pub always_relay_questions: bool,
    pub merge_policy: MergePolicy,
}

impl ProjectState {
    pub fn active_task_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|task| task.state.in_flight())
            .count()
    }
}
