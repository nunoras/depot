use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use depot_core::{TaskId, TaskState};
use depotd::{DepotHome, ProfileSettings, Settings, Store, add_project};
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
        DepotHome::at(&home)
            .write_settings(&Settings {
                profiles: BTreeMap::from([(
                    "glm-5.3".to_string(),
                    ProfileSettings {
                        harness: "pi".to_string(),
                        model: "glm-5.3".to_string(),
                        effort: "high".to_string(),
                        account: String::new(),
                    },
                )]),
                ..Settings::default()
            })
            .expect("settings");
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

    fn run_worker_from(
        &self,
        arguments: &[&str],
        task: &str,
        attempt: &str,
        directory: &Path,
    ) -> Output {
        Command::new(BIN)
            .args(arguments)
            .env("DEPOT_HOME", &self.home)
            .env("DEPOT_TASK_ID", task)
            .env("DEPOT_ATTEMPT_ID", attempt)
            .current_dir(directory)
            .output()
            .expect("the depot binary runs")
    }

    fn run_worker(&self, arguments: &[&str], task: &str, attempt: &str) -> Output {
        self.run_worker_from(arguments, task, attempt, self.temp.path())
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
    assert!(!directory.join(".depot.toml").exists());

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

    let first = cli.run(&["project", "add", directory.to_str().unwrap()]);
    assert_eq!(first.status.code(), Some(0), "stderr: {}", stderr(&first));
    assert!(!directory.join(".depot.toml").exists());

    let second = cli.run(&["project", "add", directory.to_str().unwrap()]);

    assert_eq!(second.status.code(), Some(0), "stderr: {}", stderr(&second));
    assert!(
        stdout(&second).starts_with("already registered "),
        "stdout: {}",
        stdout(&second)
    );
    assert!(!directory.join(".depot.toml").exists());
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
        "Needs you - waiting on an answer",
        "Validated",
    ] {
        assert!(
            rendered.contains(&format!("## {label} (1)")),
            "missing {label} in\n{rendered}"
        );
    }
}

#[test]
fn a_failed_task_is_history_until_status_history_asks_for_it() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");
    let added = add_project(&cli.depot_home(), directory.to_str().unwrap()).expect("registered");
    let store = Store::open(&cli.depot_home()).expect("store");
    store
        .put_task(&task(
            added.project.id.as_str(),
            "t-1",
            TaskState::Failed,
            1,
        ))
        .expect("stored");

    let default = cli.run(&["status", "--all"]);
    assert_eq!(
        default.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&default)
    );
    assert!(!stdout(&default).contains("## Failed"));
    assert!(stdout(&default).contains("1 failed"));

    let history = cli.run(&["status", "--all", "--history"]);
    assert_eq!(
        history.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&history)
    );
    assert!(stdout(&history).contains("## Failed (1)"));
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

const BUILD_ONLY: &str = "base_branch = \"main\"\n\n\
                          [profiles]\n\
                          build = \"glm-5.3\"\n\n\
                          [validation]\n\
                          command = \"cargo test\"\n";

impl Cli {
    fn registered_with(&self, config: &str) -> depotd::Added {
        let directory = self.project_directory("example");
        std::fs::write(directory.join(".depot.toml"), config).expect("committed project config");
        add_project(&self.depot_home(), directory.to_str().unwrap()).expect("registered")
    }
}

#[test]
fn a_task_is_added_held_and_approved_from_the_command_line() {
    let cli = Cli::new();
    cli.registered_with(BUILD_ONLY);

    let added = cli.run(&[
        "task",
        "add",
        "--title",
        "Wire the store",
        "--intent",
        "Persist the records.",
        "--role",
        "build",
        "--project",
        "example",
    ]);

    assert_eq!(added.status.code(), Some(0), "stderr: {}", stderr(&added));
    assert_eq!(stdout(&added), "added t-1\n");

    let held = cli.run(&["status", "--project", "example"]);
    assert!(
        stdout(&held).contains("Held - awaiting approval (1)"),
        "a new task lands held, got\n{}",
        stdout(&held)
    );

    let approved = cli.run(&["task", "approve", "t-1", "--project", "example"]);

    assert_eq!(
        approved.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&approved)
    );
    assert_eq!(stdout(&approved), "approved t-1\n");
    let running = cli.run(&["status", "--project", "example"]);
    assert!(
        stdout(&running).contains("Running (1)"),
        "an approved task starts, got\n{}",
        stdout(&running)
    );
}

