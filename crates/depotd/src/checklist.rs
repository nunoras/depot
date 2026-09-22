use std::collections::BTreeMap;

use depot_core::{
    Checks, ProjectState, Question, Task, TaskState, Timestamp, dependency_satisfied,
};

use crate::vocabulary::{checks_name, role_name};

pub const UNOBSERVED_AFTER_MILLIS: u64 = 5 * 60 * 1000;

const SECTIONS: [(TaskState, &str, bool); 12] = [
    (
        TaskState::WaitingOnQuestion,
        "Needs you - waiting on an answer",
        false,
    ),
    (
        TaskState::Held,
        "Needs you - held for a project file change",
        false,
    ),
    (TaskState::Running, "Running", false),
    (TaskState::Validating, "Validating", false),
    (TaskState::Validated, "Validated", false),
    (TaskState::PrOpen, "Pull request open", false),
    (
        TaskState::ReworkPending,
        "Rework pending - pull request held",
        false,
    ),
    (TaskState::Approved, "Approved - queued", false),
    (TaskState::Proposed, "Held - awaiting approval", false),
    (TaskState::Landed, "Landed", true),
    (TaskState::Failed, "Failed", true),
    (TaskState::Cancelled, "Cancelled", true),
];

pub fn render_checklist(state: &ProjectState, history: bool) -> String {
    render_project(state, history, None)
}

pub fn render_checklist_observed(state: &ProjectState, history: bool, now: Timestamp) -> String {
    render_project(state, history, Some(now))
}

fn render_project(state: &ProjectState, history: bool, observed_at: Option<Timestamp>) -> String {
    let mut out = String::new();
    out.push_str("# Checklist\n\n");
    out.push_str(&format!(
        "Project: {}\n\n",
        one_line(state.project.as_str())
    ));
    out.push_str("Rendered from depot records. Hand edits are overwritten.\n");

    if let Some(now) = observed_at {
        let unobserved = unobserved_tasks(state, now);
        if !unobserved.is_empty() {
            out.push_str(&format!(
                "\n## Needs you - unobserved sessions ({})\n\n",
                unobserved.len()
            ));
            for (task, session, age) in &unobserved {
                out.push_str(&format!(
                    "- `{}` **{}** - session `{}` unobserved for {age}; check whether the worker is stuck\n",
                    task.id,
                    one_line(&task.title),
                    session
                ));
            }
        }
    }

    if state.tasks.is_empty() {
        out.push_str("\nNo tasks yet.\n");
        return out;
    }

    let mut hidden: BTreeMap<TaskState, usize> = BTreeMap::new();
    for (task_state, label, terminal) in SECTIONS {
        if terminal && !history {
            let count = state
                .tasks
                .values()
                .filter(|task| task.state == task_state)
                .count();
            if count > 0 {
                hidden.insert(task_state, count);
            }
            continue;
        }
        let mut tasks: Vec<&Task> = state
            .tasks
            .values()
            .filter(|task| task.state == task_state)
            .collect();
        if tasks.is_empty() {
            continue;
        }
        tasks.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        out.push_str(&format!("\n## {label} ({})\n", tasks.len()));
        for task in tasks {
            out.push('\n');
            render_task(&mut out, state, task, observed_at);
        }
    }

    if !hidden.is_empty() {
        let counts = [
            (TaskState::Landed, "landed"),
            (TaskState::Failed, "failed"),
            (TaskState::Cancelled, "cancelled"),
        ]
        .into_iter()
        .filter_map(|(task_state, label)| {
            hidden
                .get(&task_state)
                .map(|count| format!("{count} {label}"))
        })
        .collect::<Vec<_>>()
        .join(" · ");
        out.push_str(&format!(
            "\n{counts}; `depot status --history` shows them.\n"
        ));
    }

    out
}

fn render_task(
    out: &mut String,
    state: &ProjectState,
    task: &Task,
    observed_at: Option<Timestamp>,
) {
    out.push_str(&format!(
        "- `{}/{}` **{}** ({})\n",
        state.slug,
        task.id,
        one_line(&task.title),
        role_name(task.role)
    ));
    out.push_str(&format!("  - waits on: {}\n", waiting_on(state, task)));

    if let (TaskState::Running, Some(now)) = (task.state, observed_at)
        && let Some(line) = attempt_line(task, now)
    {
        out.push_str(&line);
    }

    if !task.dependencies.is_empty() {
        let dependencies: Vec<String> = task
            .dependencies
            .iter()
            .map(|dependency| {
                let standing = if dependency_satisfied(state, dependency) {
                    "validated"
                } else {
                    "pending"
                };
                format!(
                    "`{}` at `{}` ({standing})",
                    dependency.task, dependency.commit
                )
            })
            .collect();
        out.push_str(&format!("  - depends on: {}\n", dependencies.join(", ")));
    }

    if let Some(original) = &task.rework_of {
        out.push_str(&format!("  - rework of: `{original}`\n"));
    }

    if let Some((number, url, checks)) = task.pull_request() {
        out.push_str(&format!(
            "  - pull request: [#{number}]({}) - checks {}\n",
            one_line(url),
            checks_name(checks)
        ));
    }

    if let Some(base) = &task.conflict_base {
        out.push_str(&format!("  - conflicts with base `{base}`\n"));
    }

    if task.hold_pr {
        out.push_str("  - pull request held until `depot task release`\n");
    }

    if let Some(reason) = &task.merge_refused {
        out.push_str(&format!("  - auto-merge refused: {}\n", one_line(reason)));
    }

    for (lease, hold) in &task.release_held {
        out.push_str(&format!(
            "  - worktree lease `{lease}` held, not returned: {}\n",
            one_line(&hold.reason)
        ));
    }

    if let Some(reason) = &task.failure {
        out.push_str(&format!("  - failure: {}\n", one_line(reason)));
    }

    if task.state == TaskState::Running
        && let Some(deferral) = &task.turn_deferral
    {
        let times = if deferral.count == 1 { "time" } else { "times" };
        out.push_str(&format!(
            "  - worker turn deferred {} {}: {}\n",
            deferral.count,
            times,
            one_line(&deferral.reason)
        ));
    }

    if task.state.in_flight()
        && let Some(text) = &task.redirect_text
    {
        let status = if task.redirect_delivered {
            "delivered"
        } else {
            "queued, not yet delivered"
        };
        out.push_str(&format!("  - redirect ({}): {}\n", status, one_line(text)));
    }

    if let Some(question) = unanswered(task) {
        out.push_str(&format!("  - question: {}\n", one_line(&question.text)));
    }

    if task.state == TaskState::WaitingOnQuestion && !task.artifacts.is_empty() {
        let paths: Vec<String> = task
            .artifacts
            .iter()
            .map(|artifact| format!("`{}`", one_line(&artifact.path)))
            .collect();
        out.push_str(&format!("  - artifacts: {}\n", paths.join(", ")));
    }

    if let Some(record) = task.validations.last() {
        out.push_str(&format!(
            "  - last validation: `{}` at `{}` exited {}\n",
            one_line(&record.command),
            record.commit,
            record.exit_code
        ));
    }

    if let Some(retry) = &task.retry {
        out.push_str(&format!(
            "  - retry not before: {}\n",
            format_timestamp(retry.not_before)
        ));
    }

    if let Some(head) = &task.branch_head {
        out.push_str(&format!("  - branch head: `{head}`\n"));
    }
}

