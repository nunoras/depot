mod support;

use std::collections::BTreeMap;

use depot_core::TaskState;
use depotd::{Settings, Store};

#[test]
fn a_project_starts_one_task_at_a_time_by_default_even_with_global_headroom() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    fixture
        .home
        .write_settings(&Settings {
            concurrency: 4,
            ..Settings::default()
        })
        .expect("settings");
    let store = Store::open(&fixture.home).expect("store");

    let state = store.project_state(&added.project).expect("state");

    assert_eq!(state.limits.max_concurrent_tasks, 1);
}

#[test]
fn the_configured_per_project_cap_applies_under_the_global_cap() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    fixture
        .home
        .write_settings(&Settings {
            concurrency: 4,
            project_concurrency: BTreeMap::from([("example".to_string(), 3)]),
            ..Settings::default()
        })
        .expect("settings");
    let store = Store::open(&fixture.home).expect("store");

    let state = store.project_state(&added.project).expect("state");

    assert_eq!(state.limits.max_concurrent_tasks, 3);
}

#[test]
fn a_per_project_cap_above_the_global_cap_is_clamped() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    fixture
        .home
        .write_settings(&Settings {
            concurrency: 2,
            project_concurrency: BTreeMap::from([("example".to_string(), 8)]),
            ..Settings::default()
        })
        .expect("settings");
    let store = Store::open(&fixture.home).expect("store");

    let state = store.project_state(&added.project).expect("state");

    assert_eq!(state.limits.max_concurrent_tasks, 2);
}

#[test]
fn other_projects_in_flight_tasks_shrink_the_free_global_slots() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    fixture
        .home
        .write_settings(&Settings {
            concurrency: 2,
            project_concurrency: BTreeMap::from([("first".to_string(), 2)]),
            ..Settings::default()
        })
        .expect("settings");
    let store = Store::open(&fixture.home).expect("store");
    for id in ["t-1", "t-2"] {
        store
            .put_task(&support::simple_task(
                &first.project.id,
                id,
                TaskState::Running,
                1,
            ))
            .expect("stored");
    }

    let busy = store.project_state(&first.project).expect("busy state");
    let starved = store.project_state(&second.project).expect("starved state");

    assert_eq!(busy.limits.max_concurrent_tasks, 2);
    assert_eq!(starved.limits.max_concurrent_tasks, 0);
}

#[test]
fn finished_tasks_elsewhere_release_the_global_slots_back() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    fixture
        .home
        .write_settings(&Settings {
            concurrency: 2,
            ..Settings::default()
        })
        .expect("settings");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &first.project.id,
            "t-1",
            TaskState::Running,
            1,
        ))
        .expect("stored");
    store
        .put_task(&support::simple_task(
            &first.project.id,
            "t-2",
            TaskState::Landed,
            2,
        ))
        .expect("stored");

    let state = store.project_state(&second.project).expect("state");

    assert_eq!(state.limits.max_concurrent_tasks, 1);
}

#[test]
fn waiting_on_a_question_still_counts_as_an_in_flight_slot() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    fixture
        .home
        .write_settings(&Settings {
            concurrency: 1,
            ..Settings::default()
        })
        .expect("settings");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &first.project.id,
            "t-1",
            TaskState::WaitingOnQuestion,
            1,
        ))
        .expect("stored");

    let state = store.project_state(&second.project).expect("state");

    assert_eq!(state.limits.max_concurrent_tasks, 0);
}
