mod support;

use depot_core::TaskState;
use depotd::{StatusSelection, Store, render_status};

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

    let rendered = render_status(&fixture.home, &StatusSelection::All).expect("status");

    for label in [
        "Needs you - waiting on an answer",
        "Held - awaiting approval",
        "Running",
        "Failed",
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
    )
    .expect("status");

    assert!(rendered.contains(added.project.id.as_str()));
}

#[test]
fn status_over_every_project_lists_both() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");

    let rendered = render_status(&fixture.home, &StatusSelection::All).expect("status");

    assert!(rendered.contains(first.project.id.as_str()));
    assert!(rendered.contains(second.project.id.as_str()));
}

#[test]
fn status_over_no_projects_says_so() {
    let fixture = support::fixture();

    assert_eq!(
        render_status(&fixture.home, &StatusSelection::All).expect("status"),
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
    )
    .expect_err("unknown project");

    assert!(error.to_string().contains("no project matches `elsewhere`"));
}

#[test]
fn status_from_a_directory_outside_every_project_says_so() {
    let fixture = support::fixture();
    support::register(&fixture, "first");
    support::register(&fixture, "second");

    let error = render_status(&fixture.home, &StatusSelection::CurrentDirectory)
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

    let error = render_status(&fixture.home, &StatusSelection::CurrentDirectory)
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
