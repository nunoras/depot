use std::collections::BTreeMap;
use std::time::Duration;

use crate::action::{Action, Baseline};
use crate::fact::{Fact, FactKind, Liveness};
use crate::model::{
    Answer, Attempt, AttemptOutcome, Checks, CommitId, CoordinatorSession, Dependency, Limits,
    Link, MergePolicy, ProfileId, ProjectState, Question, ReleaseHold, Retry, Role, Submission,
    Task, TaskId, TaskState, Timestamp, TurnDeferral, ValidationRecord, WorktreeLease,
};

pub fn reduce(state: &ProjectState, fact: &Fact) -> (ProjectState, Vec<Action>) {
    let mut next = state.clone();
    let mut actions = Vec::new();
    let mut changed = false;
    let mut approved: Vec<TaskId> = Vec::new();

    match &fact.kind {
        FactKind::TaskDispatchJudged { .. } => {}
        FactKind::TaskProposed {
            task,
            title,
            intent,
            role,
            dispatch_profile,
            dependencies,
            base_dependency,
            hold_pr,
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
                        dispatch_profile: dispatch_profile.clone(),
                        state: TaskState::Proposed,
                        dependencies: dependencies.clone(),
                        base_dependency: base_dependency.clone(),
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
                        hold_pr: *hold_pr,
                        release_pending: Vec::new(),
                        release_held: BTreeMap::new(),
                        turn_deferral: None,
                        retry: None,
                        created_at: fact.at,
                        updated_at: fact.at,
                    },
                );
                changed = true;
            }
        }

        FactKind::TaskReleased { task } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state == TaskState::Validated && task.hold_pr);
            if accepting {
                let commit = next
                    .tasks
                    .get(task)
                    .and_then(|task| task.validated_commit().cloned());
                let id = task.clone();
                let blocked = publication_blocked(&next, task);
                if let Some(task) = next.tasks.get_mut(task) {
                    task.hold_pr = false;
                    task.updated_at = fact.at;
                    changed = true;
                    match (commit, blocked) {
                        (Some(commit), false) => {
                            actions.push(Action::Push {
                                task: id.clone(),
                                commit: commit.clone(),
                            });
                            actions.push(Action::OpenPullRequest { task: id, commit });
                        }
                        _ => {
                            actions.push(Action::HoldForUser { task: id });
                        }
                    }
                }
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
                task.merge_refused = None;
                task.conflict_base = None;
                task.updated_at = fact.at;
                changed = true;
                if stopped {
                    actions.push(Action::StopSession {
                        task: task.id.clone(),
                    });
                }
                if let Some(lease) = owe_release(task) {
                    actions.push(Action::ReleaseWorktree {
                        task: task.id.clone(),
                        lease,
                    });
                }
            }
        }

        FactKind::TaskRetried { task } => {
            let retryable = next
                .tasks
                .get(task)
                .is_some_and(|task| matches!(task.state, TaskState::Failed | TaskState::Cancelled));
            if retryable && let Some(task) = next.tasks.get_mut(task) {
                task.state = TaskState::Approved;
                task.retry = None;
                task.merge_refused = None;
                task.conflict_base = None;
                task.failure = None;
                task.acknowledged_at = None;
                task.turn_deferral = None;
                task.updated_at = fact.at;
                changed = true;
                approved.push(task.id.clone());
            }
        }

        FactKind::TaskReworked { task, fix, text } => {
            let fix_profile = next.profiles.get(&Role::Fix).cloned();
            let accepted = next.tasks.get(task).is_some_and(|original| {
                original.state == TaskState::PrOpen
                    && !next.tasks.contains_key(fix)
                    && fix_profile.is_some()
            });
            if accepted && let Some(original) = next.tasks.get_mut(task) {
                let fix_profile = fix_profile.expect("fix profile checked");
                let stopped = close_attempt(original, AttemptOutcome::Stopped, fact.at);
                original.state = TaskState::ReworkPending;
                original.updated_at = fact.at;
                let lease = take_last_worktree(original);
                let links = original.links.clone();
                let title = format!("rework of {}: {}", task.as_str(), original.title);
                let project = original.project.clone();
                next.tasks.insert(
                    fix.clone(),
                    Task {
                        id: fix.clone(),
                        project,
                        title,
                        intent: text.clone(),
                        role: Role::Fix,
                        dispatch_profile: Some(fix_profile.clone()),
                        state: TaskState::Running,
                        dependencies: Vec::new(),
                        base_dependency: None,
                        attempts: vec![Attempt {
                            session: None,
                            profile: fix_profile.clone(),
                            worktree: lease,
                            started_at: fact.at,
                            finished_at: None,
                            outcome: AttemptOutcome::InFlight,
                            base_merge: false,
                            last_seen_at: None,
                        }],
                        questions: Vec::new(),
                        validations: Vec::new(),
                        submission: None,
                        artifacts: Vec::new(),
                        links,
                        branch_head: None,
                        merge_refused: None,
                        conflict_base: None,
                        failure: None,
                        redirect_text: None,
                        redirect_delivered: false,
                        acknowledged_at: None,
                        rework_of: Some(task.clone()),
                        hold_pr: false,
                        release_pending: Vec::new(),
                        release_held: BTreeMap::new(),
                        turn_deferral: None,
                        retry: None,
                        created_at: fact.at,
                        updated_at: fact.at,
                    },
                );
                changed = true;
                if stopped {
                    actions.push(Action::StopSession { task: task.clone() });
                }
                actions.push(Action::LaunchSession {
                    task: fix.clone(),
                    profile: fix_profile,
                });
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
                changed = true;
                if relay
                    && matches!(
                        task.state,
                        TaskState::Running | TaskState::WaitingOnQuestion,
                    )
                {
                    task.state = TaskState::WaitingOnQuestion;
                    actions.push(Action::Notify { task: id.clone() });
                    actions.push(Action::HoldForUser { task: id });
                }
            }
        }

        FactKind::QuestionAnswered { task, answer, by } => {
            if let Some(task) = next.tasks.get_mut(task) {
                let answered = if let Some(question) = task.unanswered_question() {
                    question.answer = Some(Answer {
                        text: answer.clone(),
                        by: *by,
                        at: fact.at,
                    });
                    true
                } else {
                    false
                };
                task.updated_at = fact.at;
                if answered {
                    changed = true;
                }
                if answered
                    && task.state == TaskState::WaitingOnQuestion
                    && task
                        .questions
                        .iter()
                        .all(|question| question.answer.is_some())
                {
                    if task
                        .attempts
                        .last()
                        .is_some_and(|attempt| attempt.outcome.is_open())
                    {
                        task.state = TaskState::Running;
                        actions.push(Action::ResumeSession {
                            task: task.id.clone(),
                        });
                    } else {
                        relaunch(task, fact.at, &mut actions);
                    }
                }
            }
        }

        FactKind::WorktreeAcquireRequested { .. }
        | FactKind::WorkerTurnLaunchRequested { .. }
        | FactKind::WorkerTurnResumeRequested { .. } => {}

        FactKind::WorkerRelaunchRequested { task } => {
            let eligible = next.tasks.get(task).is_some_and(|task| {
                matches!(
                    task.state,
                    TaskState::Running | TaskState::WaitingOnQuestion
                ) && !task.has_unanswered_question()
                    && task
                        .attempts
                        .last()
                        .is_some_and(|attempt| attempt.session.is_some())
            });
            if eligible && let Some(task) = next.tasks.get_mut(task) {
                close_attempt(task, AttemptOutcome::AwaitingAnswer, fact.at);
                if relaunch(task, fact.at, &mut actions) {
                    changed = true;
                }
            }
        }

        FactKind::WorkerTurnUnresolved { task, reason } => {
            let unresolved = next.tasks.get(task).is_some_and(|task| {
                task.state.in_flight()
                    && task
                        .attempts
                        .last()
                        .is_some_and(|attempt| attempt.outcome.is_open())
            });
            if unresolved && let Some(task) = next.tasks.get_mut(task) {
                close_attempt(task, AttemptOutcome::Failed, fact.at);
                task.state = TaskState::Failed;
                task.failure = Some(reason.clone());
                task.turn_deferral = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::WorkerTurnStarted { task, session } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state.in_flight()
            {
                if let Some(attempt) = task.attempts.last_mut()
                    && attempt.outcome.is_open()
                {
                    attempt.session = Some(session.clone());
                }
                task.turn_deferral = None;
                task.updated_at = fact.at;
            }
        }

        FactKind::WorkerTurnEnded { task } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state.in_flight()
            {
                task.updated_at = fact.at;
            }
        }

        FactKind::WorkerRedirected { task, text } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state.in_flight()
            {
                task.redirect_text = Some(text.clone());
                task.redirect_delivered = false;
                task.updated_at = fact.at;
                changed = true;
            }
        }

        FactKind::WorkerRedirectDelivered { task, .. } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.redirect_text.is_some()
                && !task.redirect_delivered
            {
                task.redirect_delivered = true;
                task.updated_at = fact.at;
                changed = true;
            }
        }

        FactKind::WorkerLivenessChanged { task, liveness } => {
            let in_flight = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state.in_flight());
            let open = next.tasks.get(task).is_some_and(|task| {
                task.attempts
                    .last()
                    .is_some_and(|attempt| attempt.outcome.is_open())
            });
            let paused = next
                .tasks
                .get(task)
                .is_some_and(|task| task.has_unanswered_question());
            if in_flight && open && *liveness == Liveness::Gone {
                if paused {
                    if let Some(task) = next.tasks.get_mut(task) {
                        close_attempt(task, AttemptOutcome::AwaitingAnswer, fact.at);
                        task.updated_at = fact.at;
                    }
                    changed = true;
                } else if let Some(record) = next.tasks.get_mut(task) {
                    if let Some(attempt) = record.attempts.last_mut() {
                        attempt.outcome = AttemptOutcome::Failed;
                        attempt.finished_at = Some(fact.at);
                    }
                    record.state = TaskState::Failed;
                    record.updated_at = fact.at;
                    changed = true;
                    actions.push(Action::HoldForUser { task: task.clone() });
                }
            } else if in_flight
                && *liveness == Liveness::Live
                && let Some(task) = next.tasks.get_mut(task)
            {
                if let Some(attempt) = task.attempts.last_mut()
                    && attempt.outcome.is_open()
                {
                    if attempt.outcome == AttemptOutcome::Unknown {
                        attempt.outcome = AttemptOutcome::InFlight;
                    }
                    attempt.last_seen_at = Some(fact.at);
                    changed = true;
                }
                if task.turn_deferral.take().is_some() {
                    changed = true;
                }
                task.updated_at = fact.at;
            }
        }

        FactKind::WorkerSessionFailed { task, reason } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state.in_flight());
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                close_attempt(task, AttemptOutcome::Failed, fact.at);
                task.state = TaskState::Failed;
                task.retry = None;
                task.failure = Some(reason.clone());
                task.turn_deferral = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::WorkerSubmissionRecorded {
            task,
            summary,
            artifacts,
        } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state == TaskState::Running
            {
                task.submission = Some(Submission {
                    summary: summary.clone(),
                    artifacts: artifacts.clone(),
                });
                task.updated_at = fact.at;
                changed = true;
            }
        }

        FactKind::WorkerSubmitted { task, commit, .. } => {
            let can_submit = next.tasks.get(task).is_some_and(|task| {
                task.state == TaskState::Running
                    && task
                        .attempts
                        .last()
                        .is_some_and(|attempt| attempt.outcome.is_open())
            });
            if can_submit {
                if let Some(task) = next.tasks.get_mut(task) {
                    close_attempt(task, AttemptOutcome::Submitted, fact.at);
                    task.state = TaskState::Validating;
                    task.turn_deferral = None;
                    task.updated_at = fact.at;
                }
                changed = true;
                actions.push(Action::RunValidation {
                    task: task.clone(),
                    commit: commit.clone(),
                });
            }
        }

        FactKind::WorkerCommittedNothing { task, reason, .. } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state == TaskState::Validating);
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                task.state = TaskState::Failed;
                task.retry = None;
                task.failure = Some(reason.clone());
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::ValidationStarted { task, .. } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state == TaskState::Validating
            {
                task.updated_at = fact.at;
            }
        }

        FactKind::ValidationFinished {
            task,
            command,
            commit,
            base_commit,
            exit_code,
            duration,
            output_tail,
        } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state == TaskState::Validating);
            if accepting {
                let already_open = next
                    .tasks
                    .get(task)
                    .is_some_and(|task| task.pull_request().is_some());
                if let Some(task) = next.tasks.get_mut(task) {
                    task.validations.push(ValidationRecord {
                        command: command.clone(),
                        commit: commit.clone(),
                        base_commit: base_commit.clone(),
                        exit_code: *exit_code,
                        duration: *duration,
                        output_tail: output_tail.clone(),
                    });
                    task.turn_deferral = None;
                    task.updated_at = fact.at;
                    if *exit_code == 0 {
                        task.state = if already_open {
                            TaskState::PrOpen
                        } else {
                            TaskState::Validated
                        };
                        task.retry = None;
                    } else {
                        task.state = TaskState::Failed;
                    }
                }
                changed = true;
                if *exit_code == 0 {
                    refresh_pending_dependents(&mut next, task, commit);
                    let held = next.tasks.get(task).is_some_and(|task| task.hold_pr);
                    if publication_blocked(&next, task) {
                        actions.push(Action::HoldForUser { task: task.clone() });
                    } else {
                        actions.push(Action::Push {
                            task: task.clone(),
                            commit: commit.clone(),
                        });
                        if !already_open {
                            if held {
                                actions.push(Action::HoldForUser { task: task.clone() });
                            } else {
                                actions.push(Action::OpenPullRequest {
                                    task: task.clone(),
                                    commit: commit.clone(),
                                });
                            }
                        }
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
            included,
        } => {
            if let Some(task) = next.tasks.get_mut(task)
                && let Some(index) = task.release_pending.iter().position(|owed| owed == lease)
            {
                task.release_pending.remove(index);
                task.release_held.remove(lease);
                task.updated_at = fact.at;
                changed = true;
            }
            let live = next.tasks.get(task).is_some_and(|task| {
                task.attempts
                    .last()
                    .is_some_and(|attempt| attempt.outcome.is_open())
            });
            let rework_candidate = next.tasks.get(task).is_some_and(|candidate| {
                let closed = candidate
                    .attempts
                    .last()
                    .is_some_and(|attempt| !attempt.outcome.is_open());
                let under_cap = next.active_task_count() < next.limits.max_concurrent_tasks;
                if !closed || !under_cap {
                    return false;
                }
                match candidate.state {
                    TaskState::Validated => true,
                    TaskState::PrOpen => publication_blocked(&next, task),
                    _ => false,
                }
            });
            let mut attached = false;
            if live {
                if let Some(task) = next.tasks.get_mut(task) {
                    if let Some(attempt) = task.attempts.last_mut() {
                        if let Some(prior) = attempt.worktree.take()
                            && &prior != lease
                        {
                            actions.push(Action::ReleaseWorktree {
                                task: task.id.clone(),
                                lease: prior,
                            });
                        }
                        attempt.worktree = Some(lease.clone());
                    }
                    if task.turn_deferral.take().is_some() {
                        changed = true;
                    }
                    task.updated_at = fact.at;
                    attached = true;
                }
            } else if rework_candidate {
                let previous_pins = dependency_pins(&next, task);
                apply_included_pins(&mut next, task, included);
                repin_acquired_task(&mut next, task, baseline);
                if publication_blocked(&next, task) {
                    restore_dependency_pins(&mut next, task, &previous_pins);
                } else if let Some(task) = next.tasks.get_mut(task)
                    && let Some(profile) = task.attempts.last().map(|a| a.profile.clone())
                {
                    if let Some(prior) = take_last_worktree(task)
                        && &prior != lease
                    {
                        actions.push(Action::ReleaseWorktree {
                            task: task.id.clone(),
                            lease: prior,
                        });
                    }
                    task.attempts.push(Attempt {
                        last_seen_at: None,
                        session: None,
                        profile: profile.clone(),
                        worktree: Some(lease.clone()),
                        started_at: fact.at,
                        finished_at: None,
                        outcome: AttemptOutcome::InFlight,
                        base_merge: false,
                    });
                    task.state = TaskState::Running;
                    task.updated_at = fact.at;
                    changed = true;
                    attached = true;
                    actions.push(Action::LaunchSession {
                        task: task.id.clone(),
                        profile,
                    });
                }
            }
            if !attached {
                let held = next.tasks.get(task).is_some_and(|task| {
                    task.attempts
                        .iter()
                        .any(|attempt| attempt.worktree.as_ref() == Some(lease))
                });
                if !held {
                    actions.push(Action::ReleaseWorktree {
                        task: task.clone(),
                        lease: lease.clone(),
                    });
                }
            }
        }

        FactKind::WorktreeBaselined { .. } => {}

        FactKind::BranchPushed { task, commit } => {
            if let Some(task) = next.tasks.get_mut(task)
                && !matches!(task.state, TaskState::Landed | TaskState::Cancelled)
            {
                if task.branch_head.as_ref() != Some(commit) {
                    task.branch_head = Some(commit.clone());
                    changed = true;
                }
                task.updated_at = fact.at;
            }
        }

        FactKind::PullRequestOpened { task, number, url } => {
            let state = next.tasks.get(task).map(|task| task.state);
            match state {
                Some(TaskState::Validating) => {
                    if let Some(task) = next.tasks.get_mut(task) {
                        if link_pull_request(task, *number, url) {
                            changed = true;
                        }
                        task.updated_at = fact.at;
                    }
                }
                Some(TaskState::Validated) => {
                    if let Some(task) = next.tasks.get_mut(task) {
                        let stopped = close_attempt(task, AttemptOutcome::Submitted, fact.at);
                        link_pull_request(task, *number, url);
                        task.state = TaskState::PrOpen;
                        task.updated_at = fact.at;
                        changed = true;
                        if stopped {
                            actions.push(Action::StopSession {
                                task: task.id.clone(),
                            });
                        }
                    }
                }
                Some(TaskState::PrOpen) => {
                    if let Some(task) = next.tasks.get_mut(task) {
                        if link_pull_request(task, *number, url) {
                            changed = true;
                        }
                        task.updated_at = fact.at;
                    }
                }
                _ => {}
            }
        }

        FactKind::PullRequestChecksChanged { task, checks } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state.tracks_pull_request()
            {
                let mut updated = false;
                for link in task.links.iter_mut() {
                    if let Link::PullRequest {
                        checks: recorded, ..
                    } = link
                        && *recorded != *checks
                    {
                        *recorded = *checks;
                        updated = true;
                    }
                }
                if updated {
                    changed = true;
                }
                task.updated_at = fact.at;
            }
        }

        FactKind::PullRequestMerged { task, commit }
        | FactKind::StaleMergeObserved { task, commit } => {
            let tracked = next.tasks.get(task).is_some_and(Task::may_still_land);
            if let Some(task) = next.tasks.get_mut(task) {
                let cleared_refusal = task.merge_refused.take().is_some();
                let cleared_conflict = task.conflict_base.take().is_some();
                if cleared_refusal || cleared_conflict {
                    changed = true;
                }
            }
            if tracked {
                settle_merged_rework_family(
                    &mut next,
                    task,
                    commit,
                    fact.at,
                    &mut actions,
                    &mut changed,
                );
            }
        }

        FactKind::PullRequestClosedUnmerged {
            task,
            number,
            commit,
        } => {
            let tracked = next.tasks.get(task).is_some_and(|record| {
                !matches!(record.state, TaskState::Cancelled | TaskState::Landed)
                    && record.pull_request().map(|(number, _, _)| number) == Some(*number)
            });
            if tracked {
                if close_superseded_in_family(&next, task, *number, commit) {
                    changed |= supersede_stale_close(
                        &mut next,
                        task,
                        *number,
                        commit,
                        fact.at,
                        &mut actions,
                    );
                } else {
                    cancel_rework_family(&mut next, task, fact.at, &mut actions, &mut changed);
                }
            }
        }

        FactKind::TaskAcknowledged { task } => {
            if let Some(task) = next.tasks.get_mut(task)
                && matches!(task.state, TaskState::Failed | TaskState::Cancelled)
                && task.acknowledged_at.is_none()
            {
                task.acknowledged_at = Some(fact.at);
                task.updated_at = fact.at;
                changed = true;
                if let Some(lease) = owe_release(task) {
                    actions.push(Action::ReleaseWorktree {
                        task: task.id.clone(),
                        lease,
                    });
                }
            }
        }

        FactKind::WorktreeReleased { task, lease } => {
            if let Some(task) = next.tasks.get_mut(task) {
                let owed = task.release_pending.iter().position(|owed| owed == lease);
                let held = task.release_held.remove(lease).is_some();
                if let Some(index) = owed {
                    task.release_pending.remove(index);
                }
                if owed.is_some() || held {
                    task.updated_at = fact.at;
                    changed = true;
                }
            }
        }

        FactKind::WorktreeReleaseHeld {
            task,
            lease,
            reason,
        } => {
            if let Some(task) = next.tasks.get_mut(task) {
                let unchanged = task
                    .release_held
                    .get(lease)
                    .is_some_and(|held| held.reason == *reason);
                if !unchanged {
                    if !task.release_pending.contains(lease) {
                        task.release_pending.push(lease.clone());
                    }
                    task.release_held.insert(
                        lease.clone(),
                        ReleaseHold {
                            reason: reason.clone(),
                            at: fact.at,
                        },
                    );
                    task.updated_at = fact.at;
                    changed = true;
                }
            }
        }

        FactKind::ValidationFailed { task, .. } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state == TaskState::Validating);
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                task.state = TaskState::Failed;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::DescribeFailed { task, .. } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| task.state == TaskState::Validated);
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                task.state = TaskState::Failed;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::PushFailed { task, .. } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| matches!(task.state, TaskState::Validated | TaskState::PrOpen));
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                close_attempt(task, AttemptOutcome::Failed, fact.at);
                task.state = TaskState::Failed;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::DeliveryFailed { task, .. } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| matches!(task.state, TaskState::Validated | TaskState::PrOpen));
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                close_attempt(task, AttemptOutcome::Failed, fact.at);
                task.state = TaskState::Failed;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::TaskLandedOnBase { task, commit } => {
            let accepting = next.tasks.get(task).is_some_and(|task| {
                task.state == TaskState::Validated && task.validated_commit() == Some(commit)
            });
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                let stopped = close_attempt(task, AttemptOutcome::Submitted, fact.at);
                task.state = TaskState::Landed;
                task.updated_at = fact.at;
                changed = true;
                if stopped {
                    actions.push(Action::StopSession {
                        task: task.id.clone(),
                    });
                }
                let fix_id = task.id.clone();
                if let Some(lease) = owe_release(task) {
                    actions.push(Action::ReleaseWorktree {
                        task: task.id.clone(),
                        lease,
                    });
                }
                close_rework_originals(&mut next, &fix_id, TaskState::Landed, fact.at);
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
            let accepting = next.tasks.get(task).is_some_and(|task| {
                task.state.in_flight()
                    || (task.state == TaskState::Approved && task.retry.is_some())
            });
            if accepting && let Some(task) = next.tasks.get_mut(task) {
                let stopped = if task.state.in_flight() {
                    close_attempt(task, AttemptOutcome::Failed, fact.at)
                } else {
                    false
                };
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

        FactKind::ProviderRateLimited { task, profile } => {
            let accepting = next.tasks.get(task).is_some_and(|task| {
                task.state.in_flight()
                    && task
                        .attempts
                        .last()
                        .is_some_and(|attempt| attempt.outcome.is_open())
            });
            if accepting {
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
                        if let Some(lease) = take_last_worktree(task) {
                            actions.push(Action::ReleaseWorktree {
                                task: task.id.clone(),
                                lease,
                            });
                        }
                        let not_before = fact.at.plus(backoff_for(&limits, attempts));
                        task.state = TaskState::Approved;
                        task.failure = None;
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
        }

        FactKind::CoordinatorSessionStarted { session } => {
            next.coordinator = Some(CoordinatorSession {
                session: session.clone(),
                started_at: fact.at,
                context_tokens: 0,
            });
        }

        FactKind::CoordinatorContextMeasured { tokens } => {
            let limit = next.limits.coordinator_context_tokens;
            if let Some(coordinator) = next.coordinator.clone() {
                if limit > 0 && *tokens >= limit {
                    next.coordinator = None;
                    actions.push(Action::RotateCoordinator {
                        session: coordinator.session,
                    });
                } else if coordinator.context_tokens != *tokens {
                    next.coordinator = Some(CoordinatorSession {
                        context_tokens: *tokens,
                        ..coordinator
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
                    changed = true;
                }
            }
        }

        FactKind::PullRequestMergeRefused { task, reason, .. } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.merge_refused.as_deref() != Some(reason.as_str())
            {
                task.merge_refused = Some(reason.clone());
                task.updated_at = fact.at;
                changed = true;
            }
        }

        FactKind::PullRequestMergeabilityChanged {
            task,
            mergeable,
            base,
        } => {
            let conflict_base = (!mergeable).then(|| base.clone());
            if let Some(task) = next.tasks.get_mut(task)
                && task.conflict_base != conflict_base
            {
                task.conflict_base = conflict_base;
                task.updated_at = fact.at;
                changed = true;
            }
        }

        FactKind::EvidencePosted { .. } => {}

        FactKind::EvidenceFailed { task, required, .. } => {
            let accepting = next
                .tasks
                .get(task)
                .is_some_and(|task| matches!(task.state, TaskState::PrOpen | TaskState::Validated));
            if *required
                && accepting
                && let Some(task) = next.tasks.get_mut(task)
            {
                close_attempt(task, AttemptOutcome::Failed, fact.at);
                task.state = TaskState::Failed;
                task.retry = None;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::HoldForUser {
                    task: task.id.clone(),
                });
            }
        }

        FactKind::RebaseScheduled { task, profile, .. } => {
            let scheduled = next
                .profiles
                .get(&Role::Fix)
                .is_some_and(|configured| configured == profile)
                && next
                    .tasks
                    .get(task)
                    .is_some_and(|task| base_merge_allowed(&next, task));
            if scheduled && let Some(task) = next.tasks.get_mut(task) {
                let lease = take_last_worktree(task);
                task.failure = None;
                task.attempts.push(Attempt {
                    last_seen_at: None,
                    session: None,
                    profile: profile.clone(),
                    worktree: lease,
                    started_at: fact.at,
                    finished_at: None,
                    outcome: AttemptOutcome::InFlight,
                    base_merge: true,
                });
                task.state = TaskState::Running;
                task.updated_at = fact.at;
                changed = true;
                actions.push(Action::LaunchSession {
                    task: task.id.clone(),
                    profile: profile.clone(),
                });
            }
        }

        FactKind::OnEventNotified { .. } => {}
        FactKind::WorkerTurnDeferred { task, reason } => {
            if let Some(task) = next.tasks.get_mut(task)
                && task.state.in_flight()
            {
                let count = task
                    .turn_deferral
                    .as_ref()
                    .map_or(0, |deferral| deferral.count)
                    + 1;
                task.turn_deferral = Some(TurnDeferral {
                    count,
                    reason: reason.clone(),
                });
                task.updated_at = fact.at;
                changed = true;
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

fn relaunch(task: &mut Task, at: Timestamp, actions: &mut Vec<Action>) -> bool {
    let Some(profile) = task.attempts.last().map(|attempt| attempt.profile.clone()) else {
        return false;
    };
    let lease = take_last_worktree(task);
    task.failure = None;
    task.attempts.push(Attempt {
        session: None,
        profile: profile.clone(),
        worktree: lease,
        started_at: at,
        finished_at: None,
        outcome: AttemptOutcome::InFlight,
        base_merge: false,
        last_seen_at: None,
    });
    task.state = TaskState::Running;
    task.updated_at = at;
    actions.push(Action::LaunchSession {
        task: task.id.clone(),
        profile,
    });
    true
}

fn settle_merged_rework_family(
    state: &mut ProjectState,
    task: &TaskId,
    commit: &CommitId,
    at: Timestamp,
    actions: &mut Vec<Action>,
    changed: &mut bool,
) {
    let family = rework_family(state, task);
    let owners: Vec<TaskId> = family
        .iter()
        .filter(|id| state.tasks.get(id).and_then(Task::validated_commit) == Some(commit))
        .cloned()
        .collect();
    if publication_blocked(state, task) {
        *changed = true;
        actions.push(Action::HoldForUser { task: task.clone() });
        return;
    }
    for id in family {
        let Some(member) = state.tasks.get(&id) else {
            continue;
        };
        if matches!(member.state, TaskState::Landed | TaskState::Cancelled) {
            continue;
        }
        let lands = owners.is_empty()
            || owners.contains(&id)
            || owners
                .iter()
                .any(|owner| rework_ancestor_of(state, &id, owner));
        let Some(member) = state.tasks.get_mut(&id) else {
            continue;
        };
        if lands {
            let stopped = close_attempt(member, AttemptOutcome::Submitted, at);
            member.state = TaskState::Landed;
            member.updated_at = at;
            if stopped {
                actions.push(Action::StopSession { task: id.clone() });
            }
        } else {
            let stopped = close_attempt(member, AttemptOutcome::Stopped, at);
            member.state = TaskState::Cancelled;
            member.retry = None;
            member.merge_refused = None;
            member.conflict_base = None;
            member.updated_at = at;
            if stopped {
                actions.push(Action::StopSession { task: id.clone() });
            }
        }
        if let Some(lease) = owe_release(member) {
            actions.push(Action::ReleaseWorktree {
                task: id.clone(),
                lease,
            });
        }
        *changed = true;
    }
}

fn cancel_rework_family(
    state: &mut ProjectState,
    task: &TaskId,
    at: Timestamp,
    actions: &mut Vec<Action>,
    changed: &mut bool,
) {
    for id in rework_family(state, task) {
        let Some(member) = state.tasks.get(&id) else {
            continue;
        };
        if matches!(member.state, TaskState::Landed | TaskState::Cancelled) {
            continue;
        }
        let Some(member) = state.tasks.get_mut(&id) else {
            continue;
        };
        let stopped = close_attempt(member, AttemptOutcome::Stopped, at);
        member.state = TaskState::Cancelled;
        member.retry = None;
        member.merge_refused = None;
        member.conflict_base = None;
        member.updated_at = at;
        if stopped {
            actions.push(Action::StopSession { task: id.clone() });
        }
        if let Some(lease) = owe_release(member) {
            actions.push(Action::ReleaseWorktree {
                task: id.clone(),
                lease,
            });
        }
        *changed = true;
    }
    actions.push(Action::HoldForUser { task: task.clone() });
}

fn rework_family(state: &ProjectState, task: &TaskId) -> Vec<TaskId> {
    let mut family = vec![rework_root(state, task)];
    let mut index = 0;
    while index < family.len() {
        let current = family[index].clone();
        index += 1;
        for (id, candidate) in &state.tasks {
            if candidate.rework_of.as_ref() == Some(&current) {
                family.push(id.clone());
            }
        }
    }
    family
}

fn rework_root(state: &ProjectState, task: &TaskId) -> TaskId {
    let mut current = task.clone();
    while let Some(parent) = state
        .tasks
        .get(&current)
        .and_then(|candidate| candidate.rework_of.clone())
    {
        current = parent;
    }
    current
}

fn rework_ancestor_of(state: &ProjectState, candidate: &TaskId, owner: &TaskId) -> bool {
    let mut current = owner.clone();
    while let Some(parent) = state
        .tasks
        .get(&current)
        .and_then(|task| task.rework_of.clone())
    {
        if &parent == candidate {
            return true;
        }
        current = parent;
    }
    false
}

fn close_superseded_in_family(
    state: &ProjectState,
    task: &TaskId,
    number: u64,
    commit: &CommitId,
) -> bool {
    rework_family(state, task).iter().any(|id| {
        state.tasks.get(id).is_some_and(|member| {
            !matches!(member.state, TaskState::Cancelled | TaskState::Landed)
                && (member
                    .pull_request()
                    .is_some_and(|(recorded, _, _)| recorded != number)
                    || member
                        .branch_head
                        .as_ref()
                        .is_some_and(|head| head != commit))
        })
    })
}

fn supersede_stale_close(
    state: &mut ProjectState,
    task: &TaskId,
    number: u64,
    commit: &CommitId,
    at: Timestamp,
    actions: &mut Vec<Action>,
) -> bool {
    let family = rework_family(state, task);
    let mut changed = false;
    for id in family {
        let Some(member) = state.tasks.get(&id) else {
            continue;
        };
        if matches!(member.state, TaskState::Cancelled | TaskState::Landed)
            || member.pull_request().map(|(recorded, _, _)| recorded) != Some(number)
        {
            continue;
        }
        let moved = member
            .branch_head
            .as_ref()
            .is_some_and(|head| head != commit);
        let Some(member) = state.tasks.get_mut(&id) else {
            continue;
        };
        member
            .links
            .retain(|link| !matches!(link, Link::PullRequest { .. }));
        member.updated_at = at;
        changed = true;
        if moved {
            changed |= republish_after_stale_close(member, at, actions);
        }
    }
    changed
}

fn republish_after_stale_close(task: &mut Task, at: Timestamp, actions: &mut Vec<Action>) -> bool {
    task.links
        .retain(|link| !matches!(link, Link::PullRequest { .. }));
    task.state = TaskState::Validated;
    task.merge_refused = None;
    task.conflict_base = None;
    task.updated_at = at;
    let Some(commit) = task.validated_commit().cloned() else {
        return true;
    };
    if task.hold_pr {
        actions.push(Action::HoldForUser {
            task: task.id.clone(),
        });
        return true;
    }
    if task.push_owed().is_some() {
        actions.push(Action::Push {
            task: task.id.clone(),
            commit: commit.clone(),
        });
    }
    actions.push(Action::OpenPullRequest {
        task: task.id.clone(),
        commit,
    });
    true
}

fn link_pull_request(task: &mut Task, number: u64, url: &str) -> bool {
    let index = task
        .links
        .iter()
        .position(|link| matches!(link, Link::PullRequest { .. }));
    match index {
        Some(index) => {
            let Link::PullRequest {
                number: recorded,
                url: recorded_url,
                checks,
            } = &mut task.links[index]
            else {
                return false;
            };
            if *recorded == number {
                return false;
            }
            *recorded = number;
            *recorded_url = url.to_owned();
            *checks = Checks::Unknown;
            true
        }
        None => {
            task.links.push(Link::PullRequest {
                number,
                url: url.to_owned(),
                checks: Checks::Unknown,
            });
            true
        }
    }
}

fn close_rework_originals(state: &mut ProjectState, fix: &TaskId, end: TaskState, at: Timestamp) {
    let original = state.tasks.get(fix).and_then(|fix| fix.rework_of.clone());
    let Some(original) = original else {
        return;
    };
    if let Some(task) = state.tasks.get_mut(&original)
        && task.state == TaskState::ReworkPending
    {
        task.state = end;
        task.updated_at = at;
    }
}

fn close_attempt(task: &mut Task, outcome: AttemptOutcome, at: Timestamp) -> bool {
    let Some(attempt) = task.attempts.last_mut() else {
        return false;
    };
    if !attempt.outcome.is_open() {
        return false;
    }
    attempt.outcome = outcome;
    attempt.finished_at = Some(at);
    true
}

fn take_last_worktree(task: &mut Task) -> Option<WorktreeLease> {
    task.attempts
        .last_mut()
        .and_then(|attempt| attempt.worktree.take())
}

fn owe_release(task: &mut Task) -> Option<WorktreeLease> {
    let lease = take_last_worktree(task)?;
    if !task.release_pending.contains(&lease) {
        task.release_pending.push(lease.clone());
    }
    Some(lease)
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
            last_seen_at: None,
            session: None,
            profile: profile.clone(),
            worktree: None,
            started_at: at,
            finished_at: None,
            outcome: AttemptOutcome::InFlight,
            base_merge: false,
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
            .or_else(|| task.dispatch_profile.clone())
            .or_else(|| state.profiles.get(&task.role).cloned())?;
        Some((task.id.clone(), profile, worktree_baseline(task)))
    })
}

fn retry_due(task: &Task, at: Timestamp) -> bool {
    task.retry
        .as_ref()
        .is_none_or(|retry| retry.not_before <= at)
}

pub fn worktree_baseline(task: &Task) -> Baseline {
    let base = task
        .base_dependency
        .as_ref()
        .or_else(|| (task.dependencies.len() == 1).then(|| &task.dependencies[0].task));
    base.and_then(|id| {
        task.dependencies
            .iter()
            .find(|dependency| &dependency.task == id)
            .map(|dependency| Baseline::PinnedCommit(dependency.commit.clone()))
    })
    .unwrap_or(Baseline::DefaultBranchHead)
}

fn base_dependency_is_valid(dependencies: &[Dependency], base_dependency: &Option<TaskId>) -> bool {
    match base_dependency {
        None => dependencies.len() <= 1,
        Some(base) => dependencies
            .iter()
            .any(|dependency| &dependency.task == base),
    }
}

pub fn auto_merge_due(state: &ProjectState, task: &Task, head: &CommitId) -> bool {
    match state.merge_policy {
        MergePolicy::Manual => false,
        MergePolicy::AfterChecks | MergePolicy::AfterReview => {
            task.state == TaskState::PrOpen
                && !publication_blocked(state, &task.id)
                && task
                    .pull_request()
                    .is_some_and(|(_, _, checks)| checks == Checks::Passing)
                && task.validated_commit() == Some(head)
                && (state.merge_policy == MergePolicy::AfterChecks
                    || review_landed(state, task, head))
        }
    }
}

fn review_landed(state: &ProjectState, task: &Task, head: &CommitId) -> bool {
    state.tasks.values().any(|other| {
        other.role == Role::Review
            && matches!(
                other.state,
                TaskState::Validated | TaskState::PrOpen | TaskState::Landed
            )
            && other
                .dependencies
                .iter()
                .any(|dependency| dependency.task == task.id && dependency.commit == *head)
    })
}

pub fn base_merge_due(state: &ProjectState, task: &Task, conflicting: bool) -> Option<ProfileId> {
    if !conflicting || !base_merge_allowed(state, task) {
        return None;
    }
    state.profiles.get(&Role::Fix).cloned()
}

fn base_merge_allowed(state: &ProjectState, task: &Task) -> bool {
    task.state == TaskState::PrOpen
        && task
            .attempts
            .last()
            .is_some_and(|attempt| !attempt.outcome.is_open())
        && (task.attempts.len() as u32) < state.limits.max_attempts
        && !state.tasks.values().any(|other| {
            other
                .attempts
                .iter()
                .any(|a| a.base_merge && a.outcome.is_open())
        })
}

pub fn publication_blocked(state: &ProjectState, task: &TaskId) -> bool {
    state.tasks.get(task).is_some_and(|task| {
        task.dependencies
            .iter()
            .any(|dependency| !dependency_satisfied(state, dependency))
    })
}

pub fn dependency_satisfied(state: &ProjectState, dependency: &Dependency) -> bool {
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
        let base = task
            .base_dependency
            .clone()
            .or_else(|| (task.dependencies.len() == 1).then(|| task.dependencies[0].task.clone()));
        for dependency in task.dependencies.iter_mut() {
            let is_base = base.as_ref() == Some(&dependency.task);
            if is_base && validated.get(&dependency.task) == Some(commit) {
                dependency.commit = commit.clone();
            }
        }
    }
}

fn apply_included_pins(state: &mut ProjectState, task: &TaskId, included: &[Dependency]) {
    if included.is_empty() {
        return;
    }
    if let Some(task) = state.tasks.get_mut(task) {
        for dependency in task.dependencies.iter_mut() {
            if let Some(pin) = included
                .iter()
                .find(|included| included.task == dependency.task)
            {
                dependency.commit = pin.commit.clone();
            }
        }
    }
}

fn dependency_pins(state: &ProjectState, task: &TaskId) -> Vec<(TaskId, CommitId)> {
    state
        .tasks
        .get(task)
        .map(|task| {
            task.dependencies
                .iter()
                .map(|dependency| (dependency.task.clone(), dependency.commit.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn restore_dependency_pins(state: &mut ProjectState, task: &TaskId, pins: &[(TaskId, CommitId)]) {
    if let Some(task) = state.tasks.get_mut(task) {
        for dependency in task.dependencies.iter_mut() {
            if let Some((_, commit)) = pins.iter().find(|(id, _)| id == &dependency.task) {
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
