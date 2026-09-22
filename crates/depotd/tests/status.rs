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
        "Needs you - waiting on an answer",
        "Held - awaiting approval",
        "Running",
        "Validated",
    ] {
        assert!(
            rendered.contains(&format!("## {label} (1)")),
            "missing {label} in\n{rendered}"
        );
    }
    for label in ["Landed", "Failed", "Cancelled"] {
        assert!(
            !rendered.contains(&format!("## {label}")),
            "terminal {label} leaked into the default status\n{rendered}"
        );
    }
    assert!(rendered.contains("1 landed · 1 failed · 1 cancelled"));
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
    assert!(!rendered.contains("## Landed"));
    assert!(rendered.contains("1 landed"));
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
fn a_failed_task_is_history_once_a_live_task_depends_on_it() {
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
    assert!(!faded.contains("## Failed"));
    assert!(faded.contains("1 failed"));
    assert!(history.contains("## Failed (1)"));
}

#[test]
fn an_acknowledged_failed_task_is_history_too() {
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

    assert!(!faded.contains("## Failed"));
    assert!(history.contains("## Failed (1)"));
}

#[test]
fn a_cancelled_task_is_history_without_replacement_or_acknowledgement() {
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

    assert!(!rendered.contains("## Cancelled"));
    assert!(rendered.contains("1 cancelled"));
}

#[test]
fn running_tasks_carry_attempt_age_and_observed_liveness() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");

    let mut sessionless =
        support::simple_task(&added.project.id, "t-live", TaskState::Running, 1_000);
    sessionless.attempts.push(depot_core::Attempt {
        last_seen_at: None,
        session: None,
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::Unknown,
        base_merge: false,
    });

    let mut unseen = support::simple_task(&added.project.id, "t-unseen", TaskState::Running, 1_000);
    unseen.attempts.push(depot_core::Attempt {
        last_seen_at: None,
        session: Some(depot_core::SessionId::new("s-1")),
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::Unknown,
        base_merge: false,
    });

    let mut alive = support::simple_task(&added.project.id, "t-alive", TaskState::Running, 1_000);
    alive.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("s-2")),
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        base_merge: false,
        last_seen_at: Some(depot_core::Timestamp::from_millis(1_000 + 14 * 60 * 1000)),
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
        rendered.contains("- attempt: in_flight 15m, session `s-2` last seen 1m"),
        "{rendered}"
    );
}

#[test]
fn a_fresh_observation_reads_as_last_seen() {
    let rendered = rendered_attempt_line(40_000);
    assert!(
        rendered.contains("session `s-1` last seen 40s"),
        "{rendered}"
    );
    assert!(!rendered.contains("Needs you - unobserved"), "{rendered}");
}

#[test]
fn a_stale_observation_reads_as_unobserved_and_needs_you() {
    let rendered = rendered_attempt_line(2 * 60 * 60 * 1000 + 41 * 60 * 1000);
    assert!(
        rendered.contains("session `s-1` unobserved for 2h41m"),
        "{rendered}"
    );
    assert!(
        rendered.contains("## Needs you - unobserved sessions (1)"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "- `t-1` **task t-1** - session `s-1` unobserved for 2h41m; check whether the worker is stuck"
        ),
        "{rendered}"
    );
}

#[test]
fn a_never_observed_session_still_reads_as_not_yet_seen() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let mut task = support::simple_task(&added.project.id, "t-1", TaskState::Running, 1_000);
    task.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("s-1")),
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        base_merge: false,
        last_seen_at: None,
    });
    store.put_task(&task).expect("stored");

    let rendered = render_status_at(
        &fixture.home,
        &StatusSelection::All,
        false,
        depot_core::Timestamp::from_millis(1_000 + 9 * 60 * 60 * 1000),
    )
    .expect("status");

    assert!(
        rendered.contains("session `s-1` not yet seen alive"),
        "{rendered}"
    );
    assert!(!rendered.contains("Needs you - unobserved"), "{rendered}");
}

fn rendered_attempt_line(seen_age_millis: u64) -> String {
    let now = 1_000 + 9 * 60 * 60 * 1000;
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let mut task = support::simple_task(&added.project.id, "t-1", TaskState::Running, 1_000);
    task.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("s-1")),
        profile: depot_core::ProfileId::new("pi"),
        worktree: Some("lease".to_string().into()),
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        base_merge: false,
        last_seen_at: Some(depot_core::Timestamp::from_millis(now - seen_age_millis)),
    });
    store.put_task(&task).expect("stored");

    render_status_at(
        &fixture.home,
        &StatusSelection::All,
        false,
        depot_core::Timestamp::from_millis(now),
    )
    .expect("status")
}