#[test]
fn an_unmapped_role_is_refused_rather_than_defaulted() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);

    let output = cli.run(&[
        "task",
        "add",
        "--title",
        "Review it",
        "--intent",
        "Judge the change.",
        "--role",
        "review",
        "--project",
        "example",
    ]);

    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("`review`"), "got {message}");
    assert!(message.contains(".depot.toml"), "got {message}");
    assert!(message.contains("[profiles]"), "got {message}");
    assert!(
        Store::open(&cli.depot_home())
            .expect("store")
            .tasks(&added.project.id)
            .expect("tasks")
            .is_empty(),
        "a refused role leaves no task behind"
    );
}

#[test]
fn a_question_is_answered_and_a_task_is_stopped_from_the_command_line() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    store
        .apply_fact(
            &added.project,
            "task_proposed:t-1",
            &depot_core::Fact {
                at: depot_core::Timestamp::from_millis(1_000),
                kind: depot_core::FactKind::TaskProposed {
                    task: TaskId::new("t-1"),
                    title: "Wire the store".to_string(),
                    intent: "Persist the records.".to_string(),
                    role: depot_core::Role::Build,
                    dispatch_profile: None,
                    dependencies: Vec::new(),
                    base_dependency: None,
                    hold_pr: false,
                },
            },
        )
        .expect("proposed");
    store
        .apply_fact(
            &added.project,
            "task_approved:t-1",
            &depot_core::Fact {
                at: depot_core::Timestamp::from_millis(2_000),
                kind: depot_core::FactKind::TaskApproved {
                    task: TaskId::new("t-1"),
                },
            },
        )
        .expect("approved");
    store
        .apply_fact(
            &added.project,
            "question_asked:t-1:0",
            &depot_core::Fact {
                at: depot_core::Timestamp::from_millis(3_000),
                kind: depot_core::FactKind::QuestionAsked {
                    task: TaskId::new("t-1"),
                    text: "Per project or per task?".to_string(),
                    relay: true,
                },
            },
        )
        .expect("asked");

    let waiting = cli.run(&["status", "--project", "example"]);
    assert!(
        stdout(&waiting).contains("Needs you - waiting on an answer (1)"),
        "got\n{}",
        stdout(&waiting)
    );

    let answered = cli.run(&[
        "task",
        "answer",
        "t-1",
        "--text",
        "Per project.",
        "--by",
        "user",
        "--project",
        "example",
    ]);
    assert_eq!(
        answered.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&answered)
    );
    assert_eq!(stdout(&answered), "answered t-1\n");

    let task = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(
        task.questions
            .last()
            .expect("question")
            .answer
            .as_ref()
            .map(|answer| answer.text.as_str()),
        Some("Per project."),
        "the answer joins the record"
    );
    assert_eq!(
        task.questions.last().unwrap().answer.as_ref().unwrap().by,
        depot_core::AnsweredBy::User,
        "the record says who answered"
    );

    let stopped = cli.run(&["task", "stop", "t-1", "--project", "example"]);
    assert_eq!(
        stopped.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&stopped)
    );
    assert_eq!(stdout(&stopped), "stopped t-1\n");
    let task = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(task.state, TaskState::Cancelled);

    let stopped_again = cli.run(&["task", "stop", "t-1", "--project", "example"]);
    assert_eq!(
        stopped_again.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&stopped_again)
    );
    assert_eq!(stdout(&stopped_again), "stopped t-1\n");
    let task = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(task.state, TaskState::Cancelled);
    assert_eq!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .filter(|event| event.kind == "task_cancelled")
            .count(),
        1,
        "a repeated stop does not journal again"
    );
}

#[test]
fn stopping_a_landed_task_prints_refusal_and_leaves_the_record() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    store
        .put_task(&task(
            added.project.id.as_str(),
            "t-1",
            TaskState::Landed,
            1_000,
        ))
        .expect("seeded landed");

    let output = cli.run(&["task", "stop", "t-1", "--project", "example"]);

    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("t-1"), "got {message}");
    assert!(message.contains("landed"), "got {message}");
    assert!(message.contains("stopped"), "got {message}");
    assert_eq!(stdout(&output), "");

    let held = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(held.state, TaskState::Landed);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "task_cancelled")
    );
}

