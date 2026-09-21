mod support;

use std::collections::BTreeMap;

use depot_core::{
    Fact, FactKind, Limits, ProfileId, ProjectState, Role, TaskId, TaskState, Timestamp,
};
use depotd::{
    PROJECT_CONFIG_FILE_NAME, ProfileSettings, ProjectConfig, Settings, Store, add_project,
    event_key,
};

fn exclude_lines(directory: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(directory.join(".git").join("info").join("exclude"))
        .expect("git exclude")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn registering_a_git_project_ignores_the_config_instead_of_writing_it() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "repo");
    let status = std::process::Command::new("git")
        .arg("init")
        .arg(&directory)
        .status()
        .expect("git");
    assert!(status.success());

    let added =
        add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered");

    assert!(added.ignored_config);
    assert!(!directory.join(PROJECT_CONFIG_FILE_NAME).exists());
    assert!(
        exclude_lines(&directory)
            .iter()
            .any(|line| line.trim() == PROJECT_CONFIG_FILE_NAME)
    );

    add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered again");
    let lines = exclude_lines(&directory);
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.trim() == PROJECT_CONFIG_FILE_NAME)
            .count(),
        1
    );
}

#[test]
fn registering_a_directory_outside_git_writes_no_ignore_rule() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "plain");

    let added =
        add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered");

    assert!(!added.ignored_config);
    assert!(!directory.join(".git").exists());
    assert!(!directory.join(PROJECT_CONFIG_FILE_NAME).exists());
}

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
         max_concurrent_tasks = 3\n\
         [profiles]\n\
         plan = \"fable-5\"\n\
         build = \"glm-5.3\"\n\n\
         [validation]\n\
         command = \"cargo test\"\n\n\
         [pull_request]\n\
         base = \"main\"\n\
         merge = \"manual\"\n\
         [questions]\n\
         always_relay = true\n",
    )
    .expect("project config");

    let settings = Settings {
        concurrency: 7,
        fallback_profiles: vec!["gpt-5.5".to_string(), "sonnet".to_string()],
        profiles: BTreeMap::from([
            (
                "fable-5".to_string(),
                ProfileSettings {
                    harness: "pi".to_string(),
                    model: "fable-5".to_string(),
                    effort: "high".to_string(),
                    account: String::new(),
                },
            ),
            (
                "glm-5.3".to_string(),
                ProfileSettings {
                    harness: "pi".to_string(),
                    model: "glm-5.3".to_string(),
                    effort: "high".to_string(),
                    account: String::new(),
                },
            ),
        ]),
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
        slug: added.project.slug.clone(),
        tasks: BTreeMap::from([(TaskId::new("t-1"), first), (TaskId::new("t-2"), second)]),
        coordinator: None,
        profiles: BTreeMap::from([
            (Role::Plan, ProfileId::new("fable-5")),
            (Role::Build, ProfileId::new("glm-5.3")),
        ]),
        fallback_profiles: vec![ProfileId::new("gpt-5.5"), ProfileId::new("sonnet")],
        limits: Limits {
            max_concurrent_tasks: 3,
            ..Limits::default()
        },
        always_relay_questions: true,
        merge_policy: depot_core::MergePolicy::Manual,
    };

    assert_eq!(
        store.project_state(&added.project).expect("state"),
        expected
    );
}

#[test]
fn a_project_with_no_committed_config_falls_back_to_defaults() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "bare");
    let config = ProjectConfig::load(&directory).expect("defaults when the directory exists");
    assert_eq!(config, ProjectConfig::default());

    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let state = store.project_state(&added.project).expect("state");
    assert!(state.profiles.is_empty());
    assert_eq!(state.limits.max_concurrent_tasks, 1);
    assert!(!state.always_relay_questions);
}

