mod support;

use std::collections::BTreeSet;

use depotd::{
    ARCHIVE_DIR_NAME, CHECKLIST_FILE_NAME, DATABASE_FILE_NAME, DOCUMENTS_DIR_NAME, MEDIA_DIR_NAME,
    PROJECTS_DIR_NAME, SCRATCH_DIR_NAME, SETTINGS_FILE_NAME, Settings, Store, add_project,
    slug_for,
};

#[test]
fn the_depot_home_holds_the_machine_local_config_and_the_database() {
    let fixture = support::fixture();

    Store::open(&fixture.home).expect("store");

    assert!(fixture.home.config_path().is_file());
    assert_eq!(
        fixture.home.config_path().file_name().unwrap(),
        SETTINGS_FILE_NAME
    );
    assert_eq!(
        fixture.home.database_path().file_name().unwrap(),
        DATABASE_FILE_NAME
    );
    assert!(fixture.home.database_path().is_file());
    assert!(fixture.home.projects_dir().is_dir());
    assert_eq!(
        fixture.home.projects_dir().file_name().unwrap(),
        PROJECTS_DIR_NAME
    );
}

#[test]
fn a_registered_project_gets_the_checklist_the_archive_documents_scratch_and_media() {
    let fixture = support::fixture();

    let added = support::register(&fixture, "example");
    let home = added.home.root();

    assert!(home.join(CHECKLIST_FILE_NAME).is_file(), "checklist");
    assert!(home.join(ARCHIVE_DIR_NAME).is_dir(), "archive");
    assert!(home.join(DOCUMENTS_DIR_NAME).is_dir(), "documents");
    assert!(home.join(SCRATCH_DIR_NAME).is_dir(), "scratch");
    assert!(home.join(MEDIA_DIR_NAME).is_dir(), "media");
    assert!(
        added.home.root().starts_with(fixture.home.projects_dir()),
        "a project directory lives under the depot home"
    );
    assert!(
        std::fs::read_to_string(added.home.checklist_path())
            .expect("checklist")
            .contains("No tasks yet."),
        "a fresh project renders an empty checklist"
    );
}

#[test]
fn machine_local_settings_survive_a_reopen_and_are_written_only_once() {
    let fixture = support::fixture();
    fixture.home.ensure().expect("home");
    let defaults = std::fs::read_to_string(fixture.home.config_path()).expect("config");

    fixture
        .home
        .write_settings(&Settings {
            concurrency: 9,
            run_duration_minutes: 45,
            ..Settings::default()
        })
        .expect("settings");
    fixture.home.ensure().expect("home again");

    let settings = fixture.home.load_settings().expect("settings");
    assert_eq!(settings.concurrency, 9);
    assert_eq!(settings.run_duration().as_secs(), 45 * 60);
    assert_eq!(settings.poll_interval().as_secs(), 30);
    assert_eq!(settings.limits().max_concurrent_tasks, 9);
    assert_eq!(
        Settings::from_toml(&defaults).expect("the first open writes a readable config"),
        Settings::default(),
        "the first open writes the documented defaults, got\n{defaults}"
    );
}

#[test]
fn slugs_come_from_the_project_name() {
    assert_eq!(slug_for("/home/nunoras/src/depot"), "depot");
    assert_eq!(slug_for("/home/nunoras/src/Depot.Api"), "depot-api");
    assert_eq!(slug_for("https://github.com/nunoras/depot"), "depot");
    assert_eq!(slug_for("git@github.com:nunoras/depot.git"), "depot");
    assert_eq!(slug_for("https://github.com/nunoras/depot/"), "depot");
    assert_eq!(slug_for("/"), "project");
    assert_eq!(slug_for("..."), "project");
}

#[test]
fn two_projects_with_the_same_name_get_their_own_directories() {
    let fixture = support::fixture();
    let first = support::project_directory(&fixture, "one/depot");
    let second = support::project_directory(&fixture, "two/depot");

    let first = add_project(&fixture.home, first.to_str().unwrap()).expect("first");
    let second = add_project(&fixture.home, second.to_str().unwrap()).expect("second");

    assert_ne!(first.project.slug, second.project.slug);
    assert!(first.home.root().is_dir());
    assert!(second.home.root().is_dir());

    let slugs: BTreeSet<String> = fixture
        .home
        .projects_dir()
        .read_dir()
        .expect("projects")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        slugs,
        BTreeSet::from([first.project.slug.clone(), second.project.slug.clone()])
    );

    let third = add_project(
        &fixture.home,
        fixture.temp.path().join("one/depot").to_str().unwrap(),
    )
    .expect("first again");
    assert_eq!(third.project.slug, first.project.slug);
    assert!(!third.created);
}
