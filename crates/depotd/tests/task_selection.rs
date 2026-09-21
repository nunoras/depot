mod support;

use depot_core::TaskState;
use depotd::{Store, stop_task};

fn store_task(fixture: &support::Fixture, slug: &str) {
    let added = support::register(fixture, slug);
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-5",
            TaskState::Proposed,
            1_000,
        ))
        .expect("stored");
}

#[test]
fn a_bare_id_that_two_projects_own_is_refused() {
    let fixture = support::fixture();
    store_task(&fixture, "left");
    store_task(&fixture, "right");

    let error = stop_task(&fixture.home, None, "t-5").expect_err("ambiguous bare id");
    assert!(
        error.to_string().contains("several projects"),
        "got {error}"
    );
}

#[test]
fn a_qualified_id_names_its_project_from_any_directory() {
    let fixture = support::fixture();
    store_task(&fixture, "left");
    store_task(&fixture, "right");

    let stopped = stop_task(&fixture.home, None, "right/t-5").expect("qualified id");
    assert_eq!(stopped.id.as_str(), "t-5");
    assert_eq!(stopped.state, TaskState::Cancelled);
}

#[test]
fn a_bare_id_with_one_owner_resolves_without_a_project_flag() {
    let fixture = support::fixture();
    store_task(&fixture, "left");

    let stopped = stop_task(&fixture.home, None, "t-5").expect("unique owner");
    assert_eq!(stopped.state, TaskState::Cancelled);
}
