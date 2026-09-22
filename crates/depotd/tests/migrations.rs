mod support;

use depot_core::{ProjectId, TaskId};
use depotd::{
    DepotHome, HOME_DIR_NAME, LEGACY_HOME_DIR_NAME, SCHEMA_VERSION, Store, migrate_store,
};
use fs2::FileExt;
use rusqlite::Connection;

#[test]
fn a_fresh_store_lands_at_the_current_schema() {
    let fixture = support::fixture();

    let store = Store::open(&fixture.home).expect("store");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn a_store_written_by_an_older_schema_migrates_when_the_daemon_opens() {
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

    let store = Store::open_migrating(&fixture.home).expect("migrated store");

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

    let store = Store::open_migrating(&fixture.home).expect("migrated store");

    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);

    let connection = Connection::open(&path).expect("migrated database");
    let columns = column_names(&connection, "tasks");
    for column in [
        "conflict_base",
        "failure",
        "release_pending",
        "release_held",
        "turn_deferral",
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
    assert!(
        message.contains(&SCHEMA_VERSION.to_string()),
        "the refusal should name the schema this build understands, got {message}"
    );
    assert!(
        message.contains("reinstall depot"),
        "the refusal should name the fix, got {message}"
    );
}

fn an_older_store(fixture: &support::Fixture) -> i64 {
    let path = fixture.home.database_path();
    std::fs::create_dir_all(fixture.home.root()).expect("home directory");
    let legacy = Connection::open(&path).expect("legacy database");
    legacy
        .execute_batch(include_str!("fixtures/schema-v18.sql"))
        .expect("v18 schema");
    let version: i64 = legacy
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("fixture schema version");
    drop(legacy);
    assert!(
        SCHEMA_VERSION > version,
        "the frozen fixture must be an older schema than {SCHEMA_VERSION}"
    );
    version
}

#[test]
fn a_client_open_of_an_older_store_is_refused_without_migrating() {
    let fixture = support::fixture();
    let version = an_older_store(&fixture);

    let error = match Store::open(&fixture.home) {
        Ok(_) => panic!("a client must not open an older store"),
        Err(error) => error,
    };
    let message = error.to_string();

    assert!(
        message.contains(&version.to_string()),
        "the refusal should name the schema it found, got {message}"
    );
    assert!(
        message.contains(&SCHEMA_VERSION.to_string()),
        "the refusal should name the schema this build understands, got {message}"
    );
    assert!(
        message.contains("depot store migrate"),
        "the refusal should name the fix, got {message}"
    );

    let connection = Connection::open(fixture.home.database_path()).expect("database");
    let stored: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("schema version");
    assert_eq!(stored, version, "the client left the schema untouched");
}

#[test]
fn an_explicit_migrate_moves_the_store_to_the_current_schema() {
    let fixture = support::fixture();
    let version = an_older_store(&fixture);

    let migration = migrate_store(&fixture.home).expect("the store migrates");

    assert_eq!(migration.from, version);
    assert_eq!(migration.to, SCHEMA_VERSION);
    let store = Store::open(&fixture.home).expect("the store opens after migrating");
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn an_explicit_migrate_of_a_current_store_is_a_no_op() {
    let fixture = support::fixture();
    Store::open(&fixture.home).expect("fresh store");

    let migration = migrate_store(&fixture.home).expect("the store is current");

    assert_eq!(migration.from, SCHEMA_VERSION);
    assert_eq!(migration.to, SCHEMA_VERSION);
    assert_eq!(migration.moved_from, None);
}

fn a_legacy_home() -> (tempfile::TempDir, std::path::PathBuf, DepotHome) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let legacy = temp.path().join(LEGACY_HOME_DIR_NAME);
    let home = DepotHome::at(temp.path().join(HOME_DIR_NAME));
    std::fs::create_dir_all(legacy.join("projects").join("example")).expect("legacy project store");
    let connection = Connection::open(legacy.join("depot.db")).expect("legacy database");
    connection
        .execute_batch(include_str!("fixtures/schema-v18.sql"))
        .expect("v18 schema");
    connection
        .execute_batch(
            "INSERT INTO projects (id, kind, slug, created_at) VALUES ('/work/example', 'path', 'example', 1700000000000);
             INSERT INTO tasks (project_id, id, title, intent, role, state, created_at, updated_at)
                 VALUES ('/work/example', 't-1', 'Wire the store', 'persist the records', 'build', 'validated', 1700000000000, 1700000000000);
             INSERT INTO events (project_id, key, at, kind, payload)
                 VALUES ('/work/example', 't-1:task_proposed', 1700000000000, 'task_proposed', '{}');
             PRAGMA user_version = 18;",
        )
        .expect("legacy rows");
    drop(connection);
    std::fs::write(
        legacy.join("projects").join("example").join("checklist.md"),
        "kept\n",
    )
    .expect("legacy checklist");
    std::fs::write(legacy.join("github-token"), "legacy-token\n").expect("legacy token");
    std::fs::write(legacy.join("typesafe-key"), "legacy-key\n").expect("legacy key");
    (temp, legacy, home)
}

#[test]
fn a_legacy_depot_home_moves_into_agni_keeping_every_row() {
    let (_temp, legacy, home) = a_legacy_home();

    let migration = migrate_store(&home).expect("the legacy home moves");

    assert_eq!(migration.moved_from.as_deref(), Some(legacy.as_path()));
    assert_eq!(migration.to, SCHEMA_VERSION);
    assert!(!legacy.exists(), "the old home is gone once it has moved");
    assert!(home.database_path().is_file(), "agni.db holds the records");
    assert!(home.secrets_dir().join("github-token").is_file());
    assert!(home.secrets_dir().join("typesafe-key").is_file());
    assert!(home.project_home("example").checklist_path().is_file());

    let store = Store::open(&home).expect("the moved store opens");
    let project = store
        .project(&ProjectId::new("/work/example"))
        .expect("projects are read")
        .expect("the project survived the move");
    let task = store
        .task(&project.id, &TaskId::new("t-1"))
        .expect("tasks are read")
        .expect("the task survived the move");
    assert_eq!(task.title, "Wire the store");
    assert_eq!(store.events(&project.id).expect("events").len(), 1);
}

#[test]
fn a_move_interrupted_after_the_database_finishes_on_the_next_run() {
    let (_temp, legacy, home) = a_legacy_home();
    std::fs::create_dir_all(home.root()).expect("the partially moved home");
    std::fs::rename(legacy.join("depot.db"), home.database_path()).expect("the database moved");

    let migration = migrate_store(&home).expect("the interrupted move finishes");

    assert_eq!(migration.moved_from.as_deref(), Some(legacy.as_path()));
    assert!(!legacy.exists(), "the old home is gone once it has moved");
    assert!(home.secrets_dir().join("github-token").is_file());
    assert!(home.project_home("example").checklist_path().is_file());
    let store = Store::open(&home).expect("the moved store opens");
    assert!(
        store
            .project(&ProjectId::new("/work/example"))
            .expect("projects are read")
            .is_some()
    );

    let again = migrate_store(&home).expect("a rerun is a no-op");
    assert_eq!(again.moved_from, None);
    assert_eq!(again.to, SCHEMA_VERSION);
}

#[test]
fn a_move_refuses_while_a_daemon_holds_the_legacy_lock_and_names_its_pid() {
    let (_temp, legacy, home) = a_legacy_home();
    std::fs::write(
        legacy.join("depotd.scope.json"),
        r#"{"pid":4242,"started_at_millis":1,"projects":[],"heartbeat_millis":1}"#,
    )
    .expect("the legacy scope record");
    let lock = std::fs::File::create(legacy.join("depotd.lock")).expect("the legacy lock");
    lock.lock_exclusive()
        .expect("the legacy daemon holds the lock");

    let error = migrate_store(&home).expect_err("a live legacy daemon must refuse the move");
    let message = error.to_string();

    assert!(
        message.contains("4242"),
        "the refusal names the pid, got {message}"
    );
    assert!(
        message.contains("depot store migrate"),
        "the refusal names the command, got {message}"
    );
    assert!(legacy.join("depot.db").is_file(), "nothing moved");
    assert!(!home.database_path().exists(), "nothing moved into agni");
    assert!(
        !home.root().exists(),
        "a refused move leaves no agni skeleton behind"
    );
}

fn empty_database(path: &std::path::Path) {
    std::fs::remove_file(path).expect("the stale database is removed");
    let connection = Connection::open(path).expect("an empty database");
    connection
        .execute_batch(include_str!("fixtures/schema-v18.sql"))
        .expect("v18 schema");
}

fn an_agni_database_with_one_project(home: &DepotHome) {
    std::fs::create_dir_all(home.root()).expect("the agni home");
    let connection = Connection::open(home.database_path()).expect("the agni database");
    connection
        .execute_batch(include_str!("fixtures/schema-v18.sql"))
        .expect("v18 schema");
    connection
        .execute_batch(
            "INSERT INTO projects (id, kind, slug, created_at) VALUES ('/work/agni', 'path', 'agni', 1700000000000);",
        )
        .expect("the agni project");
}

fn project_count(database: &std::path::Path) -> i64 {
    let connection = Connection::open(database).expect("the database opens");
    connection
        .query_row("select count(*) from projects", [], |row| row.get(0))
        .expect("the count")
}

#[test]
fn a_move_that_stopped_between_the_wal_and_the_database_finishes_on_the_next_run() {
    let (_temp, legacy, home) = a_legacy_home();
    std::fs::create_dir_all(home.root()).expect("the partially moved home");
    std::fs::write(home.root().join("agni.db-wal"), "orphan wal").expect("the orphan wal");

    let migration = migrate_store(&home).expect("the interrupted move finishes");

    assert_eq!(migration.moved_from.as_deref(), Some(legacy.as_path()));
    assert!(
        !home.root().join("agni.db-wal").exists(),
        "the orphan wal is cleared before the database moves"
    );
    let store = Store::open(&home).expect("the moved store opens");
    assert!(
        store
            .project(&ProjectId::new("/work/example"))
            .expect("projects are read")
            .is_some()
    );
}

#[test]
fn an_empty_agni_database_is_replaced_by_the_legacy_home() {
    let (_temp, legacy, home) = a_legacy_home();
    home.ensure().expect("the agni skeleton");
    let connection = Connection::open(home.database_path()).expect("the empty agni database");
    connection
        .execute_batch(include_str!("fixtures/schema-v18.sql"))
        .expect("v18 schema");
    drop(connection);

    let migration = migrate_store(&home).expect("an empty agni database is replaced");

    assert_eq!(migration.moved_from.as_deref(), Some(legacy.as_path()));
    assert!(!legacy.exists(), "the old home is gone once it has moved");
    let store = Store::open(&home).expect("the moved store opens");
    assert!(
        store
            .project(&ProjectId::new("/work/example"))
            .expect("projects are read")
            .is_some()
    );
}

#[test]
fn a_move_refuses_when_both_databases_hold_projects() {
    let (_temp, legacy, home) = a_legacy_home();
    an_agni_database_with_one_project(&home);

    let error = migrate_store(&home).expect_err("two populated stores are never merged silently");
    let message = error.to_string();

    assert!(message.contains("already holds records"), "got {message}");
    assert!(message.contains("merge them by hand"), "got {message}");
    assert!(
        legacy.join("depot.db").is_file(),
        "the legacy store is untouched"
    );
    assert_eq!(
        project_count(&home.database_path()),
        1,
        "the agni store is untouched"
    );
}

#[test]
fn an_empty_legacy_database_is_discarded_when_agni_holds_records() {
    let (_temp, legacy, home) = a_legacy_home();
    empty_database(&legacy.join("depot.db"));
    an_agni_database_with_one_project(&home);

    let migration =
        migrate_store(&home).expect("the stray legacy database does not block the move");

    assert_eq!(migration.moved_from.as_deref(), Some(legacy.as_path()));
    assert!(!legacy.exists(), "the stray legacy home is cleared");
    assert_eq!(
        project_count(&home.database_path()),
        1,
        "agni keeps its records"
    );
    let store = Store::open(&home).expect("the store opens");
    assert!(
        store
            .project(&ProjectId::new("/work/agni"))
            .expect("projects are read")
            .is_some()
    );
}
