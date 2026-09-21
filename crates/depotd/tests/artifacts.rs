mod support;

use std::path::{Path, PathBuf};

use depotd::{ArtifactsSettings, Settings, add_artifact, exactly_one_url};

fn home_with(command: Option<&str>) -> (tempfile::TempDir, depotd::DepotHome) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let home = depotd::DepotHome::at(temp.path().join("depot-home"));
    home.ensure().expect("the depot home");
    let settings = Settings {
        artifacts: ArtifactsSettings {
            publish_command: command.unwrap_or("").to_string(),
        },
        ..Settings::default()
    };
    home.write_settings(&settings).expect("settings");
    (temp, home)
}

fn artifact(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, format!("the bytes of {name}\n")).expect("the artifact is written");
    path
}

fn publish_command(record: &Path, url: &str) -> String {
    if cfg!(windows) {
        format!(
            ">\"{}\" <nul set /p=%DEPOT_ARTIFACT_PATH%& echo {url}",
            record.display()
        )
    } else {
        format!(
            "printf '%s' \"$DEPOT_ARTIFACT_PATH\" > '{}'; printf '{}'",
            record.display(),
            url
        )
    }
}

fn failing_command() -> &'static str {
    if cfg!(windows) { "exit /b 3" } else { "exit 3" }
}

fn printing(text: &str) -> String {
    if cfg!(windows) {
        format!("echo {text}")
    } else {
        format!("printf '{text}'")
    }
}

#[test]
fn a_staged_file_lands_under_its_own_name_and_the_command_reads_it_from_the_environment() {
    let (temp, home) = home_with(None);
    let record = temp.path().join("seen-path.txt");
    let settings = Settings {
        artifacts: ArtifactsSettings {
            publish_command: publish_command(&record, "https://example.test/report.pdf"),
        },
        ..Settings::default()
    };
    home.write_settings(&settings).expect("settings");

    let file = artifact(temp.path(), "a report with spaces.pdf");
    let staged = add_artifact(&home, &file).expect("the artifact publishes");

    assert_eq!(staged.url, "https://example.test/report.pdf");
    assert_eq!(
        staged.path,
        home.artifacts_dir().join("a report with spaces.pdf")
    );
    assert_eq!(
        std::fs::read_to_string(&staged.path).expect("the staged copy"),
        "the bytes of a report with spaces.pdf\n"
    );
    assert_eq!(
        std::fs::read_to_string(&record).expect("what the command saw"),
        staged.path.display().to_string()
    );
}

#[test]
fn a_second_file_with_the_same_name_lands_beside_the_first() {
    let (temp, home) = home_with(Some(&printing("https://example.test/x")));
    let file = artifact(temp.path(), "notes.md");

    let first = add_artifact(&home, &file).expect("the first");
    let second = add_artifact(&home, &file).expect("the second");

    assert_eq!(first.path, home.artifacts_dir().join("notes.md"));
    assert_eq!(second.path, home.artifacts_dir().join("notes-1.md"));
    assert!(first.path.exists());
    assert!(second.path.exists());
}

#[test]
fn a_missing_publish_command_explains_how_to_configure_one() {
    let (temp, home) = home_with(None);
    let file = artifact(temp.path(), "notes.md");

    let error = add_artifact(&home, &file).expect_err("no command is configured");
    let message = error.to_string();

    assert!(message.contains("publish_command"), "got {message}");
    assert!(
        message.contains(&home.config_path().display().to_string()),
        "the refusal points at the settings file, got {message}"
    );
    assert!(
        !home.artifacts_dir().join("notes.md").exists(),
        "nothing is staged before the command is known"
    );
}

#[test]
fn a_failing_command_keeps_the_staged_file_and_says_where() {
    let (_temp, home) = home_with(Some(failing_command()));
    let file = artifact(_temp.path(), "notes.md");

    let error = add_artifact(&home, &file).expect_err("the command fails");
    let message = error.to_string();

    assert!(message.contains("3"), "got {message}");
    assert!(
        message.contains(&home.artifacts_dir().join("notes.md").display().to_string()),
        "the refusal names the staged file, got {message}"
    );
    assert!(home.artifacts_dir().join("notes.md").exists());
}

#[test]
fn a_command_without_exactly_one_http_url_keeps_the_staged_file() {
    let (temp, home) = home_with(Some(&printing("no link here")));
    let file = artifact(temp.path(), "notes.md");
    let error = add_artifact(&home, &file).expect_err("no url");
    assert!(error.to_string().contains("no http(s) URL"));
    assert!(home.artifacts_dir().join("notes.md").exists());

    let two = Settings {
        artifacts: ArtifactsSettings {
            publish_command: printing("https://a.test/x https://b.test/y"),
        },
        ..Settings::default()
    };
    home.write_settings(&two).expect("settings");
    let error = add_artifact(&home, &file).expect_err("two urls");
    assert!(error.to_string().contains("2 different"), "got {error}");
    assert!(home.artifacts_dir().join("notes-1.md").exists());
}

#[test]
fn a_directory_is_not_an_artifact() {
    let (temp, home) = home_with(Some(&printing("https://example.test/x")));
    let directory = temp.path().join("a-directory");
    std::fs::create_dir_all(&directory).expect("the directory");

    let error = add_artifact(&home, &directory).expect_err("directories are refused");
    assert!(
        error.to_string().contains("not a regular file"),
        "got {error}"
    );
}

#[test]
fn exactly_one_url_reads_only_http_schemes() {
    assert_eq!(
        exactly_one_url("uploaded https://example.test/a\n").expect("one url"),
        "https://example.test/a"
    );
    assert_eq!(
        exactly_one_url("https://example.test/a https://example.test/a").expect("a repeat is one"),
        "https://example.test/a"
    );
    assert!(exactly_one_url("ftp://example.test/a").is_err());
    assert!(exactly_one_url("").is_err());
    assert!(
        exactly_one_url("https://a.test https://b.test")
            .expect_err("two")
            .contains("2 different")
    );
}
