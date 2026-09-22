use std::collections::BTreeMap;

use depot_core::{Liveness, Task, TaskId, TaskState, Timestamp};

use crate::checklist::{format_timestamp, one_line};
use crate::error::Result;
use crate::factcodec::{liveness_name, payload_array, payload_field};
use crate::store::RecordedEvent;
use crate::vocabulary::{FactTag, fact_tag_from_name, state_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Need {
    User,
    Coordinator,
    Nothing,
}

impl Need {
    pub fn heading(self) -> &'static str {
        match self {
            Need::User => "For the user",
            Need::Coordinator => "For you",
            Need::Nothing => "No action",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEntry {
    pub at: Timestamp,
    pub task: Option<TaskId>,
    pub line: String,
    pub need: Need,
}

pub fn inbox_entries(
    slug: &str,
    tasks: &BTreeMap<TaskId, Task>,
    events: &[RecordedEvent],
) -> Result<Vec<InboxEntry>> {
    let mut entries = Vec::new();
    for event in events {
        let tag = fact_tag_from_name(&event.kind)?;
        if matches!(
            tag,
            FactTag::Polled | FactTag::CoordinatorContextMeasured | FactTag::OnEventNotified
        ) {
            continue;
        }
        let task = event.task.as_ref().and_then(|id| tasks.get(id));
        entries.push(InboxEntry {
            at: event.at,
            task: event.task.clone(),
            line: line_for(tag, event, task, slug)?,
            need: need_for(tag, task),
        });
    }
    Ok(entries)
}

pub fn render_inbox(entries: &[InboxEntry]) -> String {
    let mut out = String::from("# Inbox\n\n");
    if entries.is_empty() {
        out.push_str("No facts since your last turn.\n");
        return out;
    }
    out.push_str(&format!("{} since your last turn.\n", count(entries.len())));

    for need in [Need::User, Need::Coordinator, Need::Nothing] {
        let group: Vec<&InboxEntry> = entries.iter().filter(|entry| entry.need == need).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {} ({})\n", need.heading(), group.len()));
        let runs = collapse_runs(&group);
        let shown = if need == Need::Nothing && runs.len() > NOTHING_SHOWN {
            let earlier = runs.len() - NOTHING_SHOWN;
            out.push_str(&format!("- ... and {earlier} earlier no-action facts\n",));
            earlier
        } else {
            0
        };
        for (line, first, last, run) in &runs[shown..] {
            if *run == 1 {
                out.push_str(&format!("- {line} at {}\n", format_timestamp(*first)));
            } else {
                out.push_str(&format!(
                    "- {line} at {} ({run} times through {})\n",
                    format_timestamp(*first),
                    format_timestamp(*last)
                ));
            }
        }
    }
    out
}

const NOTHING_SHOWN: usize = 10;

type Run = (String, Timestamp, Timestamp, usize);

fn collapse_runs(group: &[&InboxEntry]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for entry in group {
        match runs.last_mut() {
            Some((line, _, last, count)) if *line == entry.line => {
                *last = entry.at;
                *count += 1;
            }
            _ => runs.push((entry.line.clone(), entry.at, entry.at, 1)),
        }
    }
    runs
}

fn need_for(tag: FactTag, task: Option<&Task>) -> Need {
    let state = task.map(|task| task.state);
    match tag {
        FactTag::QuestionAsked => match task {
            Some(task) if task.state == TaskState::WaitingOnQuestion => Need::User,
            Some(task) if task.questions.iter().any(|q| q.answer.is_none()) => Need::Coordinator,
            _ => Need::Nothing,
        },
        FactTag::ValidationFinished
        | FactTag::ValidationFailed
        | FactTag::WorkerCommittedNothing
        | FactTag::WorkerLivenessChanged
        | FactTag::WorkerSessionFailed
        | FactTag::WorkerTurnUnresolved
        | FactTag::PushFailed
        | FactTag::DeliveryFailed
        | FactTag::DescribeFailed
        | FactTag::EvidenceFailed
        | FactTag::RunDurationExceeded
        | FactTag::RetryExhausted
        | FactTag::ProviderRateLimited
        | FactTag::PullRequestMerged
        | FactTag::StaleMergeObserved
        | FactTag::PullRequestClosedUnmerged => match state {
            Some(TaskState::Failed) | Some(TaskState::Cancelled) => Need::User,
            _ => Need::Nothing,
        },
        FactTag::PullRequestMergeRefused => match state {
            Some(TaskState::PrOpen) => Need::User,
            _ => Need::Nothing,
        },
        FactTag::WorktreeReleaseHeld => Need::User,
        FactTag::ProjectFileChanged => Need::User,
        FactTag::RebaseScheduled
        | FactTag::TaskDispatchJudged
        | FactTag::TaskProposed
        | FactTag::TaskReleased
        | FactTag::TaskApproved
        | FactTag::TaskCancelled
        | FactTag::TaskRetried
        | FactTag::TaskReworked
        | FactTag::TaskAcknowledged
        | FactTag::QuestionAnswered
        | FactTag::WorktreeAcquireRequested
        | FactTag::WorkerTurnLaunchRequested
        | FactTag::WorkerTurnResumeRequested
        | FactTag::WorkerRelaunchRequested
        | FactTag::WorkerTurnDeferred
        | FactTag::WorkerTurnStarted
        | FactTag::WorkerTurnEnded
        | FactTag::WorkerRedirected
        | FactTag::WorkerRedirectDelivered
        | FactTag::WorkerSubmissionRecorded
        | FactTag::WorkerSubmitted
        | FactTag::ValidationStarted
        | FactTag::WorktreeAcquired
        | FactTag::WorktreeBaselined
        | FactTag::WorktreeReleased
        | FactTag::BranchPushed
        | FactTag::PullRequestOpened
        | FactTag::TaskLandedOnBase
        | FactTag::PullRequestChecksChanged
        | FactTag::PullRequestMergeabilityChanged
        | FactTag::EvidencePosted
        | FactTag::CoordinatorSessionStarted
        | FactTag::CoordinatorContextMeasured
        | FactTag::DaemonRestarted
        | FactTag::OnEventNotified
        | FactTag::Polled => Need::Nothing,
    }
}

fn line_for(
    tag: FactTag,
    event: &RecordedEvent,
    task: Option<&Task>,
    slug: &str,
) -> Result<String> {
    let subject = match (event.task.as_ref(), task) {
        (Some(id), Some(task)) => format!(
            "`{slug}/{id}` **{}** ({}): ",
            one_line(&task.title),
            state_name(task.state)
        ),
        (Some(id), None) => format!("`{id}`: "),
        (None, _) => String::new(),
    };
    Ok(format!("{subject}{}", headline(tag, event, task)?))
}

fn headline(tag: FactTag, event: &RecordedEvent, task: Option<&Task>) -> Result<String> {
    Ok(match tag {
        FactTag::TaskDispatchJudged => "dispatch model judgement recorded".to_string(),
        FactTag::TaskProposed => "a task was filed, holding for approval".to_string(),
        FactTag::TaskApproved => "approved".to_string(),
        FactTag::TaskCancelled => "stopped".to_string(),
        FactTag::TaskRetried => "scheduled to run again".to_string(),
        FactTag::TaskReworked => match task.and_then(|task| task.rework_of.as_ref()) {
            Some(original) => format!(
                "a rework was filed as `{original}` to address review findings on the open pull request"
            ),
            None => "a rework was filed".to_string(),
        },
        FactTag::TaskAcknowledged => "acknowledged; it fades from the default status".to_string(),
        FactTag::TaskReleased => "the pull request hold was released".to_string(),
        FactTag::QuestionAsked => match unanswered(task) {
            Some(question) => format!("asked \"{}\"", one_line(&question.text)),
            None => "asked a question that is already answered".to_string(),
        },
        FactTag::QuestionAnswered => "answered".to_string(),
        FactTag::WorktreeAcquireRequested => "worktree acquire requested".to_string(),
        FactTag::WorkerTurnLaunchRequested => "worker turn launch requested".to_string(),
        FactTag::WorkerTurnResumeRequested => "worker turn resume requested".to_string(),
        FactTag::WorkerRelaunchRequested => {
            "the worker was relaunched with the pending answer".to_string()
        }
        FactTag::WorkerTurnUnresolved => match payload_field(&event.payload, "reason") {
            Ok(reason) => format!("a worker turn could not be resolved: {}", one_line(&reason)),
            Err(_) => "a worker turn could not be resolved".to_string(),
        },
        FactTag::WorkerTurnStarted => "worker turn started".to_string(),
        FactTag::WorkerTurnEnded => "worker turn ended".to_string(),
        FactTag::WorkerRedirected => match payload_field(&event.payload, "text") {
            Ok(text) => format!(
                "a new direction was queued for the worker: {}",
                one_line(&text)
            ),
            Err(_) => "a new direction was queued for the worker".to_string(),
        },
        FactTag::WorkerRedirectDelivered => {
            "the queued direction was delivered to the worker".to_string()
        }
        FactTag::WorkerLivenessChanged => liveness_line(&event.payload)?,
        FactTag::WorkerSessionFailed => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the worker session failed: {}", one_line(&reason))
        }
        FactTag::WorkerTurnDeferred => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the worker turn could not proceed: {}", one_line(&reason))
        }
        FactTag::WorkerSubmissionRecorded => "recorded a submission".to_string(),
        FactTag::WorkerSubmitted => "submitted a change".to_string(),
        FactTag::ValidationStarted => "validation started".to_string(),
        FactTag::ProjectFileChanged => {
            let files = payload_array(&event.payload, "files")?;
            format!(
                "the change edits project files a person must review and merge: {}",
                one_line(&files.join(", "))
            )
        }
        FactTag::ValidationFinished => match task.and_then(|task| task.validations.last()) {
            Some(record) => format!(
                "validation `{}` at `{}` exited {}",
                one_line(&record.command),
                record.commit,
                record.exit_code
            ),
            None => "validation finished".to_string(),
        },
        FactTag::ValidationFailed => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the validation could not run: {}", one_line(&reason))
        }
        FactTag::WorkerCommittedNothing => {
            let reason = payload_field(&event.payload, "reason")?;
            one_line(&reason)
        }
        FactTag::WorktreeAcquired => match lease(task) {
            Some(lease) => format!("worktree lease `{lease}` acquired"),
            None => "a worktree was acquired".to_string(),
        },
        FactTag::WorktreeBaselined => {
            let commit = payload_field(&event.payload, "commit")?;
            format!("worktree baselined at `{commit}`")
        }
        FactTag::WorktreeReleased => {
            let lease = payload_field(&event.payload, "lease")?;
            format!("worktree lease `{lease}` returned to the pool")
        }
        FactTag::WorktreeReleaseHeld => {
            let lease = payload_field(&event.payload, "lease")?;
            let reason = payload_field(&event.payload, "reason")?;
            format!(
                "worktree lease `{lease}` is held, not returned: {}",
                one_line(&reason)
            )
        }
        FactTag::BranchPushed => match commit(task) {
            Some(commit) => format!("branch pushed at `{commit}`"),
            None => "the branch was pushed".to_string(),
        },
        FactTag::PullRequestOpened => match task.and_then(|task| task.pull_request()) {
            Some((number, url, _)) => format!("pull request #{number} opened at {}", one_line(url)),
            None => "a pull request was opened".to_string(),
        },
        FactTag::PullRequestChecksChanged => "pull request checks changed".to_string(),
        FactTag::PullRequestMerged => "the pull request merged".to_string(),
        FactTag::StaleMergeObserved => {
            "the pull request had already merged when the daemon restarted".to_string()
        }
        FactTag::PullRequestClosedUnmerged => "the pull request closed unmerged".to_string(),
        FactTag::PullRequestMergeRefused => {
            let reason = payload_field(&event.payload, "reason")?;
            match task.and_then(|task| task.pull_request()) {
                Some((number, _, _)) => format!(
                    "the forge refused to merge pull request #{number}: {}",
                    one_line(&reason)
                ),
                None => format!("the forge refused to merge: {}", one_line(&reason)),
            }
        }
        FactTag::PullRequestMergeabilityChanged => {
            match task.and_then(|task| task.conflict_base.as_ref()) {
                Some(base) => format!("the pull request conflicts with base `{base}`"),
                None => "the pull request no longer conflicts with its base".to_string(),
            }
        }
        FactTag::EvidencePosted => match task.and_then(|task| task.pull_request()) {
            Some((number, _, _)) => format!("evidence was posted to pull request #{number}"),
            None => "evidence was posted".to_string(),
        },
        FactTag::EvidenceFailed => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the evidence capture failed: {}", one_line(&reason))
        }
        FactTag::PushFailed => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the push was rejected: {}", one_line(&reason))
        }
        FactTag::DeliveryFailed => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the delivery failed: {}", one_line(&reason))
        }
        FactTag::TaskLandedOnBase => {
            "the validated commit was already on the base branch; the task landed without a pull request"
                .to_string()
        }
        FactTag::DescribeFailed => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the describe step failed: {}", one_line(&reason))
        }
        FactTag::RebaseScheduled => {
            "a conflicting pull request was scheduled to merge the base branch".to_string()
        }
        FactTag::RunDurationExceeded => "ran past its run duration".to_string(),
        FactTag::RetryExhausted => "ran out of retries".to_string(),
        FactTag::ProviderRateLimited => "hit a provider rate limit".to_string(),
        FactTag::CoordinatorSessionStarted => "this session started".to_string(),
        FactTag::CoordinatorContextMeasured => "context measured".to_string(),
        FactTag::OnEventNotified => "the daemon ran its event hook".to_string(),
        FactTag::DaemonRestarted => "the daemon restarted".to_string(),
        FactTag::Polled => "the daemon polled".to_string(),
    })
}

fn liveness_line(payload: &str) -> Result<String> {
    let liveness = payload_field(payload, "liveness")?;
    Ok(if liveness == liveness_name(Liveness::Live) {
        "the worker is live".to_string()
    } else {
        "the worker is gone".to_string()
    })
}

fn unanswered(task: Option<&Task>) -> Option<&depot_core::Question> {
    task?.questions.iter().rev().find(|q| q.answer.is_none())
}

fn commit(task: Option<&Task>) -> Option<&depot_core::CommitId> {
    task?.branch_head.as_ref()
}

fn lease(task: Option<&Task>) -> Option<&depot_core::WorktreeLease> {
    task?
        .attempts
        .last()
        .and_then(|attempt| attempt.worktree.as_ref())
}

fn count(facts: usize) -> String {
    if facts == 1 {
        "1 fact".to_string()
    } else {
        format!("{facts} facts")
    }
}
