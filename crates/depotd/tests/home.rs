mod support;

use std::collections::BTreeSet;

use depotd::{
    ARCHIVE_DIR_NAME, CHECKLIST_FILE_NAME, CONTEXT_DOCUMENT_FILE_NAME, DATABASE_FILE_NAME,
    DOCUMENTS_DIR_NAME, DepotHome, MEDIA_DIR_NAME, PROJECTS_DIR_NAME, RUN_DIR_NAME,
    SCRATCH_DIR_NAME, SECRETS_DIR_NAME, SETTINGS_FILE_NAME, Settings, Store, UI_DIR_NAME,
    add_project, slug_for,
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
fn a_fresh_home_lays_out_agni_with_secrets_ui_and_run() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let home = DepotHome::at(temp.path().join(".agni"));

    home.ensure().expect("home");

    assert_eq!(DATABASE_FILE_NAME, "agni.db");
    assert_eq!(
        home.database_path().file_name().unwrap(),
        DATABASE_FILE_NAME
    );
    for (directory, name) in [
        (home.secrets_dir(), SECRETS_DIR_NAME),
        (home.ui_dir(), UI_DIR_NAME),
        (home.run_dir(), RUN_DIR_NAME),
        (home.projects_dir(), PROJECTS_DIR_NAME),
    ] {
        assert!(directory.is_dir(), "{} is missing", directory.display());
        assert_eq!(directory.file_name().unwrap(), name);
    }
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
fn a_registered_project_gets_a_context_document_depot_never_renders() {
    let fixture = support::fixture();
    let added = support::register(&fixture, "example");

    let context = added.home.context_document_path();
    assert_eq!(
        context.file_name().unwrap(),
        CONTEXT_DOCUMENT_FILE_NAME,
        "the kickoff names this file"
    );
    assert!(
        context.is_file(),
        "the coordinator's context document is there before the first session"
    );

    std::fs::write(&context, "# Context\n\nhand written by the coordinator\n").expect("written");
    support::register(&fixture, "example");

    assert_eq!(
        std::fs::read_to_string(&context).expect("read"),
        "# Context\n\nhand written by the coordinator\n",
        "depot renders the checklist and never the narrative documents"
    );
}

#[test]
fn machine_local_settings_survive_a_reopen_and_are_written_only_once() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let home = DepotHome::at(temp.path().join("depot-home"));
    home.ensure().expect("home");
    let defaults = std::fs::read_to_string(home.config_path()).expect("config");

    home.write_settings(&Settings {
        concurrency: 9,
        run_duration_minutes: 45,
        ..Settings::default()
    })
    .expect("settings");
    home.ensure().expect("home again");

    let settings = home.load_settings().expect("settings");
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
fn a_broken_project_config_refuses_add_without_registering() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "broken");
    std::fs::write(
        directory.join(depotd::PROJECT_CONFIG_FILE_NAME),
        "base_branch = \"main\"\n\n[profiles]\nbuild = \"\"\n",
    )
    .expect("broken project config");

    let error = add_project(&fixture.home, directory.to_str().unwrap())
        .expect_err("a broken project config must refuse registration");

    assert!(
        error.to_string().contains("empty profile"),
        "the refusal should name the config problem, got {error}"
    );

    let store = Store::open(&fixture.home).expect("store");
    assert!(
        store.projects().expect("projects").is_empty(),
        "a refused add must leave no durable project row"
    );
    assert!(
        !fixture.home.project_home("broken").root().exists(),
        "a refused add must leave no half-created project home"
    );
}

#[test]
fn a_failed_project_home_write_leaves_no_registration() {
    let fixture = support::fixture();
    fixture.home.ensure().expect("depot home");
    let directory = support::project_directory(&fixture, "example");
    let blocked_home = fixture.home.project_home("example");
    let blocked = blocked_home.root();
    std::fs::write(blocked, "not a directory").expect("block the project home path");

    let error = add_project(&fixture.home, directory.to_str().unwrap())
        .expect_err("a blocked project home must refuse registration");

    assert!(
        !error.to_string().is_empty(),
        "the refusal should carry an io error, got {error}"
    );

    let store = Store::open(&fixture.home).expect("store");
    assert!(
        store.projects().expect("projects").is_empty(),
        "a refused add must leave no durable project row"
    );
    assert!(
        blocked.is_file(),
        "the pre-existing block should remain a file, not a half-created project home"
    );
    assert!(
        !blocked.is_dir(),
        "a refused add must not replace the block with a project home directory"
    );

    std::fs::remove_file(blocked).expect("unblock");
    let added =
        add_project(&fixture.home, directory.to_str().unwrap()).expect("register after unblock");
    assert!(added.created);
    assert!(added.home.checklist_path().is_file());
    assert_eq!(store.projects().expect("projects").len(), 1);
}

#[test]
fn a_blocked_checklist_path_refuses_add_without_registering() {
    let fixture = support::fixture();
    fixture.home.ensure().expect("depot home");
    let directory = support::project_directory(&fixture, "blocked");
    let project_home = fixture.home.project_home("blocked");
    std::fs::create_dir_all(project_home.root()).expect("project home");
    std::fs::create_dir(project_home.checklist_path()).expect("block checklist path");

    let error = add_project(&fixture.home, directory.to_str().unwrap())
        .expect_err("a blocked checklist path must refuse registration");

    assert!(
        !error.to_string().is_empty(),
        "the refusal should carry an io error, got {error}"
    );

    let store = Store::open(&fixture.home).expect("store");
    assert!(
        store.projects().expect("projects").is_empty(),
        "a refused add must leave no durable project row"
    );
    assert!(
        project_home.checklist_path().is_dir(),
        "the blocked checklist path must remain a directory"
    );

    std::fs::remove_dir_all(project_home.root()).expect("remove the blocked home");
    let added =
        add_project(&fixture.home, directory.to_str().unwrap()).expect("register after unblock");
    assert!(added.created);
    assert!(added.home.checklist_path().is_file());
    assert_eq!(store.projects().expect("projects").len(), 1);
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
