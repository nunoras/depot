mod support;

use depot_core::{
    Checks, CommitId, Fact, FactKind, ProjectId, SessionId, TaskId, TaskState, Timestamp,
    ValidationRecord,
};
use depotd::{Store, read_inbox};

const BUILD_ONLY: &str = "base_branch = \"main\"\n\n\
                          [profiles]\n\
                          build = \"glm-5.3\"\n";

fn apply(
    store: &Store,
    project: &ProjectId,
    key: &str,
    at: u64,
    kind: FactKind,
) -> Vec<depot_core::Action> {
    let owner = store
        .project_by_slug("example")
        .expect("project lookup")
        .expect("registered");
    let fact = Fact {
        at: Timestamp::from_millis(at),
        kind,
    };
    assert_eq!(owner.id, *project);
    store
        .apply_fact(&owner, key, &fact)
        .expect("applied")
        .actions
}

fn proposed(id: &str, title: &str) -> FactKind {
    FactKind::TaskProposed {
        task: TaskId::new(id),
        title: title.to_string(),
        intent: format!("intent for {id}"),
        role: depot_core::Role::Build,
        dispatch_profile: None,
        dependencies: Vec::new(),
        base_dependency: None,
        hold_pr: false,
    }
}

fn approved(id: &str) -> FactKind {
    FactKind::TaskApproved {
        task: TaskId::new(id),
    }
}

fn asked(id: &str, text: &str, relay: bool) -> FactKind {
    FactKind::QuestionAsked {
        task: TaskId::new(id),
        text: text.to_string(),
        relay,
    }
}

fn section<'a>(rendered: &'a str, heading: &str) -> &'a str {
    let marker = format!("## {heading} (");
    let start = rendered
        .find(&marker)
        .unwrap_or_else(|| panic!("no `{marker}` section in\n{rendered}"));
    let rest = &rendered[start..];
    let end = rest[3..]
        .find("\n## ")
        .map(|index| index + 3)
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn the_inbox_payload_reports_a_relayed_question_and_a_fact_that_needs_no_action() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let project = &added.project.id;
    let first = TaskId::new("t-1");

    apply(
        &store,
        project,
        "task_proposed:t-1",
        1_000,
        proposed("t-1", "Wire the store"),
    );
    apply(&store, project, "task_approved:t-1", 2_000, approved("t-1"));
    apply(
        &store,
        project,
        "question_asked:t-1:0",
        3_000,
        asked("t-1", "Should the cap be per project or per task?", true),
    );
    apply(
        &store,
        project,
        "worker_turn_started:t-1",
        4_000,
        FactKind::WorkerTurnStarted {
            task: first.clone(),
            session: SessionId::new("s1"),
        },
    );
    apply(
        &store,
        project,
        "task_proposed:t-2",
        5_000,
        proposed("t-2", "Docs pass"),
    );
    apply(&store, project, "task_approved:t-2", 6_000, approved("t-2"));
    apply(
        &store,
        project,
        "question_asked:t-2:0",
        7_000,
        asked("t-2", "Which file holds the glossary?", false),
    );

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    let user = section(&payload, "For the user");
    assert!(
        user.contains("Should the cap be per project or per task?"),
        "a relayed question needs the user, got\n{payload}"
    );
    assert!(user.contains("t-1"), "got\n{payload}");
    assert!(
        !user.contains("Which file holds the glossary?"),
        "a question the brief answers is not the user's, got\n{payload}"
    );

    let coordinator = section(&payload, "For you");
    assert!(
        coordinator.contains("Which file holds the glossary?"),
        "an unsettled question the coordinator can answer is its own work, got\n{payload}"
    );

    let nothing = section(&payload, "No action");
    assert!(
        nothing.contains("worker turn started"),
        "an observed turn changes nothing, got\n{payload}"
    );
    assert!(
        !nothing.contains("Should the cap"),
        "the relayed question is not also a no-op, got\n{payload}"
    );
    assert_eq!(
        store.inbox_cursor(project).expect("cursor"),
        store
            .events(project)
            .expect("events")
            .last()
            .expect("a recorded fact")
            .id,
        "reading the inbox advances the cursor to the last fact it read"
    );

    let again = read_inbox(&fixture.home, Some("example")).expect("inbox again");
    assert_eq!(
        again, "# Inbox\n\nNo facts since your last turn.\n",
        "a turn only reads what happened since the previous one"
    );
}

