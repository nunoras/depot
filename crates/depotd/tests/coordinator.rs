mod support;

use depot_core::{Action, Fact, FactKind, SessionId, Task, TaskId, Timestamp};
use depotd::{COORDINATOR_POLICY, CoordinatorContext, Settings, Store, render_template};

const BUILD_ONLY: &str = "base_branch = \"main\"\n\n\
                          [profiles]\n\
                          build = \"glm-5.3\"\n\n\
                          [validation]\n\
                          command = \"cargo test\"\n";

fn at(millis: u64) -> Timestamp {
    Timestamp::from_millis(millis)
}

fn fact(millis: u64, kind: FactKind) -> Fact {
    Fact {
        at: at(millis),
        kind,
    }
}

fn context(fixture: &support::Fixture, name: &str) -> (Store, CoordinatorContext) {
    let added = support::register_with_config(fixture, name, BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    let context = store.coordinator_context(&added.project).expect("context");
    (store, context)
}

fn text(path: &std::path::Path) -> String {
    path.display().to_string()
}

#[test]
fn the_daemon_ships_the_policy_prompt_with_the_kickoff_of_a_new_session() {
    let fixture = support::fixture();
    let (_store, context) = context(&fixture, "example");

    let launch = context.launch().expect("launch material");

    assert_eq!(launch.policy, COORDINATOR_POLICY);
    assert!(
        !launch.policy.trim().is_empty(),
        "a session is shipped the policy prompt, not an empty string"
    );
    assert!(launch.kickoff.contains(&context.project.slug));
    assert!(
        launch.kickoff.contains(&text(&context.checklist_path())),
        "the kickoff points at the live checklist, got\n{}",
        launch.kickoff
    );
    assert!(
        launch
            .kickoff
            .contains(&text(&context.context_document_path())),
        "the kickoff points at the project context document, got\n{}",
        launch.kickoff
    );
    assert!(launch.kickoff.contains("depot inbox"));
}

#[test]
fn a_coordinator_session_is_rotated_past_the_configured_size_and_the_fresh_one_names_both_documents()
 {
    let fixture = support::fixture();
    let (_store, context) = context(&fixture, "example");
    fixture
        .home
        .write_settings(&Settings {
            coordinator_context_tokens: 100,
            ..Settings::default()
        })
        .expect("settings");

    let store = Store::open(&fixture.home).expect("store");
    let project = &context.project;

    let started = fact(
        1_000,
        FactKind::CoordinatorSessionStarted {
            session: SessionId::new("c1"),
        },
    );
    let applied = store
        .apply_fact(project, "coordinator:c1", &started)
        .expect("a session started");
    assert_eq!(applied.actions, Vec::new());
    assert_eq!(
        store
            .coordinator_session(&project.id)
            .expect("session")
            .expect("recorded")
            .session
            .as_str(),
        "c1"
    );

    let measured = fact(2_000, FactKind::CoordinatorContextMeasured { tokens: 99 });
    let applied = store
        .apply_fact(project, "coordinator:c1:99", &measured)
        .expect("below the limit");
    assert_eq!(
        applied.actions,
        Vec::new(),
        "below the limit nothing happens"
    );

    let measured = fact(3_000, FactKind::CoordinatorContextMeasured { tokens: 100 });
    let applied = store
        .apply_fact(project, "coordinator:c1:100", &measured)
        .expect("past the limit");
    assert_eq!(
        applied.actions,
        vec![Action::RotateCoordinator {
            session: SessionId::new("c1"),
        }],
        "the daemon stops the old session and starts a fresh one"
    );
    assert_eq!(
        store.coordinator_session(&project.id).expect("session"),
        None,
        "a rotated project holds no live session"
    );

    let fresh = fact(
        4_000,
        FactKind::CoordinatorSessionStarted {
            session: SessionId::new("c2"),
        },
    );
    store
        .apply_fact(project, "coordinator:c2", &fresh)
        .expect("a fresh session");

    let context = store.coordinator_context(project).expect("context");
    assert_eq!(
        store
            .coordinator_session(&project.id)
            .expect("session")
            .expect("recorded")
            .session
            .as_str(),
        "c2"
    );
    let launch = context.launch().expect("launch material");
    assert!(launch.kickoff.contains(&text(&context.checklist_path())));
    assert!(
        launch
            .kickoff
            .contains(&text(&context.context_document_path())),
        "the fresh session's kickoff names the checklist and the context document"
    );
}

#[test]
fn a_coordinator_row_without_a_session_keeps_the_inbox_cursor() {
    let fixture = support::fixture();
    let (store, context) = context(&fixture, "example");

    store
        .set_inbox_cursor(&context.project.id, 7)
        .expect("cursor");
    assert_eq!(store.inbox_cursor(&context.project.id).expect("cursor"), 7);
    assert_eq!(
        store
            .coordinator_session(&context.project.id)
            .expect("session"),
        None
    );

    store
        .apply_fact(
            &context.project,
            "coordinator:c1",
            &fact(
                1_000,
                FactKind::CoordinatorSessionStarted {
                    session: SessionId::new("c1"),
                },
            ),
        )
        .expect("a session started");

    assert_eq!(
        store.inbox_cursor(&context.project.id).expect("cursor"),
        7,
        "starting a session never rewinds the coordinator's read position"
    );
}

#[test]
fn the_brief_renders_from_a_task_record_without_repeating_the_coordinator() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "root",
            depot_core::TaskState::Landed,
            1_000,
        ))
        .expect("the prerequisite");
    let task = support::full_task(&added.project.id, "t-1");
    store.put_task(&task).expect("stored");
    let context = store.coordinator_context(&added.project).expect("context");

    let brief = context.brief(&task).expect("brief");
    let checklist = context.checklist_path();
    let scratch = context.home.scratch_dir();

    for expected in [
        "task t-1".to_string(),
        "review".to_string(),
        context.project.id.as_str().to_string(),
        text(context.home.root()),
        text(&checklist),
        text(&scratch),
        "A document in the store".to_string(),
        "A verdict written to the store".to_string(),
        "depot ask".to_string(),
        "depot submit".to_string(),
        "`root` **task root**".to_string(),
        "cargo test".to_string(),
    ] {
        assert!(
            brief.contains(&expected),
            "the brief must carry `{expected}`, got\n{brief}"
        );
    }
    assert_eq!(
        brief.matches(&task.intent).count(),
        1,
        "the intent is stated once, not restated by the coordinator, got\n{brief}"
    );
    assert!(!brief.contains("{{"), "no placeholder may survive");
}

