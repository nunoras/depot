use std::path::{Path, PathBuf};
use std::time::Duration;

use depot_core::{CommitId, Task};

use crate::adapters::profiles::ProfileSpec;
use crate::adapters::sessions::{LaunchRequest, SessionProfile, Sessions, TurnOutcome};
use crate::error::{Error, Result};

const MAX_DIFF_BYTES: usize = 60_000;
const TRUNCATED_DIFF_BYTES: usize = 40_000;

pub struct DescribeInput {
    pub title: String,
    pub diff: String,
    pub directory: PathBuf,
    pub output_path: PathBuf,
}

pub struct DescribeOutput {
    pub title: String,
    pub body: String,
}

pub trait Describer {
    fn describe(&self, input: &DescribeInput) -> Result<DescribeOutput>;
}

pub fn parse(text: &str) -> Option<DescribeOutput> {
    let (title, body) = text.trim().split_once('\n')?;
    let title = title.trim();
    let body = body.trim();
    if title.is_empty() || body.is_empty() {
        return None;
    }
    Some(DescribeOutput {
        title: title.to_owned(),
        body: body.to_owned(),
    })
}

pub fn validation_section(task: &Task, commit: &CommitId) -> String {
    let validation = task
        .validations
        .iter()
        .rev()
        .find(|record| &record.commit == commit);
    let result = validation
        .map(|record| {
            format!(
                "`{}` at `{}` exited {}",
                record.command, record.commit, record.exit_code
            )
        })
        .unwrap_or_else(|| "no validation record".to_string());
    format!("## Validation\n\n{result}")
}

pub fn assemble(
    describe: Option<DescribeOutput>,
    task: &Task,
    commit: &CommitId,
) -> (String, String) {
    let validation = validation_section(task, commit);
    match describe {
        Some(output) => (
            output.title,
            format!("{}\n\n{}\n", output.body.trim_end(), validation),
        ),
        None => (task.title.clone(), format!("{validation}\n")),
    }
}

pub fn diff_section(diff: &str, diffstat: &str) -> String {
    if diff.len() <= MAX_DIFF_BYTES {
        return diff.to_owned();
    }
    let cut = diff
        .char_indices()
        .nth(TRUNCATED_DIFF_BYTES)
        .map(|(index, _)| index)
        .unwrap_or(diff.len());
    format!(
        "{diffstat}\n\nThe diff is large; only its beginning follows.\n\n{}",
        &diff[..cut]
    )
}

pub fn prompt(title: &str, diff: &str, output_path: &Path) -> String {
    format!(
        "Load the /technical-writing skill and apply its unslop rules before writing.\n\
         \n\
         You are writing the description of a pull request for its reviewers. \
         The one-line summary of the change is: {title}\n\
         \n\
         Write a reviewer-facing description: the one-line title on the first line, \
         then a `## Why` section of one or two sentences, a `## What changed` section of \
         tight bullets drawn only from the diff, and a `## How to review` section.\n\
         \n\
         Never include URLs, internal tool names, verify commands, or any mention of a \
         sketch or mockup. No emoji, no em dashes, plain sentences, under about 180 words.\n\
         \n\
         Write the finished description, and nothing else, to the file {}\n\
         \n\
         The unified diff of the pull request branch against its base:\n\
         \n\
         {diff}",
        output_path.display()
    )
}

pub struct SessionDescriber<'a, S> {
    sessions: &'a S,
    spec: ProfileSpec,
    timeout: Duration,
}

impl<'a, S: Sessions> SessionDescriber<'a, S> {
    pub fn new(sessions: &'a S, spec: ProfileSpec, timeout: Duration) -> Self {
        Self {
            sessions,
            spec,
            timeout,
        }
    }
}

