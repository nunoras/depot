mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use depot_core::{
    Checks, CommitId, Dependency, Limits, Link, ProfileId, ProjectId, ProjectState, Question, Role,
    TaskId, TaskState, Timestamp, ValidationRecord,
};
use depotd::{CHECKLIST_FILE_NAME, Store, format_timestamp, render_checklist};

#[test]
fn the_checklist_render_is_byte_identical_for_identical_state() {
    let state = support::varied_state();

    let first = render_checklist(&state);
    let second = render_checklist(&state);

    assert_eq!(first.as_bytes(), second.as_bytes());
}

#[test]
fn the_checklist_render_is_byte_identical_for_equal_state_built_twice() {
    let first = support::varied_state();
    let second = support::varied_state();
    assert_eq!(first, second);

    assert_eq!(
        render_checklist(&first).as_bytes(),
        render_checklist(&second).as_bytes()
    );
}

#[test]
fn the_checklist_render_is_byte_identical_whatever_order_records_arrived_in() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let tasks: Vec<_> = support::ALL_STATES
        .into_iter()
        .enumerate()
        .map(|(index, task_state)| {
            let id = format!("t-{index}");
            support::simple_task(
                &added.project.id,
                &id,
                task_state,
                1_700_000_000_000 + index as u64,
            )
        })
        .collect();

    for task in &tasks {
        store.put_task(task).expect("stored");
    }
    let first = render_checklist(&store.project_state(&added.project).expect("state"));

    for task in tasks.iter().rev() {
        store.put_task(task).expect("stored again");
    }
    let second = render_checklist(&store.project_state(&added.project).expect("state"));

    assert_eq!(first.as_bytes(), second.as_bytes());
}

#[test]
fn the_checklist_shows_every_task_state_distinctly() {
    let rendered = render_checklist(&support::varied_state());

    for label in [
        "Held - awaiting approval",
        "Approved - queued",
        "Running",
        "Waiting on a question",
        "Validating",
        "Validated",
        "Pull request open",
        "Landed",
        "Blocked - needs a person",
        "Cancelled",
    ] {
        assert!(
            rendered.contains(&format!("## {label} (2)")),
            "missing section {label} in\n{rendered}"
        );
    }
}

#[test]
fn the_checklist_states_what_each_task_waits_on() {
    let project = ProjectId::new("example/project");
    let mut tasks = BTreeMap::new();

    let proposed = support::simple_task(&project, "t-proposed", TaskState::Proposed, 1);
    tasks.insert(proposed.id.clone(), proposed);

    let mut queued = support::simple_task(&project, "t-queued", TaskState::Approved, 2);
    queued.dependencies = vec![Dependency {
        task: TaskId::new("t-prerequisite"),
        commit: CommitId::new("aaa111"),
    }];
    tasks.insert(queued.id.clone(), queued);

    let mut asking = support::simple_task(&project, "t-asking", TaskState::WaitingOnQuestion, 3);
    asking.questions = vec![Question {
        text: "Which base branch?".to_string(),
        asked_at: Timestamp::from_millis(3),
        answer: None,
    }];
    tasks.insert(asking.id.clone(), asking);

    let mut reviewed = support::simple_task(&project, "t-reviewed", TaskState::Validated, 4);
    reviewed.branch_head = Some(CommitId::new("abc1234"));
    tasks.insert(reviewed.id.clone(), reviewed);

    let mut opened = support::simple_task(&project, "t-opened", TaskState::PrOpen, 5);
    opened.links = vec![Link::PullRequest {
        number: 41,
        url: "https://example.test/pull/41".to_string(),
        checks: Checks::Pending,
    }];
    tasks.insert(opened.id.clone(), opened);

    let mut failed = support::simple_task(&project, "t-failed", TaskState::Failed, 6);
    failed.validations = vec![ValidationRecord {
        command: "cargo test".to_string(),
        commit: CommitId::new("ddd444"),
        exit_code: 101,
        duration: Duration::from_millis(900),
        output_tail: "3 tests failed".to_string(),
    }];
    tasks.insert(failed.id.clone(), failed);

    let rendered = render_checklist(&ProjectState {
        project,
        tasks,
        profiles: BTreeMap::from([(Role::Build, ProfileId::new("glm-5.3"))]),
        fallback_profiles: Vec::new(),
        limits: Limits::default(),
        always_relay_questions: false,
    });

    for expected in [
        "waits on: approval",
        "waits on: dependency `t-prerequisite` at `aaa111` to be validated",
        "waits on: an answer to \"Which base branch?\"",
        "waits on: a pull request",
        "waits on: checks on pull request #41",
        "waits on: a person",
        "last validation: `cargo test` at `ddd444` exited 101",
        "pull request: [#41](https://example.test/pull/41) - checks pending",
        "branch head: `abc1234`",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected:?} in\n{rendered}"
        );
    }
}