#[test]
fn a_brief_for_a_build_task_names_its_worktree_as_the_output_destination() {
    let fixture = support::fixture();
    let (_store, context) = context(&fixture, "example");
    let mut task = support::simple_task(
        &context.project.id,
        "t-1",
        depot_core::TaskState::Running,
        1_000,
    );
    task.role = depot_core::Role::Build;
    task.attempts.push(depot_core::Attempt {
        session: Some(SessionId::new("s1")),
        profile: depot_core::ProfileId::new("glm-5.3"),
        worktree: Some(depot_core::WorktreeLease::new("lease-7")),
        started_at: at(1_000),
        finished_at: None,
        outcome: depot_core::AttemptOutcome::InFlight,
    });

    let brief = context.brief(&task).expect("brief");

    assert!(
        brief.contains("worktree, lease `lease-7`"),
        "a build brief names the worktree it commits into, got\n{brief}"
    );
    assert!(brief.contains("Nothing pinned"));
}

#[test]
fn a_template_placeholder_nothing_fills_is_refused() {
    let error = render_template("hello {{nobody}}", &[("title", "x")]).expect_err("unfilled");

    assert!(
        error.to_string().contains("nobody"),
        "the refusal names the placeholder, got {error}"
    );
}

#[test]
fn nothing_delivered_to_a_session_carries_the_configured_profile() {
    let fixture = support::fixture();
    let added = support::register_with_config(
        &fixture,
        "example",
        "base_branch = \"main\"\n\n\
         [profiles]\n\
         plan = \"fable-5\"\n\
         build = \"glm-5.3\"\n",
    );
    let store = Store::open(&fixture.home).expect("store");
    let context = store.coordinator_context(&added.project).expect("context");
    let task = support::full_task(&added.project.id, "t-1");

    let launch = context.launch().expect("launch material");
    let brief = context.brief(&task).expect("brief");

    for profile in ["fable-5", "glm-5.3"] {
        assert!(
            !launch.policy.contains(profile),
            "the profile comes from project config, not from the policy prompt"
        );
        assert!(
            !launch.kickoff.contains(profile),
            "the profile comes from project config, not from the kickoff"
        );
        assert!(
            !brief.contains(profile),
            "a worker's brief names its work, not the boxr profile that launched it"
        );
    }
}