impl<S: Sessions> Describer for SessionDescriber<'_, S> {
    fn describe(&self, input: &DescribeInput) -> Result<DescribeOutput> {
        let account = self.spec.account.trim();
        let session = self
            .sessions
            .launch(&LaunchRequest {
                directory: input.directory.clone(),
                profile: SessionProfile {
                    account: if account.is_empty() {
                        None
                    } else {
                        Some(account.into())
                    },
                    harness: self.spec.harness.clone(),
                    model: self.spec.model.clone(),
                    effort: self.spec.effort.clone(),
                },
                kind: Some("describe".to_owned()),
                prompt: prompt(&input.title, &input.diff, &input.output_path),
            })
            .map_err(|error| Error::Project(error.to_string()))?;
        let outcome = self
            .sessions
            .wait(&session, Some(self.timeout))
            .map_err(|error| Error::Project(error.to_string()));
        let _ = self.sessions.stop(&session);
        let outcome = outcome?;
        if outcome != TurnOutcome::Completed {
            return Err(Error::Project(format!(
                "the describe worker turn ended as {outcome:?}"
            )));
        }
        let text = std::fs::read_to_string(&input.output_path).map_err(|error| {
            Error::Project(format!("the describe worker wrote no output: {error}"))
        })?;
        let _ = std::fs::remove_file(&input.output_path);
        parse(&text)
            .ok_or_else(|| Error::Project("the describe worker wrote no usable output".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use depot_core::{
        CommitId, ProjectId, Role, SessionId, Task, TaskId, TaskState, Timestamp, ValidationRecord,
    };

    use super::TurnOutcome;
    use super::{Describer, assemble, diff_section, parse, prompt, validation_section};
    use crate::adapters::sessions::Sessions;

    fn task() -> Task {
        Task {
            id: TaskId::new("t-44"),
            project: ProjectId::new("depot"),
            title: "PR body from a describe step".to_owned(),
            intent: "internal brief with https://example.test and verify commands".to_owned(),
            role: Role::Build,
            dispatch_profile: None,
            state: TaskState::PrOpen,
            dependencies: Vec::new(),
            base_dependency: None,
            attempts: Vec::new(),
            questions: Vec::new(),
            validations: vec![ValidationRecord {
                command: "cargo test".to_owned(),
                commit: CommitId::new("abc123"),
                exit_code: 0,
                duration: Duration::from_secs(5),
                output_tail: String::new(),
            }],
            submission: None,
            artifacts: Vec::new(),
            links: Vec::new(),
            branch_head: None,
            merge_refused: None,
            acknowledged_at: None,
            hold_pr: false,
            retry: None,
            created_at: Timestamp::from_millis(0),
            updated_at: Timestamp::from_millis(0),
        }
    }

    fn described() -> super::DescribeOutput {
        super::DescribeOutput {
            title: "Describe PR bodies from the diff".to_owned(),
            body: "## Why\n\nReviewers need a real description.\n\n## What changed\n\n- Split the body assembler.".to_owned(),
        }
    }

    #[test]
    fn the_first_line_becomes_the_title_and_the_rest_the_body() {
        let output = parse("Title line\n\nBody text.").expect("parsed");
        assert_eq!(output.title, "Title line");
        assert_eq!(output.body, "Body text.");
    }

    #[test]
    fn blank_or_single_line_output_is_refused() {
        assert!(parse("").is_none());
        assert!(parse("only a title").is_none());
        assert!(parse("\n\nbody only").is_none());
    }

    #[test]
    fn the_worker_body_is_joined_with_the_validation_section_after_it() {
        let (title, body) = assemble(Some(described()), &task(), &CommitId::new("abc123"));
        assert_eq!(title, "Describe PR bodies from the diff");
        assert!(body.starts_with("## Why"));
        assert!(body.ends_with("## Validation\n\n`cargo test` at `abc123` exited 0\n"));
        assert_eq!(body.matches("## Validation").count(), 1);
    }

    #[test]
    fn the_fallback_is_the_task_title_and_the_validation_section_alone() {
        let (title, body) = assemble(None, &task(), &CommitId::new("abc123"));
        assert_eq!(title, "PR body from a describe step");
        assert_eq!(body, "## Validation\n\n`cargo test` at `abc123` exited 0\n");
    }

    #[test]
    fn the_intent_never_reaches_the_pull_request() {
        let record = task();
        let (with_describe, described_body) =
            assemble(Some(described()), &record, &CommitId::new("abc123"));
        let (fallback_title, fallback_body) = assemble(None, &record, &CommitId::new("abc123"));
        for text in [described_body, fallback_body, with_describe, fallback_title] {
            assert!(!text.contains(&record.intent));
        }
    }

    #[test]
    fn a_large_diff_is_replaced_by_its_diffstat_and_a_truncated_body() {
        let diff = "x".repeat(70_000);
        let section = diff_section(&diff, " file.rs | 10 +++---");
        assert!(section.starts_with(" file.rs | 10 +++---"));
        assert!(section.len() < 45_000);
        assert_eq!(diff_section("small", "stat"), "small");
    }

    #[test]
    fn the_prompt_carries_the_title_not_the_intent() {
        let text = prompt("The title", "the diff", std::path::Path::new("/tmp/out.md"));
        assert!(text.contains("The title"));
        assert!(text.contains("/tmp/out.md"));
        assert!(text.contains("/technical-writing"));
        assert!(text.contains("unslop"));
    }

    #[test]
    fn validation_without_a_record_names_it() {
        let mut record = task();
        record.validations.clear();
        assert_eq!(
            validation_section(&record, &CommitId::new("abc123")),
            "## Validation\n\nno validation record"
        );
    }

    use std::path::PathBuf;

    use crate::adapters::sessions::{
        Capabilities, LaunchRequest, SessionError, SessionState, SessionSummary,
    };

    struct FakeSessions {
        output_path: PathBuf,
        outcome: TurnOutcome,
        written: bool,
    }

    impl FakeSessions {
        fn finish_turn(&self) {
            if self.written {
                std::fs::write(&self.output_path, "Fake title\n\nFake body for review.\n")
                    .expect("the worker writes its output");
            }
        }
    }

    impl Sessions for FakeSessions {
        fn capabilities(&self) -> Result<Capabilities, SessionError> {
            unimplemented!()
        }
        fn launch(&self, _request: &LaunchRequest) -> Result<SessionId, SessionError> {
            self.finish_turn();
            Ok(SessionId::new("sess-1"))
        }
        fn resume(&self, _: &SessionId, _: &str) -> Result<(), SessionError> {
            unimplemented!()
        }
        fn status(&self, _: &SessionId) -> Result<SessionState, SessionError> {
            unimplemented!()
        }
        fn wait(&self, _: &SessionId, _: Option<Duration>) -> Result<TurnOutcome, SessionError> {
            Ok(self.outcome)
        }
        fn stop(&self, _: &SessionId) -> Result<(), SessionError> {
            Ok(())
        }
        fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
            unimplemented!()
        }
    }

    fn session_describer(
        outcome: TurnOutcome,
    ) -> (
        super::SessionDescriber<'static, FakeSessions>,
        PathBuf,
        tempfile::TempDir,
    ) {
        session_describer_with(outcome, true)
    }

    fn session_describer_with(
        outcome: TurnOutcome,
        written: bool,
    ) -> (
        super::SessionDescriber<'static, FakeSessions>,
        PathBuf,
        tempfile::TempDir,
    ) {
        let temp = tempfile::TempDir::new().expect("temporary directory");
        let output_path = temp.path().join("describe.md");
        let sessions: &'static FakeSessions = Box::leak(Box::new(FakeSessions {
            output_path: output_path.clone(),
            outcome,
            written,
        }));
        let spec = crate::adapters::profiles::ProfileSpec {
            profile: depot_core::ProfileId::new("writer"),
            harness: "h".to_owned(),
            model: "m".to_owned(),
            effort: "e".to_owned(),
            account: String::new(),
        };
        (
            super::SessionDescriber::new(sessions, spec, Duration::from_secs(10)),
            output_path,
            temp,
        )
    }

    fn input(output_path: PathBuf) -> super::DescribeInput {
        super::DescribeInput {
            title: "The title".to_owned(),
            diff: "the diff".to_owned(),
            directory: PathBuf::from("."),
            output_path,
        }
    }

    #[test]
    fn the_session_describer_parses_the_worker_output_file() {
        let (describer, output_path, _temp) = session_describer(TurnOutcome::Completed);
        let output = describer.describe(&input(output_path)).expect("described");
        assert_eq!(output.title, "Fake title");
        assert_eq!(output.body, "Fake body for review.");
    }

    #[test]
    fn a_failed_turn_falls_through_to_the_fallback() {
        let (describer, output_path, _temp) = session_describer(TurnOutcome::Failed);
        assert!(describer.describe(&input(output_path)).is_err());
    }

    #[test]
    fn an_empty_output_file_is_a_failure() {
        let (describer, output_path, _temp) = session_describer_with(TurnOutcome::Completed, false);
        std::fs::write(&output_path, "").expect("empty file");
        assert!(describer.describe(&input(output_path)).is_err());
    }

    #[test]
    fn a_missing_output_file_is_a_failure() {
        let (describer, output_path, _temp) = session_describer_with(TurnOutcome::Completed, false);
        assert!(describer.describe(&input(output_path)).is_err());
    }
}