#[test]
fn the_inbox_payload_names_the_task_state_and_the_owner_of_each_fact() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let project = &added.project.id;

    apply(
        &store,
        project,
        "task_proposed:t-1",
        1_000,
        proposed("t-1", "Wire the store"),
    );
    apply(&store, project, "task_approved:t-1", 2_000, approved("t-1"));
    apply(
        &store,
        project,
        "worker_submitted:t-1",
        3_000,
        FactKind::WorkerSubmitted {
            task: TaskId::new("t-1"),
            commit: CommitId::new("abc123"),
        },
    );
    apply(
        &store,
        project,
        "validation_finished:t-1",
        4_000,
        FactKind::ValidationFinished {
            task: TaskId::new("t-1"),
            command: "cargo test".to_string(),
            commit: CommitId::new("abc123"),
            exit_code: 0,
            duration: std::time::Duration::from_millis(1_200),
            output_tail: "ok".to_string(),
        },
    );

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    assert!(
        payload.contains("`t-1` **Wire the store** (validated)"),
        "each fact names the task and where it stands now, got\n{payload}"
    );
    assert!(
        section(&payload, "No action").contains("validation `cargo test` at `abc123` exited 0"),
        "a passing validation needs no decision, got\n{payload}"
    );
    assert!(
        section(&payload, "No action").contains("submitted a change"),
        "a submission is the daemon's work, not the coordinator's, got\n{payload}"
    );
    assert!(
        payload.starts_with("# Inbox\n\n4 facts since your last turn.\n"),
        "the payload counts the facts it reports, got\n{payload}"
    );
}

#[test]
fn a_failed_validation_reaches_the_user_from_the_inbox() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let project = &added.project.id;

    apply(
        &store,
        project,
        "task_proposed:t-1",
        1_000,
        proposed("t-1", "Wire the store"),
    );
    apply(&store, project, "task_approved:t-1", 2_000, approved("t-1"));
    apply(
        &store,
        project,
        "worker_submitted:t-1",
        3_000,
        FactKind::WorkerSubmitted {
            task: TaskId::new("t-1"),
            commit: CommitId::new("abc123"),
        },
    );
    apply(
        &store,
        project,
        "validation_finished:t-1",
        4_000,
        FactKind::ValidationFinished {
            task: TaskId::new("t-1"),
            command: "cargo test".to_string(),
            commit: CommitId::new("abc123"),
            exit_code: 101,
            duration: std::time::Duration::from_millis(1_200),
            output_tail: "boom".to_string(),
        },
    );

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    let user = section(&payload, "For the user");
    assert!(
        user.contains("validation `cargo test` at `abc123` exited 101"),
        "a failure is a decision, got\n{payload}"
    );
    assert!(
        user.contains("(failed)"),
        "the entry reports where the task stands now, got\n{payload}"
    );
}

#[test]
fn a_poll_is_not_a_fact_the_coordinator_reads() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");

    apply(
        &store,
        &added.project.id,
        "polled:1",
        1_000,
        FactKind::Polled,
    );
    apply(
        &store,
        &added.project.id,
        "polled:2",
        2_000,
        FactKind::Polled,
    );

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    assert_eq!(
        payload, "# Inbox\n\nNo facts since your last turn.\n",
        "a poll observes nothing, so it is not something to act on, got\n{payload}"
    );
    assert!(
        store.inbox_cursor(&added.project.id).expect("cursor") > 0,
        "the cursor still clears the polls it skipped"
    );
}

#[test]
fn a_question_answered_since_the_fact_was_recorded_needs_nobody() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let project = &added.project.id;

    apply(
        &store,
        project,
        "task_proposed:t-1",
        1_000,
        proposed("t-1", "Wire the store"),
    );
    apply(&store, project, "task_approved:t-1", 2_000, approved("t-1"));
    apply(
        &store,
        project,
        "question_asked:t-1:0",
        3_000,
        asked("t-1", "Which store?", true),
    );
    apply(
        &store,
        project,
        "question_answered:t-1:1",
        4_000,
        FactKind::QuestionAnswered {
            task: TaskId::new("t-1"),
            answer: "sqlite".to_string(),
            by: depot_core::AnsweredBy::Coordinator,
        },
    );

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    assert!(
        !payload.contains("For the user") && !payload.contains("For you"),
        "an answered question is nobody's work now, got\n{payload}"
    );
    assert!(
        section(&payload, "No action").contains("answered"),
        "the answer still shows in the record of the turn, got\n{payload}"
    );
}

#[test]
fn a_merge_that_held_the_task_reaches_the_user_from_the_inbox() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let project = &added.project.id;

    let mut delivered = support::full_task(project, "t-1");
    delivered.state = TaskState::PrOpen;
    delivered.dependencies = Vec::new();
    delivered.base_dependency = None;
    delivered.branch_head = Some(CommitId::new("aaa111"));
    delivered.merge_refused = None;
    delivered.validations = vec![ValidationRecord {
        command: "cargo test".to_string(),
        commit: CommitId::new("aaa111"),
        exit_code: 0,
        duration: std::time::Duration::from_millis(1),
        output_tail: "ok".to_string(),
    }];
    store
        .put_task(&delivered)
        .expect("the delivered task is stored");

    let mut held = delivered.clone();
    held.id = TaskId::new("t-2");
    held.title = "task t-2".to_string();
    store.put_task(&held).expect("the held task is stored");

    apply(
        &store,
        project,
        "pull_request_merged:t-1:aaa111",
        1_000,
        FactKind::PullRequestMerged {
            task: TaskId::new("t-1"),
            commit: CommitId::new("aaa111"),
        },
    );
    apply(
        &store,
        project,
        "pull_request_merged:t-2:bbb222",
        2_000,
        FactKind::PullRequestMerged {
            task: TaskId::new("t-2"),
            commit: CommitId::new("bbb222"),
        },
    );

    assert_eq!(
        store
            .task(project, &TaskId::new("t-1"))
            .expect("t-1")
            .expect("t-1 exists")
            .state,
        TaskState::Landed
    );
    assert_eq!(
        store
            .task(project, &TaskId::new("t-2"))
            .expect("t-2")
            .expect("t-2 exists")
            .state,
        TaskState::Failed
    );

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    let user = section(&payload, "For the user");
    assert!(
        user.contains("`t-2`"),
        "a merge of a revision depot never validated needs a person, got\n{payload}"
    );
    assert!(
        !user.contains("`t-1`"),
        "a landing needs nobody, got\n{payload}"
    );
    assert!(
        section(&payload, "No action").contains("`t-1`"),
        "a landing is the daemon's own work, got\n{payload}"
    );
}

