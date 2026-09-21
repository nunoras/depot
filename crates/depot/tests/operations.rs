use std::path::PathBuf;
use std::process::{Command, Output};

use depot_core::{Question, Task, TaskId, TaskState, Timestamp};
use depotd::{DepotHome, HOME_ENV, Store};

const DEPOT: &str = env!("CARGO_BIN_EXE_depot");

struct Cli {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Cli {
    fn new(config: &str) -> Self {
        let temp = tempfile::tempdir().expect("temporary directory");
        let home = temp.path().join("depot-home");
        std::fs::create_dir_all(&home).expect("the depot home");
        std::fs::write(home.join("config.toml"), config).expect("machine-local settings");
        let project = temp.path().join("example");
        std::fs::create_dir_all(&project).expect("the project directory");
        std::fs::write(
            project.join(".depot.toml"),
            "[profiles]\nbuild = \"builder\"\n",
        )
        .expect("the project config");

        let cli = Self {
            _temp: temp,
            home,
            project,
        };
        cli.ok(&["project", "add", cli.project.to_str().expect("utf-8")]);
        cli
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(DEPOT)
            .args(arguments)
            .env(HOME_ENV, &self.home)
            .env_remove("DEPOT_TASK_ID")
            .env_remove("DEPOT_ATTEMPT_ID")
            .current_dir(&self.project)
            .output()
            .expect("the depot binary runs")
    }

