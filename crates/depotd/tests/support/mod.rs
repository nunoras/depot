#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use depot_core::{
    Answer, AnsweredBy, Artifact, ArtifactKind, Attempt, AttemptOutcome, Checks, CommitId,
    Dependency, Limits, Link, ProfileId, ProjectId, ProjectState, Question, Retry, Role, SessionId,
    Task, TaskId, TaskState, Timestamp, ValidationRecord, WorktreeLease,
};
use depotd::{Added, DepotHome, ProfileSettings, Settings, add_project};
use tempfile::TempDir;

pub const ALL_STATES: [TaskState; 10] = [
    TaskState::Proposed,
    TaskState::Approved,
    TaskState::Running,
    TaskState::WaitingOnQuestion,
    TaskState::Validating,
    TaskState::Validated,
    TaskState::PrOpen,
    TaskState::Landed,
    TaskState::Failed,
    TaskState::Cancelled,
];

pub struct Fixture {
    pub temp: TempDir,
    pub home: DepotHome,
}

pub fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temporary directory");
    let home = DepotHome::at(temp.path().join("depot-home"));
    home.ensure().expect("depot home");
    home.write_settings(&Settings {
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
    })
    .expect("settings");
    Fixture { temp, home }
}

pub fn project_directory(fixture: &Fixture, name: &str) -> PathBuf {
    let directory = fixture.temp.path().join(name);
    std::fs::create_dir_all(&directory).expect("project directory");
    directory
}

pub fn register(fixture: &Fixture, name: &str) -> Added {
    let directory = project_directory(fixture, name);
    add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered project")
}

pub fn register_with_config(fixture: &Fixture, name: &str, config: &str) -> Added {
    let directory = project_directory(fixture, name);
    std::fs::write(directory.join(depotd::PROJECT_CONFIG_FILE_NAME), config)
        .expect("project config");
    add_project(&fixture.home, directory.to_str().expect("utf-8 path")).expect("registered project")
}

pub fn state(slug: &str) -> TaskState {
    match slug {
        "proposed" => TaskState::Proposed,
        "approved" => TaskState::Approved,
        "running" => TaskState::Running,
        "waiting_on_question" => TaskState::WaitingOnQuestion,
        "validating" => TaskState::Validating,
        "validated" => TaskState::Validated,
        "pr_open" => TaskState::PrOpen,
        "landed" => TaskState::Landed,
        "failed" => TaskState::Failed,
        "cancelled" => TaskState::Cancelled,
        other => panic!("unknown state {other}"),
    }
}

pub fn simple_task(project: &ProjectId, id: &str, state: TaskState, created_at: u64) -> Task {
    Task {
        id: TaskId::new(id),
        project: project.clone(),
        title: format!("task {id}"),
        intent: format!("intent for {id}"),
        role: Role::Build,
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
        conflict_base: None,
        failure: None,
        redirect_text: None,
        redirect_delivered: false,
        acknowledged_at: None,
        hold_pr: false,
        release_pending: None,
        release_held: None,
        rework_of: None,
        retry: None,
        created_at: Timestamp::from_millis(created_at),
        updated_at: Timestamp::from_millis(created_at),
    }
}

pub fn full_task(project: &ProjectId, id: &str) -> Task {
    Task {
        id: TaskId::new(id),
        project: project.clone(),
        title: format!("task {id}"),
        intent: format!("intent for {id}"),
        role: Role::Review,
        dispatch_profile: None,
        state: TaskState::WaitingOnQuestion,
        dependencies: vec![
            Dependency {
                task: TaskId::new("root"),
                commit: CommitId::new("aaa111"),
            },
            Dependency {
                task: TaskId::new("sibling"),
                commit: CommitId::new("bbb222"),
            },
        ],
        base_dependency: Some(TaskId::new("root")),
        attempts: vec![
            Attempt {
                last_seen_at: None,
                session: Some(SessionId::new("session-1")),
                profile: ProfileId::new("glm-5.3"),
                worktree: Some(WorktreeLease::new("lease-1")),
                started_at: Timestamp::from_millis(1_700_000_000_000),
                finished_at: Some(Timestamp::from_millis(1_700_000_060_000)),
                outcome: AttemptOutcome::Failed,
                rebase: false,
            },
            Attempt {
                last_seen_at: None,
                session: None,
                profile: ProfileId::new("gpt-5.5"),
                worktree: Some(WorktreeLease::new("lease-2")),
                started_at: Timestamp::from_millis(1_700_000_120_000),
                finished_at: None,
                outcome: AttemptOutcome::Unknown,
                rebase: false,
            },
        ],
        questions: vec![
            Question {
                text: "Answered question?".to_string(),
                asked_at: Timestamp::from_millis(1_700_000_001_000),
                answer: Some(Answer {
                    text: "Yes".to_string(),
                    by: AnsweredBy::Coordinator,
                    at: Timestamp::from_millis(1_700_000_002_000),
                }),
            },
            Question {
                text: "Open question?\nwith a newline".to_string(),
                asked_at: Timestamp::from_millis(1_700_000_003_000),
                answer: None,
            },
        ],
        validations: vec![ValidationRecord {
            command: "cargo test".to_string(),
            commit: CommitId::new("aaa111"),
            base_commit: Some(CommitId::new("bbb222")),
            exit_code: 1,
            duration: Duration::from_millis(1234),
            output_tail: "boom".to_string(),
        }],
        submission: None,
        artifacts: vec![
            Artifact {
                kind: ArtifactKind::Brief,
                path: "scratch/brief.md".to_string(),
            },
            Artifact {
                kind: ArtifactKind::Evidence,
                path: "scratch/evidence.txt".to_string(),
            },
            Artifact {
                kind: ArtifactKind::Media,
                path: "media/shot.png".to_string(),
            },
        ],
        links: vec![
            Link::Issue {
                url: "https://example.test/issues/1".to_string(),
            },
            Link::PullRequest {
                number: 7,
                url: "https://example.test/pull/7".to_string(),
                checks: Checks::Failing,
            },
        ],
        branch_head: Some(CommitId::new("ccc333")),
        merge_refused: Some("the forge refused the merge".to_string()),
        conflict_base: None,
        failure: None,
        redirect_text: None,
        redirect_delivered: false,
        acknowledged_at: None,
        hold_pr: false,
        release_pending: None,
        release_held: None,
        rework_of: None,
        retry: Some(Retry {
            profile: ProfileId::new("sonnet"),
            not_before: Timestamp::from_millis(1_700_000_120_000),
        }),
        created_at: Timestamp::from_millis(1_699_999_000_000),
        updated_at: Timestamp::from_millis(1_700_000_060_000),
    }
}

pub fn varied_state() -> ProjectState {
    let project = ProjectId::new("example/project");
    let mut tasks = BTreeMap::new();
    for (index, task_state) in ALL_STATES.into_iter().enumerate() {
        for suffix in ["a", "b"] {
            let id = format!("t-{index}-{suffix}");
            let created_at = 1_700_000_000_000 + (index as u64) * 1000;
            tasks.insert(
                TaskId::new(id.clone()),
                simple_task(&project, &id, task_state, created_at),
            );
        }
    }
    ProjectState {
        project,
        slug: "example".to_string(),
        tasks,
        coordinator: None,
        profiles: BTreeMap::from([
            (Role::Plan, ProfileId::new("fable-5")),
            (Role::Build, ProfileId::new("glm-5.3")),
        ]),
        fallback_profiles: vec![ProfileId::new("gpt-5.5")],
        limits: Limits::default(),
        always_relay_questions: false,
        merge_policy: depot_core::MergePolicy::Manual,
    }
}
