mod support;

use depot_core::{Fact, FactKind, TaskId, TaskState, Timestamp};
use depotd::{StatusSelection, Store, render_checklist, render_status, render_status_at};

#[test]
fn status_shows_held_running_blocked_waiting_and_validated_tasks_distinctly() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    for (index, task_state) in support::ALL_STATES.into_iter().enumerate() {
        let id = format!("t-{index}");
        store
            .put_task(&support::simple_task(
                &added.project.id,
                &id,
                task_state,
                1_700_000_000_000 + index as u64,
            ))
            .expect("stored");
    }

    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");

    for label in [
        "Held - awaiting approval",
        "Running",
        "Blocked - needs a person",
        "Waiting on a question",
        "Validated",
    ] {
        assert!(
            rendered.contains(&format!("## {label} (1)")),
            "missing {label} in\n{rendered}"
        );
    }
    assert!(rendered.contains(added.project.id.as_str()));
}

#[test]
fn status_can_be_narrowed_to_one_project() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &first.project.id,
            "t-1",
            TaskState::Landed,
            1,
        ))
        .expect("stored");

    let rendered = render_status(
        &fixture.home,
        &StatusSelection::Project(first.project.slug.clone()),
        false,
    )
    .expect("status");

    assert!(rendered.contains(first.project.id.as_str()));
    assert!(!rendered.contains(second.project.id.as_str()));
    assert!(rendered.contains("## Landed (1)"));
}

#[test]
fn status_accepts_a_project_path_as_the_name() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "example");
    let added = support::register(&fixture, "example");

    let rendered = render_status(
        &fixture.home,
        &StatusSelection::Project(directory.to_str().unwrap().to_string()),
        false,
    )
    .expect("status");

    assert!(rendered.contains(added.project.id.as_str()));
}

#[test]
fn status_over_every_project_lists_both() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");

    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");

    assert!(rendered.contains(first.project.id.as_str()));
    assert!(rendered.contains(second.project.id.as_str()));
}

#[test]
fn status_over_no_projects_says_so() {
    let fixture = support::fixture();

    assert_eq!(
        render_status(&fixture.home, &StatusSelection::All, false).expect("status"),
        "No projects registered.\n"
    );
}

#[test]
fn status_names_a_project_it_cannot_find() {
    let fixture = support::fixture();
    support::register(&fixture, "example");

    let error = render_status(
        &fixture.home,
        &StatusSelection::Project("elsewhere".to_string()),
        false,
    )
    .expect_err("unknown project");

    assert!(error.to_string().contains("no project matches `elsewhere`"));
}

#[test]
fn status_from_a_directory_outside_every_project_says_so() {
    let fixture = support::fixture();
    support::register(&fixture, "first");
    support::register(&fixture, "second");

    let error = render_status(&fixture.home, &StatusSelection::CurrentDirectory, false)
        .expect_err("no project owns the depot checkout");
    let message = error.to_string();

    assert!(message.contains("no project matches this directory"));
    assert!(message.contains("`first`"));
    assert!(message.contains("`second`"));
}

#[test]
fn status_from_outside_a_sole_registered_project_is_not_a_guess() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "only");

    let error = render_status(&fixture.home, &StatusSelection::CurrentDirectory, false)
        .expect_err("a bare status outside the project must not invent a match");
    let message = error.to_string();

    assert!(message.contains("no project matches this directory"));
    assert!(
        message.contains(&format!("`{}`", added.project.slug)),
        "the refusal should name the registered project, got {message}"
    );
    assert!(
        message.contains("--project") && message.contains("--all"),
        "the refusal should name the explicit selection flags, got {message}"
    );
}

#[test]
fn a_failed_task_fades_once_a_live_task_depends_on_it() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let mut failed = support::simple_task(&added.project.id, "t-1", TaskState::Failed, 1);
    failed.dependencies = Vec::new();
    store.put_task(&failed).expect("stored");
    let mut replacement = support::simple_task(&added.project.id, "t-2", TaskState::Running, 2);
    replacement.base_dependency = Some(TaskId::new("t-1"));
    store.put_task(&replacement).expect("stored");

    let faded = render_status(&fixture.home, &StatusSelection::All, false).expect("status");
    let history = render_status(&fixture.home, &StatusSelection::All, true).expect("history");

    assert!(faded.contains("## Running (1)"));
    assert!(!faded.contains("Blocked - needs a person"));
    assert!(faded.contains("1 faded task hidden"));
    assert!(history.contains("## Blocked - needs a person (1)"));
}

#[test]
fn an_acknowledged_failed_task_fades_and_history_still_shows_it() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Failed,
            1,
        ))
        .expect("stored");
    let fact = Fact {
        at: Timestamp::from_millis(2_000),
        kind: FactKind::TaskAcknowledged {
            task: TaskId::new("t-1"),
        },
    };
    store
        .apply_fact(&added.project, "task_acknowledged:t-1", &fact)
        .expect("applied");

    let faded = render_status(&fixture.home, &StatusSelection::All, false).expect("status");
    let history = render_status(&fixture.home, &StatusSelection::All, true).expect("history");

    assert!(!faded.contains("Blocked - needs a person"));
    assert!(history.contains("## Blocked - needs a person (1)"));
}

#[test]
fn a_cancelled_task_without_replacement_or_acknowledgement_stays_visible() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Cancelled,
            1,
        ))
        .expect("stored");

    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");

    assert!(rendered.contains("## Cancelled (1)"));
    assert!(!rendered.contains("faded"));
}

#[test]
fn running_tasks_carry_attempt_age_and_observed_liveness() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");

    let mut sessionless =
        support::simple_task(&added.project.id, "t-live", TaskState::Running, 1_000);
    sessionless.attempts.push(depot_core::Attempt {
        session: None,
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::Unknown,
        rebase: false,
    });

    let mut unseen = support::simple_task(&added.project.id, "t-unseen", TaskState::Running, 1_000);
    unseen.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("s-1")),
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::Unknown,
        rebase: false,
    });

    let mut alive = support::simple_task(&added.project.id, "t-alive", TaskState::Running, 1_000);
    alive.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("s-2")),
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        rebase: false,
    });

    for task in [&sessionless, &unseen, &alive] {
        store.put_task(task).expect("stored");
    }

    let rendered = render_status_at(
        &fixture.home,
        &StatusSelection::All,
        false,
        depot_core::Timestamp::from_millis(1_000 + 15 * 60 * 1000),
    )
    .expect("status");

    assert!(
        rendered.contains("- attempt: stalled - in_flight 15m, no worker session"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- attempt: in_flight 15m, session `s-1` not yet seen alive"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- attempt: in_flight 15m, session `s-2` alive"),
        "{rendered}"
    );
}

#[test]
fn the_written_checklist_stays_free_of_observation_time() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let mut task = support::simple_task(&added.project.id, "t-1", TaskState::Running, 1_000);
    task.attempts.push(depot_core::Attempt {
        session: None,
        profile: depot_core::ProfileId::new("pi"),
        worktree: None,
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::Unknown,
        rebase: false,
    });
    store.put_task(&task).expect("stored");

    let checklist = render_checklist(&store.project_state(&added.project).expect("state"), false);

    assert!(
        !checklist.contains("attempt:"),
        "the checklist must not carry time-shaped observations: {checklist}"
    );
}
