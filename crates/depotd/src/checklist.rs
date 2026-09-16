use depot_core::{
    Checks, ProjectState, Question, Task, TaskState, Timestamp, dependency_satisfied,
};

use crate::error::Result;
use crate::home::ProjectHome;
use crate::vocabulary::{checks_name, role_name};

const SECTIONS: [(TaskState, &str); 10] = [
    (TaskState::Proposed, "Held - awaiting approval"),
    (TaskState::Approved, "Approved - queued"),
    (TaskState::Running, "Running"),
    (TaskState::WaitingOnQuestion, "Waiting on a question"),
    (TaskState::Validating, "Validating"),
    (TaskState::Validated, "Validated"),
    (TaskState::PrOpen, "Pull request open"),
    (TaskState::Landed, "Landed"),
    (TaskState::Failed, "Blocked - needs a person"),
    (TaskState::Cancelled, "Cancelled"),
];

pub fn render_checklist(state: &ProjectState) -> String {
    let mut out = String::new();
    out.push_str("# Checklist\n\n");
    out.push_str(&format!(
        "Project: {}\n\n",
        one_line(state.project.as_str())
    ));
    out.push_str("Rendered from depot records. Hand edits are overwritten.\n");

    if state.tasks.is_empty() {
        out.push_str("\nNo tasks yet.\n");
        return out;
    }

    for (task_state, label) in SECTIONS {
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
            render_task(&mut out, state, task);
        }
    }

    out
}

pub fn write_checklist(
    project_home: &ProjectHome,
    state: &ProjectState,
) -> Result<std::path::PathBuf> {
    let path = project_home.checklist_path();
    std::fs::write(&path, render_checklist(state))?;
    Ok(path)
}

fn render_task(out: &mut String, state: &ProjectState, task: &Task) {
    out.push_str(&format!(
        "- `{}` **{}** ({})\n",
        task.id,
        one_line(&task.title),
        role_name(task.role)
    ));
    out.push_str(&format!("  - waits on: {}\n", waiting_on(state, task)));

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

    if let Some((number, url, checks)) = task.pull_request() {
        out.push_str(&format!(
            "  - pull request: [#{number}]({}) - checks {}\n",
            one_line(url),
            checks_name(checks)
        ));
    }

    if let Some(question) = unanswered(task) {
        out.push_str(&format!("  - question: {}\n", one_line(&question.text)));
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
        TaskState::Validating => "validation".to_string(),
        TaskState::Validated => "a pull request".to_string(),
        TaskState::PrOpen => match task.pull_request() {
            Some((number, _, Checks::Passing)) => {
                format!("the merge of pull request #{number}")
            }
            Some((number, _, Checks::Failing)) => {
                format!("failing checks on pull request #{number}")
            }
            Some((number, _, _)) => format!("checks on pull request #{number}"),
            None => "a pull request".to_string(),
        },
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

fn one_line(text: &str) -> String {
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