#[test]
fn the_unmapped_role_refusal_names_the_role_the_project_and_the_file() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");

    let error = depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Review it".to_string(),
            intent: "Judge the change.".to_string(),
            role: "review".to_string(),
            dependencies: Vec::new(),
            base_dependency: None,
        },
    )
    .expect_err("an unmapped role is refused");

    let message = error.to_string();
    assert!(message.contains("`review`"), "got {message}");
    assert!(message.contains(".depot.toml"), "got {message}");
    assert!(message.contains("[profiles]"), "got {message}");
    assert!(
        store.tasks(&added.project.id).expect("tasks").is_empty(),
        "a refused task leaves no record"
    );
}

#[test]
fn a_multi_dependency_add_without_a_base_leaves_no_trace() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");

    let error = depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Join two".to_string(),
            intent: "Depend on both sides.".to_string(),
            role: "build".to_string(),
            dependencies: vec!["t-a@aaa".to_string(), "t-b@bbb".to_string()],
            base_dependency: None,
        },
    )
    .expect_err("multiple dependencies without a base are refused");

    let message = error.to_string();
    assert!(
        message.contains("--base-dependency"),
        "refusal names the missing flag, got {message}"
    );
    assert!(
        store.tasks(&added.project.id).expect("tasks").is_empty(),
        "a refused add leaves no task"
    );
    assert!(
        store.events(&added.project.id).expect("events").is_empty(),
        "a refused add leaves no journal entry"
    );

    let task = depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Wire the store".to_string(),
            intent: "Persist the records.".to_string(),
            role: "build".to_string(),
            dependencies: Vec::new(),
            base_dependency: None,
        },
    )
    .expect("a corrected add still works");

    assert_eq!(task.id.as_str(), "t-1");
    assert_eq!(task.state, depot_core::TaskState::Proposed);
}

#[test]
fn a_task_added_through_the_command_surface_lands_held() {
    let fixture = support::fixture();
    support::register_with_config(&fixture, "example", BUILD_ONLY);

    let task = depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Wire the store".to_string(),
            intent: "Persist the records.".to_string(),
            role: "build".to_string(),
            dependencies: Vec::new(),
            base_dependency: None,
        },
    )
    .expect("added");

    assert_eq!(task.id.as_str(), "t-1");
    assert_eq!(task.state, depot_core::TaskState::Proposed);
    assert_eq!(task.role, depot_core::Role::Build);

    let second = depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Then the loop".to_string(),
            intent: "Drive facts to actions.".to_string(),
            role: "build".to_string(),
            dependencies: vec!["t-1@abc123".to_string()],
            base_dependency: None,
        },
    )
    .expect("added");

    assert_eq!(second.id.as_str(), "t-2");
    assert_eq!(
        second.dependencies,
        vec![depot_core::Dependency {
            task: TaskId::new("t-1"),
            commit: depot_core::CommitId::new("abc123"),
        }]
    );
}

