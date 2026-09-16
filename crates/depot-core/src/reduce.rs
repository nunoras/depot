use std::collections::BTreeMap;
use std::time::Duration;

use crate::action::{Action, Baseline};
use crate::fact::{Fact, FactKind, Liveness};
use crate::model::{
    Answer, Attempt, AttemptOutcome, Checks, CommitId, Dependency, Limits, Link, ProfileId,
    ProjectState, Question, Retry, Task, TaskId, TaskState, Timestamp, ValidationRecord,
};

pub fn reduce(state: &ProjectState, fact: &Fact) -> (ProjectState, Vec<Action>) {
    let mut next = state.clone();
    let mut actions = Vec::new();
    let mut changed = false;
    let mut approved: Vec<TaskId> = Vec::new();

    match &fact.kind {
        FactKind::TaskProposed {
            task,
            title,
            intent,
            role,
            dependencies,
            base_dependency,
        } => {
            if !next.tasks.contains_key(task)
                && base_dependency_is_valid(dependencies, base_dependency)
            {
                let project = next.project.clone();
                next.tasks.insert(
                    task.clone(),
                    Task {
                        id: task.clone(),
                        project,
                        title: title.clone(),
                        intent: intent.clone(),
                        role: *role,
                        state: TaskState::Proposed,
                        dependencies: dependencies.clone(),
                        base_dependency: base_dependency.clone(),
                        attempts: Vec::new(),
                        questions: Vec::new(),
                        validations: Vec::new(),
                        artifacts: Vec::new(),
                        links: Vec::new(),
                        branch_head: None,
                        retry: None,
                        created_at: fact.at,
                        updated_at: fact.at,
                    },
                );
                changed = true;
            }
        }

        FactKind::TaskApproved { task } => {
            if let Some(task) = next.tasks.get_mut(task) {
                if task.state == TaskState::Proposed {
                    task.state = TaskState::Approved;
                    task.updated_at = fact.at;
                    changed = true;
                }
                if task.state == TaskState::Approved {
                    approved.push(task.id.clone());
                }
            }
        }

        FactKind::TaskCancelled { task } => {
            if let Some(task) = next.tasks.get_mut(task)
                && !matches!(task.state, TaskState::Cancelled | TaskState::Landed)
            {
                let stopped = close_attempt(task, AttemptOutcome::Stopped, fact.at);
                task.state = TaskState::Cancelled;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                if stopped {
                    actions.push(Action::StopSession {
                        task: task.id.clone(),
                    });
                }
            }
        }

        FactKind::QuestionAsked { task, text, relay } => {
            let relay = *relay || next.always_relay_questions;
            if let Some(task) = next.tasks.get_mut(task) {
                let id = task.id.clone();
                task.questions.push(Question {
                    text: text.clone(),
                    asked_at: fact.at,
                    answer: None,
                });
                task.updated_at = fact.at;
                if relay {
                    task.state = TaskState::WaitingOnQuestion;
                    changed = true;
                    actions.push(Action::Notify { task: id.clone() });
                    actions.push(Action::HoldForUser { task: id });
                }
            }
        }

        FactKind::QuestionAnswered { task, answer, by } => {
            if let Some(task) = next.tasks.get_mut(task) {
                if let Some(question) = task.unanswered_question() {
                    question.answer = Some(Answer {
                        text: answer.clone(),
                        by: *by,
                        at: fact.at,
                    });
                }
                task.updated_at = fact.at;
                if task.state == TaskState::WaitingOnQuestion {
                    task.state = TaskState::Running;
                    changed = true;
                    actions.push(Action::ResumeSession {
                        task: task.id.clone(),
                    });
                }
            }
        }

        FactKind::WorkerTurnStarted { task, session } => {
            if let Some(task) = next.tasks.get_mut(task) {
                if let Some(attempt) = task.attempts.last_mut() {
                    attempt.session = Some(session.clone());
                }
                task.updated_at = fact.at;
            }
        }

        FactKind::WorkerTurnEnded { task } => {
            if let Some(task) = next.tasks.get_mut(task) {
                task.updated_at = fact.at;
            }
        }

        FactKind::WorkerLivenessChanged { task, liveness } => {
            let open = next.tasks.get(task).is_some_and(|task| {
                task.attempts
                    .last()
                    .is_some_and(|attempt| is_open(attempt.outcome))
            });
            if open && *liveness == Liveness::Gone {
                if let Some(task) = next.tasks.get_mut(task) {
                    if let Some(attempt) = task.attempts.last_mut() {
                        attempt.outcome = AttemptOutcome::Failed;
                        attempt.finished_at = Some(fact.at);
                    }
                    task.state = TaskState::Failed;
                    task.updated_at = fact.at;
                }
                changed = true;
                actions.push(Action::HoldForUser { task: task.clone() });
            } else if *liveness == Liveness::Live
                && let Some(task) = next.tasks.get_mut(task)
            {
                if let Some(attempt) = task.attempts.last_mut()
                    && attempt.outcome == AttemptOutcome::Unknown
                {
                    attempt.outcome = AttemptOutcome::InFlight;
                }
                task.updated_at = fact.at;
            }
        }

        FactKind::WorkerSubmitted { task, commit } => {
            let can_submit = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state.in_flight());
            if can_submit {
                if let Some(task) = next.tasks.get_mut(task) {
                    close_attempt(task, AttemptOutcome::Submitted, fact.at);
                    task.state = TaskState::Validating;
                    task.updated_at = fact.at;
                }
                changed = true;
                actions.push(Action::RunValidation {
                    task: task.clone(),
                    commit: commit.clone(),
                });
            }
        }

        FactKind::ValidationStarted { task, .. } => {
            if let Some(task) = next.tasks.get_mut(task) {
                task.updated_at = fact.at;
            }
        }

        FactKind::ValidationFinished {
            task,
            command,
            commit,
            exit_code,
            duration,
            output_tail,
        } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state == TaskState::Validating);
            if accepting {
                if let Some(task) = next.tasks.get_mut(task) {
                    task.validations.push(ValidationRecord {
                        command: command.clone(),
                        commit: commit.clone(),
                        exit_code: *exit_code,
                        duration: *duration,
                        output_tail: output_tail.clone(),
                    });
                    task.updated_at = fact.at;
                    if *exit_code == 0 {
                        task.state = TaskState::Validated;
                        task.retry = None;
                    } else {
                        task.state = TaskState::Failed;
                    }
                }
                changed = true;
                if *exit_code == 0 {
                    refresh_pending_dependents(&mut next, task, commit);
                    if publication_blocked(&next, task) {
                        actions.push(Action::HoldForUser { task: task.clone() });
                    } else {
                        actions.push(Action::Push {
                            task: task.clone(),
                            commit: commit.clone(),
                        });
                        actions.push(Action::OpenPullRequest {
                            task: task.clone(),
                            commit: commit.clone(),
                        });
                    }
                } else {
                    actions.push(Action::HoldForUser { task: task.clone() });
                }
            }
        }

        FactKind::WorktreeAcquired {
            task,
            lease,
            baseline,
        } => {
            repin_acquired_task(&mut next, task, baseline);
            let may_rework = next.tasks.get(task).is_some_and(|task| {
                task.state == TaskState::Validated
                    && task
                        .attempts
                        .last()
                        .is_some_and(|attempt| !is_open(attempt.outcome))
                    && next.active_task_count() < next.limits.max_concurrent_tasks
            });
            if let Some(task) = next.tasks.get_mut(task) {
                let live = task
                    .attempts
                    .last()
                    .is_some_and(|attempt| is_open(attempt.outcome));
                if live {
                    if let Some(attempt) = task.attempts.last_mut() {
                        attempt.worktree = Some(lease.clone());
                    }
                    task.updated_at = fact.at;
                } else if may_rework
                    && let Some(profile) = task.attempts.last().map(|a| a.profile.clone())
                {
                    task.attempts.push(Attempt {
                        session: None,
                        profile: profile.clone(),
                        worktree: Some(lease.clone()),
                        started_at: fact.at,
                        finished_at: None,
                        outcome: AttemptOutcome::InFlight,
                    });
                    task.state = TaskState::Running;
                    task.updated_at = fact.at;
                    changed = true;
                    actions.push(Action::LaunchSession {
                        task: task.id.clone(),
                        profile,
                    });
                }
            }
        }

        FactKind::BranchPushed { task, commit } => {
            if let Some(task) = next.tasks.get_mut(task) {
                task.branch_head = Some(commit.clone());
                task.updated_at = fact.at;
            }
        }

        FactKind::PullRequestOpened { task, number, url } => {
            if let Some(task) = next.tasks.get_mut(task) {
                task.links.push(Link::PullRequest {
                    number: *number,
                    url: url.clone(),
                    checks: Checks::Unknown,
                });
                task.state = TaskState::PrOpen;
                task.updated_at = fact.at;
                changed = true;
            }
        }

        FactKind::PullRequestChecksChanged { task, checks } => {
            if let Some(task) = next.tasks.get_mut(task) {
                for link in task.links.iter_mut() {
                    if let Link::PullRequest {
                        checks: recorded, ..
                    } = link
                    {
                        *recorded = *checks;
                    }
                }
                task.updated_at = fact.at;
            }
        }

        FactKind::PullRequestMerged { task, .. } => {
            if let Some(task) = next.tasks.get_mut(task) {
                task.state = TaskState::Landed;
                task.updated_at = fact.at;
                changed = true;
            }
            if let Some(lease) = next
                .tasks
                .get(task)
                .and_then(|task| task.attempts.last())
                .and_then(|attempt| attempt.worktree.clone())
            {
                actions.push(Action::ReleaseWorktree {
                    task: task.clone(),
                    lease,
                });
            }
        }

        FactKind::PullRequestClosedUnmerged { task } => {
            if let Some(task) = next.tasks.get_mut(task) {
                task.state = TaskState::Cancelled;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::RunDurationExceeded { task } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state.in_flight()
            {
                let stopped = close_attempt(task, AttemptOutcome::Stopped, fact.at);
                task.state = TaskState::Failed;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                if stopped {
                    actions.push(Action::StopSession {
                        task: task.id.clone(),
                    });
                }
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::RetryExhausted { task } => {
            if let Some(task) = next.tasks.get_mut(task) {
                task.state = TaskState::Failed;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::ProviderRateLimited { task, profile } => {
            let limits = next.limits.clone();
            let fallback = fallback_profile(&next, profile);
            if let Some(task) = next.tasks.get_mut(task) {
                let attempts = task.attempts.len() as u32;
                let stopped = close_attempt(task, AttemptOutcome::Failed, fact.at);
                task.updated_at = fact.at;
                if stopped {
                    actions.push(Action::StopSession {
                        task: task.id.clone(),
                    });
                }
                if attempts >= limits.max_attempts {
                    task.state = TaskState::Failed;
                    task.retry = None;
                    changed = true;
                    actions.push(Action::HoldForUser {
                        task: task.id.clone(),
                    });
                } else {
                    let not_before = fact.at.plus(backoff_for(&limits, attempts));
                    task.state = TaskState::Approved;
                    task.retry = Some(Retry {
                        profile: fallback,
                        not_before,
                    });
                    changed = true;
                    actions.push(Action::Queue {
                        task: task.id.clone(),
                        not_before: Some(not_before),
                    });
                }
            }
        }

        FactKind::DaemonRestarted => {
            for task in next.tasks.values_mut() {
                if !task.state.in_flight() {
                    continue;
                }
                if let Some(attempt) = task.attempts.last_mut()
                    && attempt.outcome == AttemptOutcome::InFlight
                {
                    attempt.outcome = AttemptOutcome::Unknown;
                }
            }
        }

        FactKind::Polled => {}
    }

    start_ready_tasks(&mut next, fact.at, &mut actions, &mut changed);

    for task in approved {
        if next
            .tasks
            .get(&task)
            .is_some_and(|task| task.state == TaskState::Approved)
        {
            let not_before = next
                .tasks
                .get(&task)
                .and_then(|task| task.retry.as_ref())
                .map(|retry| retry.not_before);
            actions.push(Action::Queue { task, not_before });
        }
    }

    if changed {
        actions.push(Action::RenderChecklist);
    }

    (next, actions)
}

fn is_open(outcome: AttemptOutcome) -> bool {
    matches!(outcome, AttemptOutcome::InFlight | AttemptOutcome::Unknown)
}

fn close_attempt(task: &mut Task, outcome: AttemptOutcome, at: Timestamp) -> bool {
    let Some(attempt) = task.attempts.last_mut() else {
        return false;
    };
    if !is_open(attempt.outcome) {
        return false;
    }
    attempt.outcome = outcome;
    attempt.finished_at = Some(at);
    attempt.session.is_some()
}

fn start_ready_tasks(
    state: &mut ProjectState,
    at: Timestamp,
    actions: &mut Vec<Action>,
    changed: &mut bool,
) {
    while state.active_task_count() < state.limits.max_concurrent_tasks {
        let Some((id, profile, baseline)) = next_startable(state, at) else {
            break;
        };
        let Some(task) = state.tasks.get_mut(&id) else {
            break;
        };
        task.state = TaskState::Running;
        task.retry = None;
        task.updated_at = at;
        task.attempts.push(Attempt {
            session: None,
            profile: profile.clone(),
            worktree: None,
            started_at: at,
            finished_at: None,
            outcome: AttemptOutcome::InFlight,
        });
        actions.push(Action::AcquireWorktree {
            task: id.clone(),
            baseline,
        });
        actions.push(Action::LaunchSession { task: id, profile });
        *changed = true;
    }
}

fn next_startable(state: &ProjectState, at: Timestamp) -> Option<(TaskId, ProfileId, Baseline)> {
    state.tasks.values().find_map(|task| {
        if task.state != TaskState::Approved
            || !retry_due(task, at)
            || publication_blocked(state, &task.id)
        {
            return None;
        }
        let profile = task
            .retry
            .as_ref()
            .map(|retry| retry.profile.clone())
            .or_else(|| state.profiles.get(&task.role).cloned())?;
        Some((task.id.clone(), profile, worktree_baseline(task)))
    })
}

fn retry_due(task: &Task, at: Timestamp) -> bool {
    task.retry
        .as_ref()
        .is_none_or(|retry| retry.not_before <= at)
}

fn worktree_baseline(task: &Task) -> Baseline {
    let base = task.base_dependency.as_ref().or_else(|| {
        (task.dependencies.len() == 1).then(|| &task.dependencies[0].task)
    });
    base.and_then(|id| {
        task.dependencies
            .iter()
            .find(|dependency| &dependency.task == id)
            .map(|dependency| Baseline::PinnedCommit(dependency.commit.clone()))
    })
    .unwrap_or(Baseline::DefaultBranchHead)
}

fn base_dependency_is_valid(
    dependencies: &[Dependency],
    base_dependency: &Option<TaskId>,
) -> bool {
    match base_dependency {
        None => dependencies.len() <= 1,
        Some(base) => dependencies
            .iter()
            .any(|dependency| &dependency.task == base),
    }
}

fn publication_blocked(state: &ProjectState, task: &TaskId) -> bool {
    state.tasks.get(task).is_some_and(|task| {
        task.dependencies
            .iter()
            .any(|dependency| !edge_satisfied(state, dependency))
    })
}

fn edge_satisfied(state: &ProjectState, dependency: &Dependency) -> bool {
    state
        .tasks
        .get(&dependency.task)
        .and_then(|task| task.validated_commit())
        == Some(&dependency.commit)
}

fn refresh_pending_dependents(state: &mut ProjectState, prerequisite: &TaskId, commit: &CommitId) {
    for task in state.tasks.values_mut() {
        if !matches!(task.state, TaskState::Proposed | TaskState::Approved) {
            continue;
        }
        for dependency in task.dependencies.iter_mut() {
            if &dependency.task == prerequisite {
                dependency.commit = commit.clone();
            }
        }
    }
}

fn repin_acquired_task(state: &mut ProjectState, task: &TaskId, baseline: &Baseline) {
    let Baseline::PinnedCommit(commit) = baseline else {
        return;
    };
    let validated: BTreeMap<TaskId, CommitId> = state
        .tasks
        .iter()
        .filter_map(|(id, task)| task.validated_commit().map(|c| (id.clone(), c.clone())))
        .collect();
    if let Some(task) = state.tasks.get_mut(task) {
        for dependency in task.dependencies.iter_mut() {
            if validated.get(&dependency.task) == Some(commit) {
                dependency.commit = commit.clone();
            }
        }
    }
}

fn fallback_profile(state: &ProjectState, current: &ProfileId) -> ProfileId {
    match state.fallback_profiles.iter().position(|p| p == current) {
        Some(index) => state
            .fallback_profiles
            .get(index + 1)
            .cloned()
            .unwrap_or_else(|| current.clone()),
        None => state
            .fallback_profiles
            .first()
            .cloned()
            .unwrap_or_else(|| current.clone()),
    }
}

fn backoff_for(limits: &Limits, attempts: u32) -> Duration {
    let doublings = attempts.saturating_sub(1).min(16);
    limits
        .retry_backoff
        .checked_mul(2u32.saturating_pow(doublings))
        .unwrap_or(limits.retry_backoff)
}
