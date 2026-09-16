mod support;

use std::collections::BTreeMap;

use depot_core::{Limits, ProfileId, ProjectState, Role, TaskId};
use depotd::{PROJECT_CONFIG_FILE_NAME, ProjectConfig, Settings, Store, add_project, event_key};

#[test]
fn a_task_round_trips_through_the_store_with_every_field_intact() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let task = support::full_task(&added.project.id, "t-1");

    store.put_task(&task).expect("stored");

    let loaded = store
        .task(&task.project, &task.id)
        .expect("read")
        .expect("present");
    assert_eq!(loaded, task);
}

#[test]
fn rewriting_a_task_leaves_no_duplicate_children_behind() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let mut task = support::full_task(&added.project.id, "t-1");

    store.put_task(&task).expect("stored");
    task.questions.clear();
    task.attempts.clear();
    task.links.clear();
    store.put_task(&task).expect("restored");

    let loaded = store.task(&task.project, &task.id).unwrap().unwrap();
    assert_eq!(loaded, task);
}

#[test]
fn a_project_state_round_trips_through_the_configuration_split() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "example");
    std::fs::write(
        directory.join(PROJECT_CONFIG_FILE_NAME),
        "base_branch = \"main\"\n\n\
         [profiles]\n\
         plan = \"fable-5\"\n\
         build = \"glm-5.3\"\n\n\
         [validation]\n\
         command = \"cargo test\"\n\n\
         [pull_request]\n\
         base = \"main\"\n\
         auto_merge = false\n\n\
         [questions]\n\
         always_relay = true\n",
    )
    .expect("project config");

    let settings = Settings {
        concurrency: 7,
        fallback_profiles: vec!["gpt-5.5".to_string(), "sonnet".to_string()],
        ..Settings::default()
    };
    fixture.home.write_settings(&settings).expect("settings");

    let added =
        add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered");
    let store = Store::open(&fixture.home).expect("store");

    let first = support::full_task(&added.project.id, "t-1");
    let second = support::full_task(&added.project.id, "t-2");
    store.put_task(&first).expect("stored");
    store.put_task(&second).expect("stored");

    let expected = ProjectState {
        project: added.project.id.clone(),
        tasks: BTreeMap::from([(TaskId::new("t-1"), first), (TaskId::new("t-2"), second)]),
        profiles: BTreeMap::from([
            (Role::Plan, ProfileId::new("fable-5")),
            (Role::Build, ProfileId::new("glm-5.3")),
        ]),
        fallback_profiles: vec![ProfileId::new("gpt-5.5"), ProfileId::new("sonnet")],
        limits: Limits {
            max_concurrent_tasks: 7,
            ..Limits::default()
        },
        always_relay_questions: true,
    };

    assert_eq!(
        store.project_state(&added.project).expect("state"),
        expected
    );
}

#[test]
fn a_project_with_no_committed_config_falls_back_to_defaults() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let config = ProjectConfig::load(fixture.home.root().join("elsewhere").as_path())
        .expect("defaults for a missing file");
    assert_eq!(config, ProjectConfig::default());

    let store = Store::open(&fixture.home).expect("store");
    let state = store.project_state(&added.project).expect("state");
    assert!(state.profiles.is_empty());
    assert_eq!(state.limits.max_concurrent_tasks, 4);
    assert!(!state.always_relay_questions);
}

#[test]
fn the_event_journal_records_a_fact_once_and_replays_it_as_a_duplicate() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let fact = depot_core::Fact {
        at: depot_core::Timestamp::from_millis(1_700_000_000_000),
        kind: depot_core::FactKind::WorkerSubmitted {
            task: TaskId::new("t-1"),
            commit: depot_core::CommitId::new("aaa111"),
        },
    };
    let key = event_key(&["t-1", "worker_submitted", "aaa111"]);

    assert!(matches!(
        store.record_event(&added.project.id, &key, &fact).unwrap(),
        depotd::EventOutcome::Recorded
    ));
    assert!(matches!(
        store.record_event(&added.project.id, &key, &fact).unwrap(),
        depotd::EventOutcome::Duplicate
    ));

    let events = store.events(&added.project.id).expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, key);
    assert_eq!(events[0].kind, "worker_submitted");
    assert!(events[0].payload.contains("\"commit\":\"aaa111\""));
}