#[test]
fn answering_two_open_questions_records_two_facts() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");

    depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Wire the store".to_string(),
            intent: "Persist the records.".to_string(),
            role: "build".to_string(),
            dependencies: Vec::new(),
            base_dependency: None,
        },
    )
    .expect("added");
    depotd::approve_tasks(&fixture.home, Some("example"), &["t-1".to_string()]).expect("approved");

    for (index, text) in ["first open?", "second open?"].into_iter().enumerate() {
        store
            .apply_fact(
                &added.project,
                &format!("question_asked:t-1:{index}"),
                &fact(
                    1_000 + index as u64,
                    FactKind::QuestionAsked {
                        task: TaskId::new("t-1"),
                        text: text.to_string(),
                        relay: true,
                    },
                ),
            )
            .expect("asked");
    }

    let first = depotd::answer_question(
        &fixture.home,
        Some("example"),
        "t-1",
        "answer the later one",
        "coordinator",
    )
    .expect("first answer");
    assert_eq!(
        first.questions[1].answer.as_ref().map(|answer| answer.text.as_str()),
        Some("answer the later one")
    );
    assert!(first.questions[0].answer.is_none());

    let second = depotd::answer_question(
        &fixture.home,
        Some("example"),
        "t-1",
        "answer the earlier one",
        "user",
    )
    .expect("second answer");
    assert!(second.questions.iter().all(|question| question.answer.is_some()));
    assert_eq!(
        second.questions[0].answer.as_ref().map(|answer| answer.text.as_str()),
        Some("answer the earlier one")
    );
    assert_eq!(
        second.questions[1].answer.as_ref().map(|answer| answer.text.as_str()),
        Some("answer the later one")
    );

    let answered: Vec<_> = store
        .events(&added.project.id)
        .expect("events")
        .into_iter()
        .filter(|event| event.kind == "question_answered")
        .map(|event| event.key)
        .collect();
    assert_eq!(
        answered,
        vec![
            "question_answered:t-1:1".to_string(),
            "question_answered:t-1:0".to_string(),
        ]
    );
}

#[test]
fn approving_a_cancelled_task_is_refused_without_writing() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");

    depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Wire the store".to_string(),
            intent: "Persist the records.".to_string(),
            role: "build".to_string(),
            dependencies: Vec::new(),
            base_dependency: None,
        },
    )
    .expect("added");
    depotd::stop_task(&fixture.home, Some("example"), "t-1").expect("stopped");

    let error = depotd::approve_tasks(&fixture.home, Some("example"), &["t-1".to_string()])
        .expect_err("a cancelled task cannot be approved");
    let message = error.to_string();
    assert!(message.contains("t-1"), "got {message}");
    assert!(message.contains("cancelled"), "got {message}");

    let task = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(task.state, depot_core::TaskState::Cancelled);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "task_approved"),
        "a refused approval leaves no approve journal entry"
    );
}

#[test]
fn approving_an_already_approved_task_is_a_successful_noop() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            depot_core::TaskState::Approved,
            1_000,
        ))
        .expect("seeded approved");

    let approved = depotd::approve_tasks(&fixture.home, Some("example"), &["t-1".to_string()])
        .expect("already approved stays a success");

    assert_eq!(approved[0].state, depot_core::TaskState::Approved);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "task_approved"),
        "a no-op approval leaves no journal entry"
    );
}

#[test]
fn stopping_a_landed_task_is_refused_without_writing() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            depot_core::TaskState::Landed,
            1_000,
        ))
        .expect("seeded landed");

    let error = depotd::stop_task(&fixture.home, Some("example"), "t-1")
        .expect_err("a landed task cannot be stopped");
    let message = error.to_string();
    assert!(message.contains("t-1"), "got {message}");
    assert!(message.contains("landed"), "got {message}");
    assert!(message.contains("stopped"), "got {message}");

    let task = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(task.state, depot_core::TaskState::Landed);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "task_cancelled"),
        "a refused stop leaves no cancel journal entry"
    );
}

