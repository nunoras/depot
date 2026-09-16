use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use depot_core::{TaskId, TaskState};
use depotd::{DepotHome, Store, add_project};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_depot");

struct Cli {
    temp: TempDir,
    home: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary directory");
        let home = temp.path().join("depot-home");
        Self { temp, home }
    }

    fn depot_home(&self) -> DepotHome {
        DepotHome::at(&self.home)
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.run_from(arguments, self.temp.path())
    }

    fn run_from(&self, arguments: &[&str], directory: &Path) -> Output {
        Command::new(BIN)
            .args(arguments)
            .env("DEPOT_HOME", &self.home)
            .current_dir(directory)
            .output()
            .expect("the depot binary runs")
    }

    fn project_directory(&self, name: &str) -> PathBuf {
        let directory = self.temp.path().join(name);
        std::fs::create_dir_all(&directory).expect("project directory");
        directory
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn adding_a_project_registers_it_and_writes_its_home_directory() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");

    let output = cli.run(&["project", "add", directory.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).starts_with("registered "),
        "stdout: {}",
        stdout(&output)
    );
    assert!(directory.join(".depot.toml").is_file());

    let store = Store::open(&cli.depot_home()).expect("store");
    let projects = store.projects().expect("projects");
    assert_eq!(projects.len(), 1);
    let project_home = cli.depot_home().project_home(&projects[0].slug);
    assert!(project_home.checklist_path().is_file());
    assert!(project_home.archive_dir().is_dir());
    assert!(project_home.documents_dir().is_dir());
    assert!(project_home.scratch_dir().is_dir());
    assert!(project_home.media_dir().is_dir());
}

#[test]
fn adding_the_same_project_twice_is_idempotent() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");
    let config = directory.join(".depot.toml");

    let first = cli.run(&["project", "add", directory.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "stderr: {}", stderr(&first));
    let committed = std::fs::read_to_string(&config).expect("committed config");

    let second = cli.run(&["project", "add", directory.to_str().unwrap()]);

    assert_eq!(second.status.code(), Some(0), "stderr: {}", stderr(&second));
    assert!(
        stdout(&second).starts_with("already registered "),
        "stdout: {}",
        stdout(&second)
    );
    assert_eq!(std::fs::read_to_string(&config).expect("config"), committed);
    assert_eq!(
        Store::open(&cli.depot_home())
            .expect("store")
            .projects()
            .expect("projects")
            .len(),
        1
    );
}

#[test]
fn a_project_with_an_unusable_path_is_refused_with_a_clear_error() {
    let cli = Cli::new();
    let missing = cli.temp.path().join("absent");

    let output = cli.run(&["project", "add", missing.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert!(
        stderr(&output).contains("does not exist"),
        "stderr: {}",
        stderr(&output)
    );

    let file = cli.temp.path().join("notes.txt");
    std::fs::write(&file, "not a project").expect("file");

    let output = cli.run(&["project", "add", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("is not a directory"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(
        Store::open(&cli.depot_home())
            .expect("store")
            .projects()
            .expect("projects")
            .is_empty()
    );
}

#[test]
fn a_project_can_be_registered_by_url() {
    let cli = Cli::new();

    let output = cli.run(&["project", "add", "https://github.com/nunoras/depot.git"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let store = Store::open(&cli.depot_home()).expect("store");
    let projects = store.projects().expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id.as_str(), "https://github.com/nunoras/depot");
}

#[test]
fn status_shows_held_running_blocked_waiting_and_validated_tasks_distinctly() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");
    let added = add_project(&cli.depot_home(), directory.to_str().unwrap()).expect("registered");
    let store = Store::open(&cli.depot_home()).expect("store");
    for (index, task_state) in [
        TaskState::Proposed,
        TaskState::Running,
        TaskState::WaitingOnQuestion,
        TaskState::Validated,
        TaskState::Failed,
    ]
    .into_iter()
    .enumerate()
    {
        store
            .put_task(&task(
                added.project.id.as_str(),
                &format!("t-{index}"),
                task_state,
                index as u64,
            ))
            .expect("stored");
    }

    let output = cli.run(&["status", "--all"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let rendered = stdout(&output);
    for label in [
        "Held - awaiting approval",
        "Running",
        "Waiting on a question",
        "Validated",
        "Blocked - needs a person",
    ] {
        assert!(
            rendered.contains(&format!("## {label} (1)")),
            "missing {label} in\n{rendered}"
        );
    }
}

#[test]
fn status_reads_the_project_the_directory_belongs_to() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");
    let added = add_project(&cli.depot_home(), directory.to_str().unwrap()).expect("registered");

    let output = cli.run_from(&["status"], &directory);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains(added.project.id.as_str()));
}

#[test]
fn status_over_no_projects_says_so() {
    let cli = Cli::new();

    let output = cli.run(&["status", "--all"]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "No projects registered.\n");
}

#[test]
fn status_reports_a_project_it_cannot_find() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");
    add_project(&cli.depot_home(), directory.to_str().unwrap()).expect("registered");

    let output = cli.run(&["status", "--project", "elsewhere"]);

    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert!(
        stderr(&output).contains("no project matches `elsewhere`"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn unknown_commands_and_flags_are_usage_errors() {
    let cli = Cli::new();

    for arguments in [
        vec!["frobnicate"],
        vec!["project"],
        vec!["project", "publish"],
        vec!["status", "--nope"],
        vec!["status", "--project", "one", "--all"],
        vec!["status", "--project"],
        vec!["project", "add"],
    ] {
        let output = cli.run(&arguments);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?} should be a usage error, stderr: {}",
            stderr(&output)
        );
    }
}

#[test]
fn depot_with_no_arguments_prints_usage() {
    let cli = Cli::new();

    let output = cli.run(&[]);

    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("USAGE"));
}

fn task(project: &str, id: &str, state: TaskState, offset: u64) -> depot_core::Task {
    let project = depot_core::ProjectId::new(project);
    let at = depot_core::Timestamp::from_millis(1_700_000_000_000 + offset);
    depot_core::Task {
        id: TaskId::new(id),
        project,
        title: format!("task {id}"),
        intent: "seeded by the command line test".to_string(),
        role: depot_core::Role::Build,
        state,
        dependencies: Vec::new(),
        base_dependency: None,
        attempts: Vec::new(),
        questions: Vec::new(),
        validations: Vec::new(),
        artifacts: Vec::new(),
        links: Vec::new(),
        branch_head: None,
        retry: None,
        created_at: at,
        updated_at: at,
    }
}