#[test]
fn the_written_checklist_stays_free_of_observation_time() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let mut task = support::simple_task(&added.project.id, "t-1", TaskState::Running, 1_000);
    task.attempts.push(depot_core::Attempt {
        last_seen_at: None,
        session: None,
        profile: depot_core::ProfileId::new("pi"),
        worktree: None,
        started_at: depot_core::Timestamp::from_millis(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::Unknown,
        base_merge: false,
    });
    store.put_task(&task).expect("stored");

    let checklist = render_checklist(&store.project_state(&added.project).expect("state"), false);

    assert!(
        !checklist.contains("attempt:"),
        "the checklist must not carry time-shaped observations: {checklist}"
    );
}

#[test]
fn status_warns_when_a_live_daemon_does_not_cover_a_project_with_open_work() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &second.project.id,
            "t-1",
            TaskState::Approved,
            1,
        ))
        .expect("stored");

    let lock = depotd::InstanceLock::acquire(&fixture.home).expect("lock");
    lock.record_scope(std::slice::from_ref(&first.project))
        .expect("scope");
    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");
    assert!(
        rendered.contains("no daemon is driving this project"),
        "missing warning in\n{rendered}"
    );
}

#[test]
fn status_stays_quiet_when_a_live_daemon_covers_the_project() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &second.project.id,
            "t-1",
            TaskState::Approved,
            1,
        ))
        .expect("stored");

    let lock = depotd::InstanceLock::acquire(&fixture.home).expect("lock");
    lock.record_scope(&[first.project.clone(), second.project.clone()])
        .expect("scope");
    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");
    assert!(
        !rendered.contains("no daemon is driving this project"),
        "unexpected warning in\n{rendered}"
    );
}

#[test]
fn status_treats_a_stale_heartbeat_as_no_coverage() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &second.project.id,
            "t-1",
            TaskState::Approved,
            1,
        ))
        .expect("stored");

    let lock = depotd::InstanceLock::acquire(&fixture.home).expect("lock");
    lock.record_scope(&[first.project.clone(), second.project.clone()])
        .expect("scope");
    let path = fixture.home.root().join(depotd::DAEMON_SCOPE_FILE_NAME);
    let mut scope: depotd::DaemonScope =
        serde_json::from_slice(&std::fs::read(&path).expect("lock record")).expect("parsed scope");
    scope.heartbeat_millis -= 10 * 60 * 1000;
    std::fs::write(&path, serde_json::to_vec(&scope).expect("encoded scope")).expect("rewritten");
    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");
    assert!(
        rendered.contains("no daemon is driving this project"),
        "missing warning in\n{rendered}"
    );
}

#[test]
fn status_warns_when_the_running_daemon_was_built_from_another_commit() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let lock = depotd::InstanceLock::acquire(&fixture.home).expect("lock");
    lock.record_scope(std::slice::from_ref(&added.project))
        .expect("scope");

    let path = fixture.home.root().join(depotd::DAEMON_SCOPE_FILE_NAME);
    let mut scope: depotd::DaemonScope =
        serde_json::from_slice(&std::fs::read(&path).expect("scope record")).expect("parsed scope");
    scope.build_id = "0ldbu11d".to_string();
    std::fs::write(&path, serde_json::to_vec(&scope).expect("encoded scope")).expect("rewritten");

    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");
    assert!(
        rendered.contains("was built from commit 0ldbu11d"),
        "missing build warning in\n{rendered}"
    );
    assert!(
        rendered.contains(depotd::BUILD_ID),
        "the warning names this client build in\n{rendered}"
    );
}

#[test]
fn status_stays_quiet_when_the_running_daemon_matches_this_build() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let lock = depotd::InstanceLock::acquire(&fixture.home).expect("lock");
    lock.record_scope(std::slice::from_ref(&added.project))
        .expect("scope");

    let rendered = render_status(&fixture.home, &StatusSelection::All, false).expect("status");
    assert!(
        !rendered.contains("was built from commit"),
        "unexpected build warning in\n{rendered}"
    );
}

#[test]
fn refresh_heartbeat_moves_the_recorded_heartbeat_past_the_start() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");

    let lock = depotd::InstanceLock::acquire(&fixture.home).expect("lock");
    lock.record_scope(std::slice::from_ref(&first.project))
        .expect("scope");
    std::thread::sleep(std::time::Duration::from_millis(30));
    lock.refresh_heartbeat().expect("refresh");

    let path = fixture.home.root().join(depotd::DAEMON_SCOPE_FILE_NAME);
    let scope: depotd::DaemonScope =
        serde_json::from_slice(&std::fs::read(&path).expect("scope record")).expect("parsed scope");
    assert!(
        scope.heartbeat_millis > scope.started_at_millis,
        "heartbeat {} must move past started_at {}",
        scope.heartbeat_millis,
        scope.started_at_millis
    );
}