#[test]
fn a_missing_project_directory_is_refused_rather_than_defaulted() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "gone");
    std::fs::write(
        directory.join(PROJECT_CONFIG_FILE_NAME),
        "base_branch = \"develop\"\n\n\
         [profiles]\n\
         build = \"glm-5.3\"\n\n\
         [questions]\n\
         always_relay = true\n",
    )
    .expect("project config");
    let added =
        add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered");
    std::fs::remove_dir_all(&directory).expect("delete the project path");

    let store = Store::open(&fixture.home).expect("store");
    let error = store
        .project_state(&added.project)
        .expect_err("a deleted project path must not read as healthy defaults");
    let message = error.to_string();

    assert!(
        message.contains("does not exist"),
        "the refusal should name the missing path, got {message}"
    );
    assert!(
        message.contains(added.project.id.as_str()) || message.contains("gone"),
        "the refusal should identify the project path, got {message}"
    );
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

    let loaded = store
        .event(&added.project.id, &key)
        .expect("lookup")
        .expect("present");
    assert_eq!(loaded.project, added.project.id);
    assert_eq!(loaded.key, key);
}

#[test]
fn event_keys_are_isolated_per_project() {
    let fixture = support::fixture();
    let first = support::register(&fixture, "first");
    let second = support::register(&fixture, "second");
    let store = Store::open(&fixture.home).expect("store");
    let key = event_key(&["t-1", "worker_submitted", "aaa111"]);
    let first_fact = depot_core::Fact {
        at: depot_core::Timestamp::from_millis(1_700_000_000_000),
        kind: depot_core::FactKind::WorkerSubmitted {
            task: TaskId::new("t-1"),
            commit: depot_core::CommitId::new("aaa111"),
        },
    };
    let second_fact = depot_core::Fact {
        at: depot_core::Timestamp::from_millis(1_700_000_000_100),
        kind: depot_core::FactKind::WorkerSubmitted {
            task: TaskId::new("t-1"),
            commit: depot_core::CommitId::new("aaa111"),
        },
    };

    assert!(matches!(
        store
            .record_event(&first.project.id, &key, &first_fact)
            .unwrap(),
        depotd::EventOutcome::Recorded
    ));
    assert!(matches!(
        store
            .record_event(&second.project.id, &key, &second_fact)
            .unwrap(),
        depotd::EventOutcome::Recorded
    ));

    let first_events = store.events(&first.project.id).expect("first events");
    let second_events = store.events(&second.project.id).expect("second events");
    assert_eq!(first_events.len(), 1);
    assert_eq!(second_events.len(), 1);
    assert_eq!(first_events[0].project, first.project.id);
    assert_eq!(second_events[0].project, second.project.id);

    let first_lookup = store
        .event(&first.project.id, &key)
        .expect("first lookup")
        .expect("first present");
    let second_lookup = store
        .event(&second.project.id, &key)
        .expect("second lookup")
        .expect("second present");
    assert_eq!(first_lookup.project, first.project.id);
    assert_eq!(second_lookup.project, second.project.id);
    assert_eq!(first_lookup.at.millis(), 1_700_000_000_000);
    assert_eq!(second_lookup.at.millis(), 1_700_000_000_100);
}

#[test]
fn a_worker_session_failure_round_trips_through_the_journal_and_the_task() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let task = support::simple_task(&added.project.id, "t-1", TaskState::Running, 1);
    store.put_task(&task).expect("stored");

    let fact = Fact {
        at: Timestamp::from_millis(2),
        kind: FactKind::WorkerSessionFailed {
            task: TaskId::new("t-1"),
            reason: "the harness crashed".to_string(),
        },
    };
    store
        .apply_fact(&added.project, "worker-session-failed", &fact)
        .expect("the failure fact is recorded");

    let event = store
        .events(&added.project.id)
        .expect("events")
        .into_iter()
        .find(|event| event.kind == "worker_session_failed")
        .expect("the fact is in the journal");
    assert!(
        event.payload.contains("the harness crashed"),
        "{}",
        event.payload
    );

    let loaded = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(loaded.state, TaskState::Failed);
    assert_eq!(loaded.failure.as_deref(), Some("the harness crashed"));
}