    fn ok(&self, arguments: &[&str]) -> String {
        let output = self.run(arguments);
        assert_eq!(
            output.status.code(),
            Some(0),
            "depot {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn failed(&self, arguments: &[&str]) -> String {
        let output = self.run(arguments);
        assert_ne!(
            output.status.code(),
            Some(0),
            "depot {arguments:?} was expected to fail: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn store(&self) -> Store {
        Store::open(&DepotHome::at(&self.home)).expect("the store opens")
    }

    fn add_task(&self, extra: &[&str]) -> String {
        let mut arguments = vec![
            "task",
            "add",
            "--title",
            "a task",
            "--intent",
            "why the task exists",
        ];
        arguments.extend_from_slice(extra);
        self.ok(&arguments)
    }
}

fn settings(publish: Option<&str>) -> String {
    let mut text = String::from(
        "concurrency = 1\npoll_interval_seconds = 1\n\n[profiles.builder]\nharness = \"pi\"\nmodel = \"glm-5.3\"\neffort = \"high\"\naccount = \"\"\n",
    );
    if let Some(command) = publish {
        text.push_str(&format!("\n[artifacts]\npublish_command = \"{command}\"\n"));
    }
    text
}

fn stored(cli: &Cli, id: &str, state: TaskState, question: Option<&str>) {
    let store = cli.store();
    let project = store.projects().expect("projects").remove(0);
    let task = Task {
        id: TaskId::new(id),
        project: project.id.clone(),
        title: format!("task {id}"),
        intent: format!("intent for {id}"),
        role: depot_core::Role::Build,
        dispatch_profile: None,
        state,
        dependencies: Vec::new(),
        base_dependency: None,
        attempts: Vec::new(),
        questions: question
            .map(|text| {
                vec![Question {
                    text: text.to_string(),
                    asked_at: Timestamp::from_millis(1),
                    answer: None,
                }]
            })
            .unwrap_or_default(),
        validations: Vec::new(),
        submission: None,
        artifacts: Vec::new(),
        links: Vec::new(),
        branch_head: None,
        merge_refused: None,
        redirect_text: None,
        redirect_delivered: false,
        acknowledged_at: None,
        hold_pr: false,
        rework_of: None,
        retry: None,
        created_at: Timestamp::from_millis(1),
        updated_at: Timestamp::from_millis(1),
    };
    store.put_task(&task).expect("the task is stored");
}

fn publishing_command(url: &str) -> String {
    if cfg!(windows) {
        format!("echo {url}")
    } else {
        format!("printf '{url}'")
    }
}

#[test]
fn version_names_the_package_and_the_embedded_build() {
    let cli = Cli::new(&settings(None));
    let output = cli.run(&["--version"]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("depot "), "got {stdout}");
    assert!(
        stdout.contains(&format!("(build {})", depotd::BUILD_ID)),
        "got {stdout}"
    );
}

#[test]
fn project_list_names_each_project_its_path_and_its_resolved_profiles() {
    let cli = Cli::new(&settings(None));
    std::fs::write(
        cli.project.join(".depot.toml"),
        "[profiles]\nbuild = \"builder\"\nreview = \"unmapped\"\n",
    )
    .expect("the project config");

    let listed = cli.ok(&["project", "list"]);

    assert!(listed.starts_with("example\n"), "got {listed}");
    assert!(
        listed.contains(&format!(
            "path: {}",
            std::fs::canonicalize(&cli.project)
                .expect("the project path")
                .display()
        )),
        "got {listed}"
    );
    assert!(listed.contains("build=builder"), "got {listed}");
    assert!(
        listed.contains("review=unmapped (not defined in machine-local settings)"),
        "got {listed}"
    );
}

#[test]
fn project_list_says_so_when_nothing_is_registered() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let output = Command::new(DEPOT)
        .args(["project", "list"])
        .env(HOME_ENV, temp.path().join("depot-home"))
        .env_remove("DEPOT_TASK_ID")
        .env_remove("DEPOT_ATTEMPT_ID")
        .current_dir(temp.path())
        .output()
        .expect("the depot binary runs");

    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("No projects registered."));
}

#[test]
fn kind_is_an_alias_for_role_and_a_disagreement_is_refused() {
    let cli = Cli::new(&settings(None));

    assert_eq!(cli.add_task(&["--kind", "build"]), "added t-1\n");

    let message = cli.failed(&[
        "task", "add", "--title", "a task", "--intent", "why", "--role", "build", "--kind",
        "review",
    ]);
    assert!(message.contains("disagree"), "got {message}");
}

#[test]
fn approving_a_cancelled_task_points_at_retry() {
    let cli = Cli::new(&settings(None));
    cli.add_task(&["--role", "build"]);
    cli.ok(&["task", "stop", "t-1"]);

    let message = cli.failed(&["task", "approve", "t-1"]);

    assert!(
        message.contains("depot task retry t-1"),
        "the refusal suggests the way back, got {message}"
    );
}

#[test]
fn wait_returns_when_the_task_settles() {
    let cli = Cli::new(&settings(None));
    cli.add_task(&["--role", "build"]);
    cli.ok(&["task", "stop", "t-1"]);

    assert_eq!(cli.ok(&["task", "wait", "t-1"]), "t-1 cancelled\n");
}

#[test]
fn wait_prints_the_question_a_task_is_blocked_on() {
    let cli = Cli::new(&settings(None));
    stored(
        &cli,
        "t-1",
        TaskState::WaitingOnQuestion,
        Some("which base branch?"),
    );

    let waited = cli.ok(&["task", "wait", "t-1"]);

    assert_eq!(waited, "t-1 waiting_on_question\nwhich base branch?\n");
}

#[test]
fn wait_times_out_nonzero_and_says_what_it_saw() {
    let cli = Cli::new(&settings(None));
    cli.add_task(&["--role", "build"]);

    let message = cli.failed(&["task", "wait", "t-1", "--timeout", "1"]);

    assert!(message.contains("still proposed after 1s"), "got {message}");
    assert!(message.contains("--timeout"), "got {message}");
}

#[test]
fn artifact_add_stages_the_file_and_prints_the_published_url() {
    let cli = Cli::new(&settings(Some(&publishing_command(
        "https://example.test/report",
    ))));
    let file = cli.project.join("report with spaces.txt");
    std::fs::write(&file, "the report\n").expect("the artifact");

    let published = cli.ok(&["artifact", "add", file.to_str().expect("utf-8")]);

    assert!(
        published.contains("https://example.test/report"),
        "got {published}"
    );
    let staged = cli.home.join("artifacts").join("report with spaces.txt");
    assert_eq!(
        std::fs::read_to_string(&staged).expect("the staged copy"),
        "the report\n"
    );
}

#[test]
fn artifact_add_without_configuration_explains_the_setup() {
    let cli = Cli::new(&settings(None));
    let file = cli.project.join("report.txt");
    std::fs::write(&file, "the report\n").expect("the artifact");

    let message = cli.failed(&["artifact", "add", file.to_str().expect("utf-8")]);

    assert!(message.contains("publish_command"), "got {message}");
    assert!(
        message.contains(&cli.home.join("config.toml").display().to_string()),
        "got {message}"
    );
}

#[test]
fn artifact_add_refuses_a_path_that_is_not_a_file() {
    let cli = Cli::new(&settings(Some(&publishing_command(
        "https://example.test/x",
    ))));

    let message = cli.failed(&["artifact", "add", cli.project.to_str().expect("utf-8")]);

    assert!(message.contains("not a regular file"), "got {message}");
}

#[test]
fn daemon_dispatch_names_its_only_subcommand() {
    let cli = Cli::new(&settings(None));

    assert!(cli.failed(&["daemon"]).contains("depot daemon restart"));
    assert!(
        cli.failed(&["daemon", "bounce"])
            .contains("unknown daemon command")
    );
}

#[test]
fn the_worker_context_may_not_restart_the_daemon() {
    let cli = Cli::new(&settings(None));
    let output = Command::new(DEPOT)
        .args(["daemon", "restart"])
        .env(HOME_ENV, &cli.home)
        .env("DEPOT_TASK_ID", "t-1")
        .env("DEPOT_ATTEMPT_ID", "attempt-1")
        .current_dir(&cli.project)
        .output()
        .expect("the depot binary runs");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("worker context"),
        "got {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_qualified_task_id_works_from_any_directory() {
    let cli = Cli::new(&settings(None));
    cli.add_task(&["--role", "build"]);
    cli.ok(&["task", "stop", "t-1"]);

    let output = Command::new(DEPOT)
        .args(["task", "wait", "example/t-1"])
        .env(HOME_ENV, &cli.home)
        .env_remove("DEPOT_TASK_ID")
        .env_remove("DEPOT_ATTEMPT_ID")
        .current_dir(cli._temp.path())
        .output()
        .expect("the depot binary runs");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "t-1 cancelled\n",
        "a qualified id resolves outside the repository"
    );
}