#[test]
fn the_checklist_render_changes_when_the_state_changes() {
    let before = support::varied_state();
    let mut after = before.clone();
    let id = TaskId::new("t-0-a");
    after.tasks.get_mut(&id).expect("task").state = TaskState::Landed;

    assert_ne!(render_checklist(&before), render_checklist(&after));
}

#[test]
fn an_empty_project_renders_a_checklist_with_no_tasks() {
    let rendered = render_checklist(&ProjectState {
        project: ProjectId::new("example/project"),
        tasks: BTreeMap::new(),
        profiles: BTreeMap::new(),
        fallback_profiles: Vec::new(),
        limits: Limits::default(),
        always_relay_questions: false,
    });

    assert_eq!(
        rendered,
        "# Checklist\n\nProject: example/project\n\n\
         Rendered from depot records. Hand edits are overwritten.\n\n\
         No tasks yet.\n"
    );
}

#[test]
fn putting_a_task_refreshes_the_on_disk_checklist() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let checklist = fixture
        .home
        .project_home(&added.project.slug)
        .checklist_path();
    let before = std::fs::read_to_string(&checklist).expect("empty checklist");
    assert!(
        before.contains("No tasks yet."),
        "registration should start from an empty checklist, got\n{before}"
    );

    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Running,
            1_700_000_000_000,
        ))
        .expect("stored");

    let after = std::fs::read_to_string(&checklist).expect("refreshed checklist");
    assert!(
        after.contains("## Running (1)"),
        "put_task must rewrite {CHECKLIST_FILE_NAME}, got\n{after}"
    );
    assert!(
        after.contains("`t-1`"),
        "the refreshed checklist must name the task, got\n{after}"
    );
    assert!(
        !after.contains("No tasks yet."),
        "the empty checklist must not linger after a task write, got\n{after}"
    );
}

#[test]
fn a_broken_project_config_refuses_put_task_without_writing() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "broken");
    let added = support::register(&fixture, "broken");
    let checklist = fixture
        .home
        .project_home(&added.project.slug)
        .checklist_path();
    let before = std::fs::read_to_string(&checklist).expect("empty checklist");
    std::fs::write(
        directory.join(depotd::PROJECT_CONFIG_FILE_NAME),
        "base_branch = \"main\"\n\n[profiles]\nbuild = \"\"\n",
    )
    .expect("broken project config");

    let store = Store::open(&fixture.home).expect("store");
    let error = store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Running,
            1_700_000_000_000,
        ))
        .expect_err("a broken project config must refuse the write");

    assert!(
        error.to_string().contains("empty profile"),
        "the refusal should name the config problem, got {error}"
    );
    assert!(
        store
            .task(&added.project.id, &TaskId::new("t-1"))
            .expect("read")
            .is_none(),
        "a failed put_task must not leave a durable task row"
    );
    assert_eq!(
        std::fs::read_to_string(&checklist).expect("checklist"),
        before,
        "a failed put_task must leave the checklist untouched"
    );
}

#[test]
fn a_missing_project_directory_refuses_put_task_without_writing() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "gone");
    let added = support::register(&fixture, "gone");
    let checklist = fixture
        .home
        .project_home(&added.project.slug)
        .checklist_path();
    let before = std::fs::read_to_string(&checklist).expect("empty checklist");
    std::fs::remove_dir_all(&directory).expect("delete the project path");

    let store = Store::open(&fixture.home).expect("store");
    let error = store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Running,
            1_700_000_000_000,
        ))
        .expect_err("a missing project path must refuse the write");

    assert!(
        error.to_string().contains("does not exist"),
        "the refusal should name the missing path, got {error}"
    );
    assert!(
        store
            .task(&added.project.id, &TaskId::new("t-1"))
            .expect("read")
            .is_none(),
        "a failed put_task must not leave a durable task row"
    );
    assert_eq!(
        std::fs::read_to_string(&checklist).expect("checklist"),
        before,
        "a failed put_task must leave the checklist untouched"
    );
}

#[test]
fn a_timestamp_renders_as_utc_and_never_reads_the_clock() {
    assert_eq!(
        format_timestamp(Timestamp::from_millis(0)),
        "1970-01-01T00:00:00Z"
    );
    assert_eq!(
        format_timestamp(Timestamp::from_millis(1_700_000_000_000)),
        "2023-11-14T22:13:20Z"
    );
    assert_eq!(
        format_timestamp(Timestamp::from_millis(1_700_000_000_999)),
        "2023-11-14T22:13:20Z"
    );
}
