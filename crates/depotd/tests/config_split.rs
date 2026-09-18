mod support;

use std::collections::{BTreeMap, BTreeSet};

use depotd::{PROJECT_CONFIG_FILE_NAME, ProjectConfig, Settings, add_project};

const PROJECT_KEYS: [&str; 5] = [
    "base_branch",
    "profiles",
    "validation",
    "pull_request",
    "questions",
];

const PROJECT_ONLY_KEYS: [&str; 5] = [
    "base_branch",
    "validation",
    "pull_request",
    "questions",
    "dispatch",
];

const MACHINE_LOCAL_ONLY_KEYS: [&str; 8] = [
    "typesafe_base_url",
    "concurrency",
    "run_duration_minutes",
    "poll_interval_seconds",
    "pool_root",
    "fallback_profiles",
    "coordinator_context_tokens",
    "credentials",
];

const MACHINE_LOCAL_KEYS: [&str; 9] = [
    "typesafe_base_url",
    "concurrency",
    "run_duration_minutes",
    "poll_interval_seconds",
    "pool_root",
    "fallback_profiles",
    "coordinator_context_tokens",
    "credentials",
    "profiles",
];

#[test]
fn the_committed_project_file_holds_only_project_knowledge() {
    let config = ProjectConfig {
        profiles: BTreeMap::from([
            ("plan".to_string(), "fable-5".to_string()),
            ("build".to_string(), "glm-5.3".to_string()),
        ]),
        ..ProjectConfig::default()
    };

    let text = config.to_toml().expect("toml");

    assert_eq!(top_level_keys(&text), names(PROJECT_KEYS));
    assert!(
        keys_at_any_depth(&text).is_disjoint(&names(MACHINE_LOCAL_ONLY_KEYS)),
        "the committed project file must never carry a machine-local setting, got\n{text}"
    );
}

#[test]
fn the_machine_local_file_holds_only_machine_local_settings() {
    let settings = Settings {
        pool_root: Some("/pool".into()),
        fallback_profiles: vec!["gpt-5.5".to_string()],
        credentials: BTreeMap::from([("github".to_string(), "gh-cli".to_string())]),
        ..Settings::default()
    };

    let text = settings.to_toml().expect("toml");

    assert_eq!(top_level_keys(&text), names(MACHINE_LOCAL_KEYS));
    assert!(
        keys_at_any_depth(&text).is_disjoint(&names(PROJECT_ONLY_KEYS)),
        "the machine-local file must never carry project knowledge, got\n{text}"
    );
}

#[test]
fn a_project_config_that_carries_machine_local_settings_is_refused() {
    let error = ProjectConfig::from_toml("concurrency = 12\n")
        .expect_err("machine-local settings do not belong in the project file");
    let message = error.to_string();

    assert!(
        message.contains("concurrency"),
        "the refusal should name the offending key, got {message}"
    );
}

#[test]
fn registering_a_project_never_writes_machine_local_settings_into_the_repository() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "example");
    let committed = "base_branch = \"develop\"\n\n\
                     [profiles]\n\
                     build = \"glm-5.3\"\n\n\
                     [validation]\n\
                     command = \"cargo test\"\n";
    let path = directory.join(PROJECT_CONFIG_FILE_NAME);
    std::fs::write(&path, committed).expect("hand-written project config");
    let before = std::fs::read_to_string(&path).expect("read");

    fixture
        .home
        .write_settings(&Settings {
            typesafe_base_url: depotd::adapters::typesafe::DEFAULT_API_BASE.into(),
            concurrency: 12,
            run_duration_minutes: 90,
            poll_interval_seconds: 5,
            pool_root: Some(fixture.temp.path().join("pool")),
            fallback_profiles: vec!["sonnet".to_string()],
            coordinator_context_tokens: 60_000,
            credentials: BTreeMap::from([("github".to_string(), "gh-cli".to_string())]),
            profiles: BTreeMap::new(),
        })
        .expect("machine-local settings");

    add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered");

    let after = std::fs::read_to_string(&path).expect("read");
    assert_eq!(after, before);
    assert!(
        keys_at_any_depth(&after).is_disjoint(&names(MACHINE_LOCAL_ONLY_KEYS)),
        "the committed project file must never carry a machine-local setting, got\n{after}"
    );

    let settings = fixture.home.load_settings().expect("settings");
    assert_eq!(settings.concurrency, 12);
    assert_eq!(settings.pool_root, Some(fixture.temp.path().join("pool")));
}

#[test]
fn registering_a_project_twice_leaves_the_committed_config_alone() {
    let fixture = support::fixture();
    let directory = support::project_directory(&fixture, "example");
    add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered");
    let path = directory.join(PROJECT_CONFIG_FILE_NAME);
    let before = std::fs::read_to_string(&path).expect("read");

    let added = add_project(&fixture.home, directory.to_str().expect("utf-8 path"))
        .expect("registered again");

    assert!(!added.created);
    assert_eq!(std::fs::read_to_string(&path).expect("read"), before);
}

fn document(toml_text: &str) -> toml::Value {
    toml::from_str(toml_text).expect("valid toml")
}

fn top_level_keys(toml_text: &str) -> BTreeSet<String> {
    let toml::Value::Table(table) = document(toml_text) else {
        panic!("a configuration file is a table");
    };
    table.keys().cloned().collect()
}

fn keys_at_any_depth(toml_text: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    collect(&document(toml_text), &mut keys);
    keys
}

fn collect(value: &toml::Value, keys: &mut BTreeSet<String>) {
    let toml::Value::Table(table) = value else {
        return;
    };
    for (name, child) in table {
        keys.insert(name.clone());
        collect(child, keys);
    }
}

fn names<const N: usize>(keys: [&str; N]) -> BTreeSet<String> {
    keys.into_iter().map(str::to_string).collect()
}