#[test]
fn stopping_an_already_cancelled_task_is_a_successful_noop() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            depot_core::TaskState::Cancelled,
            1_000,
        ))
        .expect("seeded cancelled");

    let stopped = depotd::stop_task(&fixture.home, Some("example"), "t-1")
        .expect("already cancelled stays a success");

    assert_eq!(stopped.state, depot_core::TaskState::Cancelled);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "task_cancelled"),
        "a no-op stop leaves no journal entry"
    );
}

#[test]
fn answering_with_no_open_question_is_refused_without_writing() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");

    depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Wire the store".to_string(),
            intent: "Persist the records.".to_string(),
            role: "build".to_string(),
            dependencies: Vec::new(),
            base_dependency: None,
        },
    )
    .expect("added");

    let error = depotd::answer_question(
        &fixture.home,
        Some("example"),
        "t-1",
        "nothing to answer",
        "coordinator",
    )
    .expect_err("no open question means refuse");
    let message = error.to_string();
    assert!(message.contains("t-1"), "got {message}");
    assert!(message.contains("proposed"), "got {message}");
    assert!(message.contains("no unanswered question"), "got {message}");

    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "question_answered"),
        "a refused answer leaves no journal entry"
    );
}

#[test]
fn a_refused_approval_leaves_the_whole_batch_unwritten() {
    let fixture = support::fixture();
    let added = support::register_with_config(&fixture, "example", BUILD_ONLY);
    let store = Store::open(&fixture.home).expect("store");

    for title in ["First", "Second"] {
        depotd::add_task(
            &fixture.home,
            Some("example"),
            &depotd::TaskRequest {
                title: title.to_string(),
                intent: format!("{title} intent."),
                role: "build".to_string(),
                dependencies: Vec::new(),
                base_dependency: None,
            },
        )
        .expect("added");
    }
    depotd::stop_task(&fixture.home, Some("example"), "t-2").expect("stopped");

    let error = depotd::approve_tasks(
        &fixture.home,
        Some("example"),
        &["t-1".to_string(), "t-2".to_string()],
    )
    .expect_err("one refused id refuses the batch");
    assert!(error.to_string().contains("t-2"), "got {error}");

    let first = store
        .task(&added.project.id, &TaskId::new("t-1"))
        .expect("read")
        .expect("present");
    assert_eq!(first.state, depot_core::TaskState::Proposed);
    assert!(
        store
            .events(&added.project.id)
            .expect("events")
            .iter()
            .all(|event| event.kind != "task_approved"),
        "a refused batch leaves no approve journal entry"
    );
}

#[test]
fn approving_a_task_records_the_actions_the_daemon_will_take() {
    let fixture = support::fixture();
    let (_store, context) = context(&fixture, "example");
    let store = Store::open(&fixture.home).expect("store");
    let task: Task = depotd::add_task(
        &fixture.home,
        Some("example"),
        &depotd::TaskRequest {
            title: "Wire the store".to_string(),
            intent: "Persist the records.".to_string(),
            role: "build".to_string(),
            dependencies: Vec::new(),
            base_dependency: None,
        },
    )
    .expect("added");

    let approved = depotd::approve_tasks(&fixture.home, Some("example"), &["t-1".to_string()])
        .expect("approved");

    assert_eq!(approved[0].state, depot_core::TaskState::Running);
    let applied = store
        .apply_fact(
            &context.project,
            "task_approved:t-1",
            &fact(
                2_000,
                FactKind::TaskApproved {
                    task: TaskId::new("t-1"),
                },
            ),
        )
        .expect("a replayed approval is idempotent");
    assert_eq!(
        applied.outcome,
        depotd::EventOutcome::Duplicate,
        "the key the command line wrote is the key the daemon replays"
    );
    assert_eq!(
        store
            .task(&context.project.id, &task.id)
            .expect("read")
            .unwrap(),
        approved[0]
    );
}
