use std::collections::BTreeMap;

use depot_core::{Liveness, Task, TaskId, TaskState, Timestamp};

use crate::checklist::{format_timestamp, one_line};
use crate::error::Result;
use crate::factcodec::{liveness_name, payload_field};
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
            line: line_for(tag, event, task)?,
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
        if need == Need::Nothing && group.len() > NOTHING_SHOWN {
            let earlier = group.len() - NOTHING_SHOWN;
            out.push_str(&format!("- ... and {earlier} earlier no-action facts\n",));
            for entry in &group[earlier..] {
                out.push_str(&format!(
                    "- {} at {}\n",
                    entry.line,
                    format_timestamp(entry.at)
                ));
            }
        } else {
            for entry in group {
                out.push_str(&format!(
                    "- {} at {}\n",
                    entry.line,
                    format_timestamp(entry.at)
                ));
            }
        }
    }
    out
}

const NOTHING_SHOWN: usize = 10;

fn need_for(tag: FactTag, task: Option<&Task>) -> Need {
    let state = task.map(|task| task.state);
    match tag {
        FactTag::QuestionAsked => match task {
            Some(task) if task.state == TaskState::WaitingOnQuestion => Need::User,
            Some(task) if task.questions.iter().any(|q| q.answer.is_none()) => Need::Coordinator,
            _ => Need::Nothing,
        },
        FactTag::ValidationFinished
        | FactTag::WorkerLivenessChanged
        | FactTag::WorkerTurnUnresolved
        | FactTag::PushFailed
        | FactTag::RunDurationExceeded
        | FactTag::RetryExhausted
        | FactTag::ProviderRateLimited
        | FactTag::PullRequestMerged
        | FactTag::PullRequestClosedUnmerged => match state {
            Some(TaskState::Failed) | Some(TaskState::Cancelled) => Need::User,
            _ => Need::Nothing,
        },
        FactTag::PullRequestMergeRefused => match state {
            Some(TaskState::PrOpen) => Need::User,
            _ => Need::Nothing,
        },
        FactTag::RebaseScheduled
        | FactTag::TaskDispatchJudged
        | FactTag::TaskProposed
        | FactTag::TaskReleased
        | FactTag::TaskApproved
        | FactTag::TaskCancelled
        | FactTag::TaskAcknowledged
        | FactTag::QuestionAnswered
        | FactTag::WorktreeAcquireRequested
        | FactTag::WorkerTurnLaunchRequested
        | FactTag::WorkerTurnResumeRequested
        | FactTag::WorkerTurnStarted
        | FactTag::WorkerTurnEnded
        | FactTag::WorkerRedirected
        | FactTag::WorkerSubmissionRecorded
        | FactTag::WorkerSubmitted
        | FactTag::ValidationStarted
        | FactTag::WorktreeAcquired
        | FactTag::BranchPushed
        | FactTag::PullRequestOpened
        | FactTag::PullRequestChecksChanged
        | FactTag::CoordinatorSessionStarted
        | FactTag::CoordinatorContextMeasured
        | FactTag::DaemonRestarted
        | FactTag::OnEventNotified
        | FactTag::Polled => Need::Nothing,
    }
}

fn line_for(tag: FactTag, event: &RecordedEvent, task: Option<&Task>) -> Result<String> {
    let subject = match (event.task.as_ref(), task) {
        (Some(id), Some(task)) => format!(
            "`{id}` **{}** ({}): ",
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
        FactTag::WorkerTurnUnresolved => {
            "a worker turn could not be resolved; the worker may already be running".to_string()
        }
        FactTag::WorkerTurnStarted => "worker turn started".to_string(),
        FactTag::WorkerTurnEnded => "worker turn ended".to_string(),
        FactTag::WorkerRedirected => match payload_field(&event.payload, "text") {
            Ok(text) => format!(
                "a new direction was queued for the worker: {}",
                one_line(&text)
            ),
            Err(_) => "a new direction was queued for the worker".to_string(),
        },
        FactTag::WorkerLivenessChanged => liveness_line(&event.payload)?,
        FactTag::WorkerSubmissionRecorded => "recorded a submission".to_string(),
        FactTag::WorkerSubmitted => "submitted a change".to_string(),
        FactTag::ValidationStarted => "validation started".to_string(),
        FactTag::ValidationFinished => match task.and_then(|task| task.validations.last()) {
            Some(record) => format!(
                "validation `{}` at `{}` exited {}",
                one_line(&record.command),
                record.commit,
                record.exit_code
            ),
            None => "validation finished".to_string(),
        },
        FactTag::WorktreeAcquired => match lease(task) {
            Some(lease) => format!("worktree lease `{lease}` acquired"),
            None => "a worktree was acquired".to_string(),
        },
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
        FactTag::PushFailed => {
            let reason = payload_field(&event.payload, "reason")?;
            format!("the push was rejected: {}", one_line(&reason))
        }
        FactTag::RebaseScheduled => {
            "a conflicting pull request was scheduled for a rebase".to_string()
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