#[test]
fn approving_a_running_task_prints_refusal_and_leaves_the_record() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    store
        .put_task(&task(
            added.project.id.as_str(),
            "t-1",
            TaskState::Running,
            1_000,
        ))
        .expect("seeded running");

    let output = cli.run(&["task", "approve", "t-1", "--project", "example"]);

    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("t-1"), "got {message}");
    assert!(message.contains("running"), "got {message}");
    assert!(message.contains("approved"), "got {message}");
    assert_eq!(stdout(&output), "");

    let held = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(held.state, TaskState::Running);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "task_approved")
    );
}

#[test]
fn answering_with_no_open_question_prints_refusal_and_leaves_the_record() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    store
        .put_task(&task(
            added.project.id.as_str(),
            "t-1",
            TaskState::Proposed,
            1_000,
        ))
        .expect("seeded proposed");

    let output = cli.run(&[
        "task",
        "answer",
        "t-1",
        "--text",
        "nothing open",
        "--project",
        "example",
    ]);

    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("t-1"), "got {message}");
    assert!(message.contains("proposed"), "got {message}");
    assert!(message.contains("no unanswered question"), "got {message}");
    assert_eq!(stdout(&output), "");

    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "question_answered")
    );
}

#[test]
fn the_inbox_prints_the_facts_since_the_last_turn_and_advances() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let output = cli.run(&[
        "task",
        "add",
        "--title",
        "Wire the store",
        "--intent",
        "Persist the records.",
        "--role",
        "build",
        "--project",
        "example",
    ]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let first = cli.run(&["inbox", "--project", "example"]);

    assert_eq!(first.status.code(), Some(0), "stderr: {}", stderr(&first));
    let payload = stdout(&first);
    assert!(payload.starts_with("# Inbox\n"), "got\n{payload}");
    assert!(payload.contains("Wire the store"), "got\n{payload}");
    assert!(payload.contains("holding for approval"), "got\n{payload}");

    let second = cli.run(&["inbox", "--project", "example"]);

    assert_eq!(second.status.code(), Some(0), "stderr: {}", stderr(&second));
    assert_eq!(
        stdout(&second),
        "# Inbox\n\nNo facts since your last turn.\n",
        "the same turn does not read the same facts twice"
    );

    let store = Store::open(&cli.depot_home()).expect("store");
    let cursor = store.inbox_cursor(&added.project.id).expect("cursor");
    assert_eq!(
        cursor,
        store
            .events(&added.project.id)
            .expect("events")
            .last()
            .expect("a fact")
            .id
    );
}

#[test]
fn a_narrative_document_is_written_into_the_store_and_a_foreign_name_is_refused() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);

    let written = cli.run(&[
        "doc",
        "write",
        "plan.md",
        "--content",
        "Step one.",
        "--project",
        "example",
    ]);

    assert_eq!(
        written.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&written)
    );
    let path = added.home.documents_dir().join("plan.md");
    assert_eq!(
        std::fs::read_to_string(&path).expect("document"),
        "Step one."
    );
    assert!(stdout(&written).contains("plan.md"));

    let escaped = cli.run(&[
        "doc",
        "write",
        "../escape.md",
        "--content",
        "no",
        "--project",
        "example",
    ]);

    assert_eq!(
        escaped.status.code(),
        Some(1),
        "stdout: {}",
        stdout(&escaped)
    );
    assert!(
        !added.home.root().join("escape.md").exists(),
        "a document stays inside the store"
    );
}

#[test]
fn a_command_run_from_the_project_store_reads_that_project() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);

    let output = cli.run_from(&["status"], added.home.root());

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains(added.project.id.as_str()),
        "the store is where a coordinator works, got\n{}",
        stdout(&output)
    );
}

#[test]
fn the_new_commands_refuse_unknown_flags_and_missing_arguments() {
    let cli = Cli::new();

    for arguments in [
        vec!["task"],
        vec!["task", "frobnicate"],
        vec!["task", "add", "--title", "only a title"],
        vec!["task", "add", "--role", "build", "--nope"],
        vec!["task", "approve"],
        vec!["task", "answer", "t-1"],
        vec!["task", "stop"],
        vec!["doc"],
        vec!["doc", "write"],
        vec!["doc", "write", "plan.md"],
        vec!["inbox", "extra"],
        vec!["inbox", "--nope"],
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
fn worker_ask_records_a_relayed_question_from_explicit_context() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    let mut seeded = task(added.project.id.as_str(), "t-1", TaskState::Running, 1);
    seeded.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("session-1")),
        profile: depot_core::ProfileId::new("build"),
        worktree: Some(depot_core::WorktreeLease::new("attempt-1")),
        started_at: depot_core::Timestamp::from_millis(1),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        rebase: false,
    });
    store.put_task(&seeded).expect("seeded task");

    let output = cli.run_worker(
        &[
            "ask",
            "--task",
            "t-1",
            "--project",
            "example",
            "--relay",
            "Which API?",
        ],
        "t-1",
        "attempt-1",
    );

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let recorded = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("task")
        .expect("present");
    assert_eq!(recorded.state, TaskState::WaitingOnQuestion);
    assert_eq!(recorded.questions[0].text, "Which API?");
}

