mod support;

use std::collections::BTreeMap;

use depot_core::{TaskId, TaskState};
use depotd::{DepotHome, ProfileSettings, Settings, Store, approve_tasks};

const CLAUDE_BUILD: &str = "base_branch = \"main\"\n\n\
                            [profiles]\n\
                            build = \"claude\"\n\n\
                            [validation]\n\
                            command = \"cargo test\"\n";

fn covering_settings(home: &DepotHome) {
    home.write_settings(&Settings {
        fallback_profiles: vec!["backup".to_string()],
        profiles: BTreeMap::from([(
            "backup".to_string(),
            ProfileSettings {
                harness: "pi".to_string(),
                model: "glm-5.3".to_string(),
                effort: "high".to_string(),
                account: String::new(),
            },
        )]),
        ..Settings::default()
    })
    .expect("settings");
}

#[test]
fn approving_refuses_when_no_role_profile_is_defined_in_machine_settings() {
    let fixture = support::fixture();
    covering_settings(&fixture.home);
    let added = support::register_with_config(&fixture, "example", CLAUDE_BUILD);
    fixture
        .home
        .write_settings(&Settings::default())
        .expect("settings");
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Proposed,
            1_000,
        ))
        .expect("seeded");

    let error = approve_tasks(&fixture.home, Some("example"), &["t-1".to_string()])
        .expect_err("the unmapped profile is refused at approve time");
    let message = error.to_string();
    assert!(message.contains("build"), "got {message}");
    assert!(message.contains("claude"), "got {message}");
    assert!(message.contains("machine-local"), "got {message}");

    let task = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(task.state, TaskState::Proposed);
}

#[test]
fn approving_passes_when_a_fallback_covers_the_role() {
    let fixture = support::fixture();
    covering_settings(&fixture.home);
    let added = support::register_with_config(&fixture, "example", CLAUDE_BUILD);
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Proposed,
            1_000,
        ))
        .expect("seeded");

    let approved = approve_tasks(&fixture.home, Some("example"), &["t-1".to_string()])
        .expect("the fallback covers the role");

    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0].state, TaskState::Running);
}

#[test]
fn adding_a_project_refuses_a_role_whose_profile_is_not_defined() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "example");
    std::fs::write(
        directory.join(depotd::PROJECT_CONFIG_FILE_NAME),
        CLAUDE_BUILD,
    )
    .expect("project config");

    let error = depotd::add_project(&fixture.home, directory.to_str().unwrap())
        .expect_err("the unmapped profile is refused at add time");

    assert!(error.to_string().contains("claude"), "got {error}");
}
