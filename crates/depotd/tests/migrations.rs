mod support;

use depot_core::{ProjectId, TaskId};
use depotd::{SCHEMA_VERSION, Store};
use rusqlite::Connection;

#[test]
fn a_fresh_store_lands_at_the_current_schema() {
    let fixture = support::fixture();

    let store = Store::open(&fixture.home).expect("store");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn a_store_written_by_an_older_schema_migrates_forward_on_open() {
    let fixture = support::fixture();
    let path = fixture.home.database_path();
    std::fs::create_dir_all(fixture.home.root()).expect("home directory");

    let legacy = Connection::open(&path).expect("legacy database");
    legacy
        .execute_batch(include_str!("fixtures/schema-v1.sql"))
        .expect("v1 schema");
    let fixture_version: i64 = legacy
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("fixture schema version");
    assert!(
        SCHEMA_VERSION > fixture_version,
        "the frozen fixture must be an older schema than {SCHEMA_VERSION}"
    );
    legacy
        .execute_batch(
            "INSERT INTO projects (id, kind, slug, created_at) VALUES ('/work/example', 'path', 'example', 1700000000000);
             INSERT INTO tasks (
                 project_id, id, title, intent, role, state, base_dependency, branch_head,
                 retry_profile, retry_not_before, created_at, updated_at
             ) VALUES (
                 '/work/example', 't-1', 'Wire the store', 'persist the records', 'build',
                 'validated', NULL, 'abc1234', NULL, NULL, 1700000000000, 1700000000000
             );
             INSERT INTO task_validations (
                 project_id, task_id, position, command, commit_id, exit_code, duration_millis,
                 output_tail
             ) VALUES ('/work/example', 't-1', 0, 'cargo test', 'abc1234', 0, 4200, 'ok');",
        )
        .expect("legacy rows");
    drop(legacy);

    let store = Store::open(&fixture.home).expect("migrated store");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let project = store
        .project(&ProjectId::new("/work/example"))
        .unwrap()
        .expect("the project survived the migration");
    assert_eq!(project.slug, "example");
    let task = store
        .task(&project.id, &TaskId::new("t-1"))
        .unwrap()
        .expect("the task survived the migration");
    assert_eq!(task.title, "Wire the store");
    assert_eq!(task.validations.len(), 1);
    assert_eq!(task.validations[0].exit_code, 0);

    let key = "t-1:worker_submitted:abc1234";
    let first = depot_core::Fact {
        at: depot_core::Timestamp::from_millis(1_700_000_000_000),
        kind: depot_core::FactKind::WorkerSubmitted {
            task: TaskId::new("t-1"),
            commit: depot_core::CommitId::new("abc1234"),
        },
    };
    store
        .put_project(&depotd::Project {
            id: ProjectId::new("/work/other"),
            kind: depotd::LocationKind::Path,
            slug: "other".to_string(),
            created_at: depot_core::Timestamp::from_millis(1_700_000_000_000),
        })
        .expect("second project");
    assert!(matches!(
        store
            .record_event(&ProjectId::new("/work/example"), key, &first)
            .unwrap(),
        depotd::EventOutcome::Recorded
    ));
    assert!(matches!(
        store
            .record_event(&ProjectId::new("/work/other"), key, &first)
            .unwrap(),
        depotd::EventOutcome::Recorded
    ));
}

fn column_names(connection: &Connection, table: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("table info");
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("column rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("column names")
}

#[test]
fn a_store_migrated_from_main_gains_every_new_column() {
    let fixture = support::fixture();
    let path = fixture.home.database_path();
    std::fs::create_dir_all(fixture.home.root()).expect("home directory");

    let previous = Connection::open(&path).expect("previous database");
    previous
        .execute_batch(include_str!("fixtures/schema-v18.sql"))
        .expect("previous schema");
    let previous_version: i64 = previous
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("previous schema version");
    assert_eq!(
        previous_version, 18,
        "the frozen fixture is main's released schema"
    );
    assert!(
        previous_version < SCHEMA_VERSION,
        "the frozen fixture must be an older schema than {SCHEMA_VERSION}"
    );
    previous
        .execute_batch(
            "INSERT INTO projects (id, kind, slug, created_at) VALUES ('/work/example', 'path', 'example', 1700000000000);
             INSERT INTO tasks (project_id, id, title, intent, role, state, created_at, updated_at)
                 VALUES ('/work/example', 't-1', 'Wire the store', 'persist the records', 'build', 'validated', 1700000000000, 1700000000000);
             INSERT INTO task_counters (project_id, next_number) VALUES ('/work/example', 7);",
        )
        .expect("previous rows");
    drop(previous);

    let store = Store::open(&fixture.home).expect("migrated store");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);

    let connection = Connection::open(&path).expect("migrated database");
    let columns = column_names(&connection, "tasks");
    for column in [
        "conflict_base",
        "failure",
        "release_pending",
        "release_held",
    ] {
        assert!(
            columns.contains(&column.to_string()),
            "the migration must add the {column} column"
        );
    }
    let counter: i64 = connection
        .query_row(
            "SELECT next_number FROM task_counters WHERE project_id = '/work/example'",
            [],
            |row| row.get(0),
        )
        .expect("task_counters is intact");
    assert_eq!(counter, 7);
}

#[test]
fn a_store_written_by_a_newer_schema_is_refused_rather_than_downgraded() {
    let fixture = support::fixture();
    let path = fixture.home.database_path();
    std::fs::create_dir_all(fixture.home.root()).expect("home directory");

    let future = Connection::open(&path).expect("database");
    future
        .execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 7))
        .expect("future schema");
    drop(future);

    let error = match Store::open(&fixture.home) {
        Ok(_) => panic!("a newer store must not be opened"),
        Err(error) => error,
    };
    let message = error.to_string();

    assert!(
        message.contains(&(SCHEMA_VERSION + 7).to_string()),
        "the refusal should name the schema it found, got {message}"
    );
}