fn attempt_line(task: &Task, now: Timestamp) -> Option<String> {
    let attempt = task.attempts.last()?;
    if !attempt.outcome.is_open() {
        return None;
    }
    let age = format_age(now.millis().saturating_sub(attempt.started_at.millis()));
    let Some(session) = attempt.session.as_ref() else {
        return Some(format!(
            "  - attempt: stalled - in_flight {age}, no worker session\n"
        ));
    };
    let liveness = match attempt.last_seen_at {
        Some(seen) => {
            let seen_age = now.millis().saturating_sub(seen.millis());
            if seen_age > UNOBSERVED_AFTER_MILLIS {
                format!(
                    "session `{session}` unobserved for {}",
                    format_age(seen_age)
                )
            } else {
                format!("session `{session}` last seen {}", format_age(seen_age))
            }
        }
        None => format!("session `{session}` not yet seen alive"),
    };
    Some(format!("  - attempt: in_flight {age}, {liveness}\n"))
}

fn unobserved_tasks(state: &ProjectState, now: Timestamp) -> Vec<(&Task, String, String)> {
    state
        .tasks
        .values()
        .filter(|task| task.state == TaskState::Running)
        .filter_map(|task| {
            let attempt = task.attempts.last()?;
            if !attempt.outcome.is_open() {
                return None;
            }
            let session = attempt.session.as_ref()?;
            let seen = attempt.last_seen_at?;
            let seen_age = now.millis().saturating_sub(seen.millis());
            if seen_age <= UNOBSERVED_AFTER_MILLIS {
                return None;
            }
            Some((task, session.to_string(), format_age(seen_age)))
        })
        .collect()
}

fn format_age(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

fn waiting_on(state: &ProjectState, task: &Task) -> String {
    match task.state {
        TaskState::Proposed => "approval".to_string(),
        TaskState::Approved => approved_wait(state, task),
        TaskState::Running => match task
            .attempts
            .last()
            .and_then(|attempt| attempt.worktree.as_ref())
        {
            Some(lease) => format!("a worker turn in worktree lease `{lease}`"),
            None => "a worker turn".to_string(),
        },
        TaskState::WaitingOnQuestion => match unanswered(task) {
            Some(question) => format!("an answer to \"{}\"", one_line(&question.text)),
            None => "an answer".to_string(),
        },
        TaskState::Held => "a person to review the project file change".to_string(),
        TaskState::Validating => "validation".to_string(),
        TaskState::Validated => {
            if task.hold_pr {
                "a person to release the pull request".to_string()
            } else {
                "a pull request".to_string()
            }
        }
        TaskState::PrOpen => match task.pull_request() {
            Some((number, _, Checks::Passing)) => {
                format!("the merge of pull request #{number}")
            }
            Some((number, _, Checks::None)) => {
                format!("no checks configured; awaiting merge decision on pull request #{number}")
            }
            Some((number, _, Checks::Failing)) => {
                format!("failing checks on pull request #{number}")
            }
            Some((number, _, _)) => format!("checks on pull request #{number}"),
            None => "a pull request".to_string(),
        },
        TaskState::ReworkPending => "the rework to land on the pull request".to_string(),
        TaskState::Landed => "nothing".to_string(),
        TaskState::Failed => "a person".to_string(),
        TaskState::Cancelled => "nothing".to_string(),
    }
}

fn approved_wait(state: &ProjectState, task: &Task) -> String {
    if let Some(dependency) = task
        .dependencies
        .iter()
        .find(|dependency| !dependency_satisfied(state, dependency))
    {
        return format!(
            "dependency `{}` at `{}` to be validated",
            dependency.task, dependency.commit
        );
    }
    if let Some(retry) = &task.retry {
        return format!("a retry, not before {}", format_timestamp(retry.not_before));
    }
    "a free slot".to_string()
}

fn unanswered(task: &Task) -> Option<&Question> {
    task.questions
        .iter()
        .rev()
        .find(|question| question.answer.is_none())
}

pub(crate) fn one_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn format_timestamp(at: Timestamp) -> String {
    let seconds = at.millis() / 1000;
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let second_of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