#[test]
fn worker_commentary_does_not_change_task_state() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    let mut seeded = task(added.project.id.as_str(), "t-1", TaskState::Running, 1);
    seeded.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("session-1")),
        profile: depot_core::ProfileId::new("build"),
        worktree: Some(depot_core::WorktreeLease::new("attempt-1")),
        started_at: depot_core::Timestamp::from_millis(1),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        rebase: false,
    });
    store.put_task(&seeded).expect("seeded task");

    let output = cli.run_worker(
        &[
            "doc",
            "write",
            "notes.md",
            "--content",
            "Worker commentary.",
            "--project",
            "example",
        ],
        "t-1",
        "attempt-1",
    );

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        store
            .task(&added.project.id, &TaskId::new("t-1"))
            .expect("task")
            .expect("present"),
        seeded
    );
    assert_eq!(
        std::fs::read_to_string(added.home.documents_dir().join("notes.md")).expect("commentary"),
        "Worker commentary."
    );
}

#[test]
fn worker_context_refuses_coordinator_state_commands() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    let mut seeded = task(added.project.id.as_str(), "t-1", TaskState::Running, 1);
    seeded.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("session-1")),
        profile: depot_core::ProfileId::new("build"),
        worktree: Some(depot_core::WorktreeLease::new("attempt-1")),
        started_at: depot_core::Timestamp::from_millis(1),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        rebase: false,
    });
    store.put_task(&seeded).expect("seeded task");

    let output = cli.run_worker(
        &["task", "stop", "t-1", "--project", "example"],
        "t-1",
        "attempt-1",
    );

    assert_eq!(output.status.code(), Some(1), "stdout: {}", stdout(&output));
    assert!(
        stderr(&output).contains("depot ask"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("depot submit"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(
        store
            .task(&added.project.id, &TaskId::new("t-1"))
            .expect("task")
            .expect("present"),
        seeded
    );
}

#[test]
fn worker_submit_records_its_summary_artifacts_and_starts_validation() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    let mut seeded = task(added.project.id.as_str(), "t-1", TaskState::Running, 1);
    seeded.attempts.push(depot_core::Attempt {
        session: Some(depot_core::SessionId::new("session-1")),
        profile: depot_core::ProfileId::new("build"),
        worktree: Some(depot_core::WorktreeLease::new("attempt-1")),
        started_at: depot_core::Timestamp::from_millis(1),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
        rebase: false,
    });
    store.put_task(&seeded).expect("seeded task");

    let worktree = cli.project_directory("worktree");
    for arguments in [
        vec!["init"],
        vec!["config", "user.email", "worker@example.test"],
        vec!["config", "user.name", "Worker"],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&worktree)
            .output()
            .expect("git runs");
        assert!(output.status.success());
    }
    std::fs::write(worktree.join("result.txt"), "done\n").expect("work result");
    let output = Command::new("git")
        .args(["add", "result.txt"])
        .current_dir(&worktree)
        .output()
        .expect("git runs");
    assert!(output.status.success());
    let output = Command::new("git")
        .args(["commit", "-m", "finish task"])
        .current_dir(&worktree)
        .output()
        .expect("git runs");
    assert!(output.status.success());

    let output = cli.run_worker_from(
        &["submit", "--task", "t-1", "--project", "example"],
        "t-1",
        "attempt-1",
        &worktree,
    );

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "submitted t-1\n");
    let recorded = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("task")
        .expect("present");
    assert_eq!(recorded.state, TaskState::Validating);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .any(|event| event.kind == "worker_submitted"),
        "submission starts the validation path"
    );
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
        dispatch_profile: None,
        state,
        dependencies: Vec::new(),
        base_dependency: None,
        attempts: Vec::new(),
        questions: Vec::new(),
        validations: Vec::new(),
        submission: None,
        artifacts: Vec::new(),
        links: Vec::new(),
        branch_head: None,
        merge_refused: None,
        acknowledged_at: None,
        hold_pr: false,
        retry: None,
        created_at: at,
        updated_at: at,
    }
}

