mod support;

use std::time::Duration;

use depot_core::{Fact, FactKind, TaskId, TaskState, Timestamp};
use depotd::{MIN_WAIT_POLL, Store, event_key, wait_for_task};

#[test]
fn a_settled_task_returns_at_once_with_its_state() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::PrOpen,
            1,
        ))
        .expect("stored");

    let waited = wait_for_task(&fixture.home, Some("example"), "t-1", None, MIN_WAIT_POLL)
        .expect("the wait returns");

    assert!(!waited.timed_out);
    assert_eq!(waited.task.state, TaskState::PrOpen);
}

#[test]
fn a_task_that_settles_during_the_wait_is_seen() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Running,
            1,
        ))
        .expect("stored");

    let home = fixture.home.clone();
    let project = added.project.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let store = Store::open(&home).expect("store");
        store
            .apply_fact(
                &project,
                &event_key(&["task_cancelled", "t-1"]),
                &Fact {
                    at: Timestamp::from_millis(2),
                    kind: FactKind::TaskCancelled {
                        task: TaskId::new("t-1"),
                    },
                },
            )
            .expect("the cancellation lands");
    });

    let waited = wait_for_task(&fixture.home, Some("example"), "t-1", None, MIN_WAIT_POLL)
        .expect("the wait returns");
    canceller.join().expect("the canceller finishes");

    assert!(!waited.timed_out);
    assert_eq!(waited.task.state, TaskState::Cancelled);
}

#[test]
fn a_wait_that_runs_out_reports_the_state_it_left_behind() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Validated,
            1,
        ))
        .expect("stored");

    let waited = wait_for_task(
        &fixture.home,
        Some("example"),
        "t-1",
        Some(Duration::from_millis(300)),
        MIN_WAIT_POLL,
    )
    .expect("the wait gives up rather than failing");

    assert!(waited.timed_out);
    assert_eq!(waited.task.state, TaskState::Validated);
}

#[test]
fn a_wait_does_not_move_the_inbox_cursor() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Landed,
            1,
        ))
        .expect("stored");
    let before = store.inbox_cursor(&added.project.id).expect("cursor");

    wait_for_task(&fixture.home, Some("example"), "t-1", None, MIN_WAIT_POLL).expect("the wait");

    assert_eq!(
        store.inbox_cursor(&added.project.id).expect("cursor"),
        before
    );
}

#[test]
fn a_wait_never_sleeps_less_than_its_minimum_poll() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Running,
            1,
        ))
        .expect("stored");

    let started = std::time::Instant::now();
    let waited = wait_for_task(
        &fixture.home,
        Some("example"),
        "t-1",
        Some(MIN_WAIT_POLL),
        Duration::ZERO,
    )
    .expect("the wait returns");

    assert!(waited.timed_out);
    assert!(
        started.elapsed() >= MIN_WAIT_POLL,
        "a zero poll must not become a busy spin, took {:?}",
        started.elapsed()
    );
}

#[test]
fn an_unknown_task_is_refused_before_any_waiting() {
    let fixture = support::fixture();
    support::register(&fixture, "example");

    let error = wait_for_task(&fixture.home, Some("example"), "t-404", None, MIN_WAIT_POLL)
        .expect_err("an unknown task fails fast");

    assert!(error.to_string().contains("t-404"), "got {error}");
}