#[test]
fn a_refused_auto_merge_reaches_the_user_from_the_inbox() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let project = &added.project.id;

    let mut task = support::full_task(project, "t-1");
    task.state = TaskState::PrOpen;
    store.put_task(&task).expect("the task is stored");

    apply(
        &store,
        project,
        "pull_request_merge_refused:t-1:ccc333:bbb222:passing",
        1_000,
        FactKind::PullRequestMergeRefused {
            task: TaskId::new("t-1"),
            commit: CommitId::new("ccc333"),
            base: CommitId::new("bbb222"),
            checks: Checks::Passing,
            reason: "GitHub refused to merge and answered 409: {\"message\":\"Head branch was modified\"}"
                .to_string(),
        },
    );

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    let user = section(&payload, "For the user");
    assert!(
        user.contains("the forge refused to merge pull request #7"),
        "a refusal depot could not carry out is the user's to see, got\n{payload}"
    );
    assert!(
        user.contains("Head branch was modified"),
        "the refusal carries the reason the forge gave, got\n{payload}"
    );
}

#[test]
fn a_long_no_action_section_collapses_behind_a_count_and_keeps_the_user_first() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let project = &added.project.id;

    apply(
        &store,
        project,
        "task_proposed:t-1",
        1_000,
        proposed("t-1", "Wire the store"),
    );
    apply(
        &store,
        project,
        "validation_finished:t-1",
        2_000,
        FactKind::ValidationFinished {
            task: TaskId::new("t-1"),
            command: "cargo test".to_string(),
            commit: CommitId::new("bad000"),
            exit_code: 101,
            duration: std::time::Duration::from_millis(1),
            output_tail: "boom".to_string(),
        },
    );
    let mut task = support::full_task(project, "t-1");
    task.state = TaskState::Failed;
    store.put_task(&task).expect("the task is stored");

    for index in 0..15_u64 {
        apply(
            &store,
            project,
            &format!("polled-fact:{index}"),
            60_000 + index * 60_000,
            FactKind::WorkerTurnStarted {
                task: TaskId::new("t-1"),
                session: SessionId::new(format!("s{index}")),
            },
        );
    }

    let payload = read_inbox(&fixture.home, Some("example")).expect("inbox");

    let nothing_start = payload.find("\n## No action").expect("a no-action section");
    assert!(
        payload[..nothing_start].contains("## For the user (1)"),
        "the user's facts come before any bulk, got\n{payload}"
    );

    let nothing = section(&payload, "No action");
    assert_eq!(
        nothing.lines().count(),
        3,
        "the heading, the summary line, the filing and the collapsed run, got\n{payload}"
    );
    assert!(
        nothing.contains("(15 times through 1970-01-01T00:15:00Z)"),
        "the run of identical facts collapses into one counted line, got\n{payload}"
    );
    assert!(
        !nothing.contains("1970-01-01T00:05:00Z"),
        "the middle of the run is not printed, got\n{payload}"
    );
    assert!(
        nothing.contains("1970-01-01T00:15:00Z"),
        "the end of the run still shows, got\n{payload}"
    );
}

#[test]
fn consecutive_identical_entries_render_as_one_counted_line() {
    use depot_core::Timestamp;
    use depotd::{InboxEntry, Need, render_inbox};

    let mut entries: Vec<InboxEntry> = (0..3)
        .map(|millis| InboxEntry {
            at: Timestamp::from_millis(millis * 60_000),
            task: None,
            line: "the worker is live".to_owned(),
            need: Need::Nothing,
        })
        .collect();
    entries.push(InboxEntry {
        at: Timestamp::from_millis(4 * 60_000),
        task: None,
        line: "the worker is gone".to_owned(),
        need: Need::Nothing,
    });

    let rendered = render_inbox(&entries);

    assert_eq!(
        rendered.matches("the worker is live").count(),
        1,
        "the run of identical entries collapses, got\n{rendered}"
    );
    assert!(
        rendered.contains(
            "the worker is live at 1970-01-01T00:00:00Z (3 times through 1970-01-01T00:02:00Z)"
        ),
        "the collapsed line carries the count and the span, got\n{rendered}"
    );
    assert!(
        rendered.contains("the worker is gone at 1970-01-01T00:04:00Z"),
        "a single entry still renders alone, got\n{rendered}"
    );
}