#[test]
fn an_acknowledged_failed_task_stays_hidden_until_history_asks_for_it() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");
    let added = add_project(&cli.depot_home(), directory.to_str().unwrap()).expect("registered");
    let store = Store::open(&cli.depot_home()).expect("store");
    store
        .put_task(&task(
            added.project.id.as_str(),
            "t-1",
            TaskState::Failed,
            1,
        ))
        .expect("stored");

    let before = cli.run(&["task", "acknowledge", "t-1", "--project", "example"]);
    assert_eq!(before.status.code(), Some(0), "stderr: {}", stderr(&before));

    let faded = cli.run(&["status", "--project", "example"]);
    assert_eq!(faded.status.code(), Some(0), "stderr: {}", stderr(&faded));
    assert!(!stdout(&faded).contains("## Failed"));
    assert!(stdout(&faded).contains("1 failed"));

    let history = cli.run(&["status", "--project", "example", "--history"]);
    assert_eq!(
        history.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&history)
    );
    assert!(stdout(&history).contains("Failed (1)"));
}

#[test]
fn acknowledging_a_running_task_is_refused() {
    let cli = Cli::new();
    let directory = cli.project_directory("example");
    let added = add_project(&cli.depot_home(), directory.to_str().unwrap()).expect("registered");
    let store = Store::open(&cli.depot_home()).expect("store");
    store
        .put_task(&task(
            added.project.id.as_str(),
            "t-1",
            TaskState::Running,
            1,
        ))
        .expect("stored");

    let output = cli.run(&["task", "acknowledge", "t-1", "--project", "example"]);

    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("cannot be"));
}

#[test]
fn a_task_holds_its_pull_request_until_release() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);

    let output = cli.run(&[
        "task",
        "add",
        "--title",
        "Sketch the shape",
        "--intent",
        "Stop at the artifact choice.",
        "--role",
        "build",
        "--hold-pr",
        "--project",
        "example",
    ]);

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "added t-1\n");

    let store = Store::open(&cli.depot_home()).expect("store");
    let project = added.project.clone();
    let held = store.task(&project.id, &TaskId::new("t-1")).expect("task");
    assert!(held.expect("t-1").hold_pr, "--hold-pr persists on the task");

    let premature = cli.run(&["task", "release", "t-1", "--project", "example"]);
    assert_eq!(
        premature.status.code(),
        Some(1),
        "a proposed task cannot be released"
    );

    let seeding = store
        .apply_fact(
            &project,
            "task_approved:t-1",
            &depot_core::Fact {
                at: depot_core::Timestamp::from_millis(2_000),
                kind: depot_core::FactKind::TaskApproved {
                    task: TaskId::new("t-1"),
                },
            },
        )
        .expect("approved");
    assert!(!seeding.actions.is_empty());

    let released = cli.run(&["task", "release", "t-1", "--project", "example"]);
    assert_eq!(
        released.status.code(),
        Some(1),
        "an approved task cannot be released, only a validated held one"
    );
}

#[test]
fn releasing_a_validated_held_task_is_a_no_op_after_the_fact() {
    let cli = Cli::new();
    let added = cli.registered_with(BUILD_ONLY);
    let store = Store::open(&cli.depot_home()).expect("store");
    let project = added.project.clone();
    store
        .apply_fact(
            &project,
            "task_proposed:t-1",
            &depot_core::Fact {
                at: depot_core::Timestamp::from_millis(1_000),
                kind: depot_core::FactKind::TaskProposed {
                    task: TaskId::new("t-1"),
                    title: "Design the seam".to_string(),
                    intent: "Stop for a human pick.".to_string(),
                    role: depot_core::Role::Build,
                    dispatch_profile: None,
                    dependencies: Vec::new(),
                    base_dependency: None,
                    hold_pr: true,
                },
            },
        )
        .expect("proposed");

    let released = cli.run(&["task", "release", "t-1", "--project", "example"]);
    assert_eq!(
        released.status.code(),
        Some(1),
        "a validated held task needs the daemon, so a proposed one is refused"
    );
}
