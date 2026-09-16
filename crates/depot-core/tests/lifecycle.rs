use std::collections::BTreeMap;
use std::time::Duration;

use depot_core::*;

type Check = fn(&ProjectState) -> bool;

const BUILD: &str = "build-profile";
const PLAN: &str = "plan-profile";

fn at(millis: u64) -> Timestamp {
    Timestamp::from_millis(millis)
}

fn fact(millis: u64, kind: FactKind) -> Fact {
    Fact {
        at: at(millis),
        kind,
    }
}

fn commit(id: &str) -> CommitId {
    CommitId::from(id)
}

fn task_id(id: &str) -> TaskId {
    TaskId::from(id)
}

fn session(id: &str) -> SessionId {
    SessionId::from(id)
}

fn lease(id: &str) -> WorktreeLease {
    WorktreeLease::from(id)
}

fn profile(id: &str) -> ProfileId {
    ProfileId::from(id)
}

fn subject<'a>(state: &'a ProjectState, id: &str) -> &'a Task {
    state.tasks.get(&task_id(id)).expect("subject task")
}

fn holds(state: &ProjectState, id: &str, outcome: AttemptOutcome) -> bool {
    subject(state, id)
        .attempts
        .last()
        .is_some_and(|attempt| attempt.outcome == outcome)
}

fn task(id: &str, state: TaskState) -> Task {
    Task {
        id: task_id(id),
        project: ProjectId::from("depot"),
        title: format!("task {id}"),
        intent: format!("intent for {id}"),
        role: Role::Build,
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
        created_at: at(0),
        updated_at: at(0),
    }
}

fn attempt(profile: &str) -> Attempt {
    Attempt {
        session: None,
        profile: ProfileId::from(profile),
        worktree: None,
        started_at: at(0),
        finished_at: None,
        outcome: AttemptOutcome::InFlight,
    }
}

fn spent(profile: &str) -> Attempt {
    Attempt {
        outcome: AttemptOutcome::Failed,
        finished_at: Some(at(0)),
        ..attempt(profile)
    }
}

fn with_attempt(mut task: Task, attempt: Attempt) -> Task {
    task.attempts.push(attempt);
    task
}

fn running(id: &str) -> Task {
    with_attempt(task(id, TaskState::Running), attempt(BUILD))
}

fn running_with_session(id: &str, session_id: &str, lease_id: &str) -> Task {
    with_attempt(
        task(id, TaskState::Running),
        Attempt {
            session: Some(session(session_id)),
            worktree: Some(lease(lease_id)),
            ..attempt(BUILD)
        },
    )
}

fn validating(id: &str) -> Task {
    with_attempt(task(id, TaskState::Validating), attempt(BUILD))
}

fn validated(id: &str, commit_id: &str) -> Task {
    let mut task = task(id, TaskState::Validated);
    task.validations.push(ValidationRecord {
        command: "cargo test".to_owned(),
        commit: commit(commit_id),
        exit_code: 0,
        duration: Duration::from_secs(5),
        output_tail: "ok".to_owned(),
    });
    task
}

fn depending_on(mut task: Task, prerequisite: &str, commit_id: &str) -> Task {
    task.dependencies.push(Dependency {
        task: task_id(prerequisite),
        commit: commit(commit_id),
    });
    task
}

fn with_base(mut task: Task, prerequisite: &str) -> Task {
    task.base_dependency = Some(task_id(prerequisite));
    task
}

fn base() -> ProjectState {
    ProjectState {
        project: ProjectId::from("depot"),
        tasks: BTreeMap::new(),
        profiles: BTreeMap::from([(Role::Build, profile(BUILD)), (Role::Plan, profile(PLAN))]),
        fallback_profiles: Vec::new(),
        limits: Limits::default(),
        always_relay_questions: false,
    }
}

fn state(tasks: Vec<Task>) -> ProjectState {
    let mut state = base();
    for task in tasks {
        state.tasks.insert(task.id.clone(), task);
    }
    state
}

fn approved(title: &str) -> FactKind {
    FactKind::TaskApproved {
        task: task_id(title),
    }
}

fn passed(task: &str, commit_id: &str) -> FactKind {
    FactKind::ValidationFinished {
        task: task_id(task),
        command: "cargo test".to_owned(),
        commit: commit(commit_id),
        exit_code: 0,
        duration: Duration::from_secs(5),
        output_tail: "ok".to_owned(),
    }
}

fn failed(task: &str, commit_id: &str) -> FactKind {
    FactKind::ValidationFinished {
        task: task_id(task),
        command: "cargo test".to_owned(),
        commit: commit(commit_id),
        exit_code: 1,
        duration: Duration::from_secs(5),
        output_tail: "2 tests failed".to_owned(),
    }
}

fn submitted(task: &str, commit_id: &str) -> FactKind {
    FactKind::WorkerSubmitted {
        task: task_id(task),
        commit: commit(commit_id),
    }
}

fn merged(task: &str) -> FactKind {
    FactKind::PullRequestMerged {
        task: task_id(task),
        commit: commit("merge-commit"),
    }
}

fn queue(task: &str, not_before: Option<Timestamp>) -> Action {
    Action::Queue {
        task: task_id(task),
        not_before,
    }
}

fn acquire(task: &str, baseline: Baseline) -> Action {
    Action::AcquireWorktree {
        task: task_id(task),
        baseline,
    }
}

fn launch(task: &str, profile_id: &str) -> Action {
    Action::LaunchSession {
        task: task_id(task),
        profile: profile(profile_id),
    }
}

fn hold(task: &str) -> Action {
    Action::HoldForUser {
        task: task_id(task),
    }
}

fn release(task: &str, lease_id: &str) -> Action {
    Action::ReleaseWorktree {
        task: task_id(task),
        lease: lease(lease_id),
    }
}

struct Case {
    name: &'static str,
    subject: &'static str,
    start: ProjectState,
    facts: Vec<Fact>,
    want_state: TaskState,
    want_actions: Vec<Action>,
    check: Option<Check>,
}

fn case(name: &'static str, start: ProjectState, facts: Vec<Fact>) -> Case {
    Case {
        name,
        subject: "t1",
        start,
        facts,
        want_state: TaskState::Proposed,
        want_actions: Vec::new(),
        check: None,
    }
}

impl Case {
    fn when(
        mut self,
        subject: &'static str,
        want_state: TaskState,
        want_actions: Vec<Action>,
    ) -> Self {
        self.subject = subject;
        self.want_state = want_state;
        self.want_actions = want_actions;
        self
    }

    fn checking(mut self, check: Check) -> Self {
        self.check = Some(check);
        self
    }
}

fn run(cases: Vec<Case>) {
    for case in cases {
        let mut state = case.start.clone();
        let mut actions = Vec::new();
        for fact in &case.facts {
            let (next, produced) = reduce(&state, fact);
            state = next;
            actions = produced;
        }
        let task = subject(&state, case.subject);
        assert_eq!(task.state, case.want_state, "case {}: state", case.name);
        assert_eq!(actions, case.want_actions, "case {}: actions", case.name);
        if let Some(check) = case.check {
            assert!(check(&state), "case {}: extra check", case.name);
        }
    }
}

#[test]
fn rule_01_a_proposed_task_runs_only_after_approval() {
    run(vec![
        case(
            "a proposed task is not launched by a polling pass",
            state(vec![task("t1", TaskState::Proposed)]),
            vec![fact(1_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Proposed, vec![]),
        case(
            "approval is the fact that starts it",
            state(vec![task("t1", TaskState::Proposed)]),
            vec![fact(2_000, approved("t1"))],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                acquire("t1", Baseline::DefaultBranchHead),
                launch("t1", BUILD),
                Action::RenderChecklist,
            ],
        ),
        case(
            "approval is required even when every dependency is validated",
            state(vec![
                depending_on(task("t1", TaskState::Proposed), "t0", "c1"),
                validated("t0", "c1"),
            ]),
            vec![fact(3_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Proposed, vec![]),
    ]);
}

#[test]
fn rule_02_an_approved_task_starts_once_every_dependency_is_validated() {
    run(vec![
        case(
            "an unvalidated prerequisite queues the task",
            state(vec![
                depending_on(task("t1", TaskState::Proposed), "t0", "c1"),
                validating("t0"),
            ]),
            vec![fact(1_000, approved("t1"))],
        )
        .when(
            "t1",
            TaskState::Approved,
            vec![queue("t1", None), Action::RenderChecklist],
        ),
        case(
            "the task starts when the prerequisite validates its pinned commit",
            state(vec![
                depending_on(task("t1", TaskState::Approved), "t0", "c1"),
                validating("t0"),
            ]),
            vec![fact(2_000, passed("t0", "c1"))],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                Action::Push {
                    task: task_id("t0"),
                    commit: commit("c1"),
                },
                Action::OpenPullRequest {
                    task: task_id("t0"),
                    commit: commit("c1"),
                },
                acquire("t1", Baseline::PinnedCommit(commit("c1"))),
                launch("t1", BUILD),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| subject(state, "t1").attempts.len() == 1),
        case(
            "a task with no dependencies starts on approval",
            state(vec![task("t1", TaskState::Proposed)]),
            vec![fact(3_000, approved("t1"))],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                acquire("t1", Baseline::DefaultBranchHead),
                launch("t1", BUILD),
                Action::RenderChecklist,
            ],
        ),
    ]);
}

#[test]
fn rule_03_a_validation_validates_exactly_one_commit() {
    run(vec![
        case(
            "a validated commit counts while the branch has not moved",
            state(vec![validated("t1", "c1")]),
            vec![fact(1_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Validated, vec![])
        .checking(|state| subject(state, "t1").validated_commit() == Some(&commit("c1"))),
        case(
            "the same commit pushed again keeps the validation",
            state(vec![validated("t1", "c1")]),
            vec![fact(
                2_000,
                FactKind::BranchPushed {
                    task: task_id("t1"),
                    commit: commit("c1"),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![])
        .checking(|state| subject(state, "t1").validated_commit() == Some(&commit("c1"))),
        case(
            "a new commit on the branch invalidates the validation",
            state(vec![validated("t1", "c1")]),
            vec![fact(
                3_000,
                FactKind::BranchPushed {
                    task: task_id("t1"),
                    commit: commit("c2"),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![])
        .checking(|state| subject(state, "t1").validated_commit().is_none()),
        case(
            "an invalidated prerequisite refuses to unlock a waiting dependent",
            state(vec![
                depending_on(task("t1", TaskState::Approved), "t0", "c1"),
                validated("t0", "c1"),
            ]),
            vec![fact(
                4_000,
                FactKind::BranchPushed {
                    task: task_id("t0"),
                    commit: commit("c2"),
                },
            )],
        )
        .when("t1", TaskState::Approved, vec![])
        .checking(|state| {
            subject(state, "t0").validated_commit().is_none()
                && subject(state, "t1")
                    .dependencies
                    .first()
                    .map(|edge| &edge.commit)
                    == Some(&commit("c1"))
        }),
        case(
            "a task approved against a moved commit queues",
            state(vec![
                depending_on(task("t1", TaskState::Proposed), "t0", "c1"),
                {
                    let mut prerequisite = validated("t0", "c1");
                    prerequisite.branch_head = Some(commit("c2"));
                    prerequisite
                },
            ]),
            vec![fact(5_000, approved("t1"))],
        )
        .when(
            "t1",
            TaskState::Approved,
            vec![queue("t1", None), Action::RenderChecklist],
        ),
    ]);
}

#[test]
fn rule_04_a_stale_dependency_blocks_publication_until_revalidated() {
    run(vec![
        case(
            "the dependent may finish but cannot publish against a stale pin",
            state(vec![
                depending_on(
                    with_attempt(
                        task("t1", TaskState::Running),
                        Attempt {
                            session: Some(session("s1")),
                            worktree: Some(lease("w1")),
                            ..attempt(BUILD)
                        },
                    ),
                    "t0",
                    "c1",
                ),
                {
                    let mut prerequisite = validated("t0", "c1");
                    prerequisite.branch_head = Some(commit("c2"));
                    prerequisite
                },
            ]),
            vec![
                fact(1_000, submitted("t1", "cb")),
                fact(2_000, passed("t1", "cb")),
            ],
        )
        .when(
            "t1",
            TaskState::Validated,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| {
            subject(state, "t1").validated_commit() == Some(&commit("cb"))
                && subject(state, "t1")
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.worktree.clone())
                    == Some(lease("w1"))
        }),
        case(
            "re-baselining and revalidating publishes the dependent",
            state(vec![
                depending_on(
                    with_attempt(
                        validated("t1", "cb"),
                        Attempt {
                            outcome: AttemptOutcome::Submitted,
                            worktree: Some(lease("w1")),
                            ..attempt(BUILD)
                        },
                    ),
                    "t0",
                    "c1",
                ),
                {
                    let mut prerequisite = validated("t0", "c1");
                    prerequisite.branch_head = Some(commit("c2"));
                    prerequisite.state = TaskState::Validating;
                    prerequisite
                },
            ]),
            vec![
                fact(3_000, passed("t0", "c2")),
                fact(
                    4_000,
                    FactKind::WorktreeAcquired {
                        task: task_id("t1"),
                        lease: lease("w2"),
                        baseline: Baseline::PinnedCommit(commit("c2")),
                        included: Vec::new(),
                    },
                ),
                fact(5_000, submitted("t1", "cb2")),
                fact(6_000, passed("t1", "cb2")),
            ],
        )
        .when(
            "t1",
            TaskState::Validated,
            vec![
                Action::Push {
                    task: task_id("t1"),
                    commit: commit("cb2"),
                },
                Action::OpenPullRequest {
                    task: task_id("t1"),
                    commit: commit("cb2"),
                },
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1")
                .dependencies
                .first()
                .is_some_and(|edge| edge.commit == commit("c2"))
        }),
        case(
            "the re-baselined dependent is running again",
            state(vec![
                depending_on(
                    with_attempt(
                        validated("t1", "cb"),
                        Attempt {
                            outcome: AttemptOutcome::Submitted,
                            worktree: Some(lease("w1")),
                            ..attempt(BUILD)
                        },
                    ),
                    "t0",
                    "c1",
                ),
                validated("t0", "c2"),
            ]),
            vec![fact(
                7_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::PinnedCommit(commit("c2")),
                    included: Vec::new(),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                release("t1", "w1"),
                launch("t1", BUILD),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1").attempts.len() == 2
                && subject(state, "t1")
                    .attempts
                    .first()
                    .is_some_and(|attempt| attempt.worktree.is_none())
                && subject(state, "t1")
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.worktree.clone())
                    == Some(lease("w2"))
        }),
    ]);
}

#[test]
fn rule_05_the_concurrency_cap_queues_work_and_frees_it() {
    let capped = || {
        let mut state = state(vec![
            task("t1", TaskState::Proposed),
            task("t2", TaskState::Proposed),
        ]);
        state.limits.max_concurrent_tasks = 1;
        state
    };

    run(vec![
        case(
            "the first approved task takes the only slot",
            capped(),
            vec![fact(1_000, approved("t1"))],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                acquire("t1", Baseline::DefaultBranchHead),
                launch("t1", BUILD),
                Action::RenderChecklist,
            ],
        ),
        case(
            "work past the cap queues",
            capped(),
            vec![fact(1_000, approved("t1")), fact(2_000, approved("t2"))],
        )
        .when(
            "t2",
            TaskState::Approved,
            vec![queue("t2", None), Action::RenderChecklist],
        )
        .checking(|state| subject(state, "t1").state == TaskState::Running),
        case(
            "a freed slot starts the queued task",
            capped(),
            vec![
                fact(1_000, approved("t1")),
                fact(
                    2_000,
                    FactKind::WorktreeAcquired {
                        task: task_id("t1"),
                        lease: lease("w1"),
                        baseline: Baseline::DefaultBranchHead,
                        included: Vec::new(),
                    },
                ),
                fact(3_000, approved("t2")),
                fact(
                    3_500,
                    FactKind::PullRequestOpened {
                        task: task_id("t1"),
                        number: 1,
                        url: "https://example.com/1".to_owned(),
                    },
                ),
                fact(4_000, merged("t1")),
            ],
        )
        .when(
            "t2",
            TaskState::Running,
            vec![release("t1", "w1"), Action::RenderChecklist],
        )
        .checking(|state| {
            subject(state, "t1").state == TaskState::Landed
                && subject(state, "t2").attempts.len() == 1
        }),
    ]);
}

#[test]
fn rule_06_a_run_duration_overrun_pauses_the_task_and_keeps_its_work() {
    run(vec![
        case(
            "an overrun stops the session and holds the task for review",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![fact(
                7_000,
                FactKind::RunDurationExceeded {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                hold("t1"),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1").attempts.len() == 1
                && subject(state, "t1")
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.worktree.clone())
                    == Some(lease("w1"))
        }),
        case(
            "a task that is not in flight is left alone",
            state(vec![validated("t1", "c1")]),
            vec![fact(
                8_000,
                FactKind::RunDurationExceeded {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![]),
    ]);
}

#[test]
fn rule_07_rate_limits_retry_with_bounded_backoff_then_pause() {
    let backoff = Duration::from_secs(30);
    let limited = |profile_id: &str| FactKind::ProviderRateLimited {
        task: task_id("t1"),
        profile: profile(profile_id),
    };
    let retrying = |profile_id: &str, not_before: Timestamp| {
        with_retry(task("t1", TaskState::Approved), profile_id, not_before)
    };

    run(vec![
        case(
            "the first rate limit retries after the base backoff",
            state(vec![running("t1")]),
            vec![fact(1_000, limited(BUILD))],
        )
        .when(
            "t1",
            TaskState::Approved,
            vec![
                queue("t1", Some(at(1_000).plus(backoff))),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1")
                .retry
                .as_ref()
                .is_some_and(|retry| retry.profile == profile(BUILD))
                && holds(state, "t1", AttemptOutcome::Failed)
        }),
        case(
            "the backoff doubles on the next attempt",
            state(vec![{
                let mut task = with_attempt(task("t1", TaskState::Running), spent(BUILD));
                task.attempts.push(attempt(BUILD));
                task
            }]),
            vec![fact(1_000, limited(BUILD))],
        )
        .when(
            "t1",
            TaskState::Approved,
            vec![
                queue("t1", Some(at(1_000).plus(backoff * 2))),
                Action::RenderChecklist,
            ],
        ),
        case(
            "retries stop at the bound",
            state(vec![{
                let mut task = with_attempts(task("t1", TaskState::Running), 2);
                task.attempts.push(attempt(BUILD));
                task
            }]),
            vec![fact(1_000, limited(BUILD))],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
        ),
        case(
            "the retry uses the configured fallback profile",
            {
                let mut state = state(vec![running("t1")]);
                state.fallback_profiles = vec![profile(BUILD), profile("fallback-profile")];
                state
            },
            vec![fact(1_000, limited(BUILD))],
        )
        .when(
            "t1",
            TaskState::Approved,
            vec![
                queue("t1", Some(at(1_000).plus(backoff))),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1")
                .retry
                .as_ref()
                .is_some_and(|retry| retry.profile == profile("fallback-profile"))
        }),
        case(
            "a profile outside the fallback list falls back only inside it",
            {
                let mut state = state(vec![running("t1")]);
                state.fallback_profiles = vec![profile("first-profile"), profile("second-profile")];
                state
            },
            vec![fact(1_000, limited("unlisted-profile"))],
        )
        .when(
            "t1",
            TaskState::Approved,
            vec![
                queue("t1", Some(at(1_000).plus(backoff))),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1")
                .retry
                .as_ref()
                .is_some_and(|retry| retry.profile == profile("first-profile"))
        }),
        case(
            "the queued retry waits out the backoff",
            state(vec![retrying("fallback-profile", at(31_000))]),
            vec![fact(30_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Approved, vec![]),
        case(
            "the retry launches on the fallback profile once the backoff passes",
            state(vec![retrying("fallback-profile", at(31_000))]),
            vec![fact(31_000, FactKind::Polled)],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                acquire("t1", Baseline::DefaultBranchHead),
                launch("t1", "fallback-profile"),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1").retry.is_none()
                && subject(state, "t1")
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.profile == profile("fallback-profile"))
        }),
    ]);
}

#[test]
fn rule_08_question_routing_follows_the_project_setting() {
    let asked = |text: &str, relay: bool| FactKind::QuestionAsked {
        task: task_id("t1"),
        text: text.to_owned(),
        relay,
    };

    run(vec![
        case(
            "a relayed question waits on the user",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![fact(1_000, asked("which database?", true))],
        )
        .when(
            "t1",
            TaskState::WaitingOnQuestion,
            vec![
                Action::Notify {
                    task: task_id("t1"),
                },
                hold("t1"),
                Action::RenderChecklist,
            ],
        ),
        case(
            "a question the coordinator settles does not block",
            state(vec![running("t1")]),
            vec![fact(1_000, asked("which database?", false))],
        )
        .when("t1", TaskState::Running, vec![])
        .checking(|state| subject(state, "t1").questions.len() == 1),
        case(
            "an always-relay project relays every question",
            {
                let mut state = state(vec![running("t1")]);
                state.always_relay_questions = true;
                state
            },
            vec![fact(1_000, asked("which database?", false))],
        )
        .when(
            "t1",
            TaskState::WaitingOnQuestion,
            vec![
                Action::Notify {
                    task: task_id("t1"),
                },
                hold("t1"),
                Action::RenderChecklist,
            ],
        ),
        case(
            "answering the question resumes the session",
            state(vec![with_question(
                running_with_session("t1", "s1", "w1"),
                TaskState::WaitingOnQuestion,
                "which database?",
            )]),
            vec![fact(
                2_000,
                FactKind::QuestionAnswered {
                    task: task_id("t1"),
                    answer: "sqlite".to_owned(),
                    by: AnsweredBy::User,
                },
            )],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                Action::ResumeSession {
                    task: task_id("t1"),
                },
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1")
                .questions
                .last()
                .and_then(|question| question.answer.as_ref())
                .is_some_and(|answer| answer.by == AnsweredBy::User && answer.text == "sqlite")
        }),
    ]);
}

#[test]
fn rule_09_a_failed_validation_opens_no_pull_request() {
    run(vec![
        case(
            "the failure keeps the branch, the worktree and the output",
            state(vec![with_attempt(
                validating("t1"),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(1_000, failed("t1", "cb"))],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| {
            subject(state, "t1").validated_commit().is_none()
                && subject(state, "t1")
                    .validations
                    .last()
                    .is_some_and(|record| {
                        record.exit_code == 1
                            && record.commit == commit("cb")
                            && record.output_tail == "2 tests failed"
                    })
                && subject(state, "t1")
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.worktree.clone())
                    == Some(lease("w1"))
        }),
    ]);
}

#[test]
fn rule_10_restart_reconciliation_prefers_unknown_over_a_guess() {
    run(vec![
        case(
            "a running task becomes unknown rather than failed or succeeded",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![fact(1_000, FactKind::DaemonRestarted)],
        )
        .when("t1", TaskState::Running, vec![])
        .checking(|state| {
            holds(state, "t1", AttemptOutcome::Unknown)
                && subject(state, "t1")
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.finished_at.is_none())
        }),
        case(
            "a task waiting on a question is reconciled the same way",
            state(vec![with_question(
                running_with_session("t1", "s1", "w1"),
                TaskState::WaitingOnQuestion,
                "which database?",
            )]),
            vec![fact(2_000, FactKind::DaemonRestarted)],
        )
        .when("t1", TaskState::WaitingOnQuestion, vec![])
        .checking(|state| holds(state, "t1", AttemptOutcome::Unknown)),
        case(
            "a session proven gone is reported rather than replaced",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![
                fact(3_000, FactKind::DaemonRestarted),
                fact(
                    4_000,
                    FactKind::WorkerLivenessChanged {
                        task: task_id("t1"),
                        liveness: Liveness::Gone,
                    },
                ),
            ],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| holds(state, "t1", AttemptOutcome::Failed)),
        case(
            "a session proven live resumes without a replacement",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![
                fact(5_000, FactKind::DaemonRestarted),
                fact(
                    6_000,
                    FactKind::WorkerLivenessChanged {
                        task: task_id("t1"),
                        liveness: Liveness::Live,
                    },
                ),
            ],
        )
        .when("t1", TaskState::Running, vec![])
        .checking(|state| holds(state, "t1", AttemptOutcome::InFlight)),
    ]);
}

#[test]
fn rule_11_a_landed_task_releases_its_worktree_and_renders() {
    run(vec![
        case(
            "the merge releases the lease and renders the checklist",
            state(vec![with_pull_request(
                with_attempt(
                    task("t1", TaskState::PrOpen),
                    Attempt {
                        outcome: AttemptOutcome::Submitted,
                        worktree: Some(lease("w1")),
                        ..attempt(BUILD)
                    },
                ),
                42,
                Checks::Passing,
            )]),
            vec![fact(1_000, merged("t1"))],
        )
        .when(
            "t1",
            TaskState::Landed,
            vec![release("t1", "w1"), Action::RenderChecklist],
        )
        .checking(|state| {
            subject(state, "t1").attempts.len() == 1
                && subject(state, "t1")
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.worktree.is_none())
        }),
        case(
            "a closed pull request holds the task instead of landing it",
            state(vec![with_pull_request(
                task("t1", TaskState::PrOpen),
                42,
                Checks::Failing,
            )]),
            vec![fact(
                2_000,
                FactKind::PullRequestClosedUnmerged {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Cancelled,
            vec![hold("t1"), Action::RenderChecklist],
        ),
    ]);
}

#[test]
fn facts_about_unknown_tasks_change_nothing() {
    let state = base();
    let (next, actions) = reduce(&state, &fact(1_000, approved("ghost")));
    assert_eq!(next, state);
    assert!(actions.is_empty());
}

#[test]
fn a_proposed_task_is_recorded_once() {
    let before = base();
    let proposed = fact(
        1_000,
        FactKind::TaskProposed {
            task: task_id("t1"),
            title: "add the store".to_owned(),
            intent: "record tasks in sqlite".to_owned(),
            role: Role::Build,
            dependencies: Vec::new(),
            base_dependency: None,
        },
    );
    let (recorded, actions) = reduce(&before, &proposed);
    assert_eq!(actions, vec![Action::RenderChecklist]);
    let (again, actions) = reduce(&recorded, &proposed);
    assert!(actions.is_empty());
    assert_eq!(again.tasks.len(), 1);
    let task = subject(&again, "t1");
    assert_eq!(task.state, TaskState::Proposed);
    assert_eq!(task.title, "add the store");
    assert_eq!(task.intent, "record tasks in sqlite");
    assert_eq!(task.project, ProjectId::from("depot"));
    assert_eq!(task.created_at, at(1_000));
}

#[test]
fn a_worker_turn_that_ends_without_submitting_is_commentary() {
    run(vec![
        case(
            "a finished turn changes no state",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![
                fact(
                    1_000,
                    FactKind::WorkerTurnStarted {
                        task: task_id("t1"),
                        session: session("s1"),
                    },
                ),
                fact(
                    2_000,
                    FactKind::WorkerTurnEnded {
                        task: task_id("t1"),
                    },
                ),
            ],
        )
        .when("t1", TaskState::Running, vec![])
        .checking(|state| holds(state, "t1", AttemptOutcome::InFlight)),
    ]);
}

#[test]
fn pull_request_checks_are_recorded_without_a_transition() {
    run(vec![
        case(
            "a checks change is recorded on the link",
            state(vec![with_pull_request(
                task("t1", TaskState::PrOpen),
                42,
                Checks::Pending,
            )]),
            vec![fact(
                1_000,
                FactKind::PullRequestChecksChanged {
                    task: task_id("t1"),
                    checks: Checks::Passing,
                },
            )],
        )
        .when("t1", TaskState::PrOpen, vec![])
        .checking(|state| {
            subject(state, "t1")
                .pull_request()
                .map(|(_, _, checks)| checks)
                == Some(Checks::Passing)
        }),
    ]);
}

fn with_attempts(mut task: Task, count: usize) -> Task {
    while task.attempts.len() < count {
        task.attempts.push(spent(BUILD));
    }
    task
}

fn with_pull_request(mut task: Task, number: u64, checks: Checks) -> Task {
    task.links.push(Link::PullRequest {
        number,
        url: format!("https://github.com/nunoras/depot/pull/{number}"),
        checks,
    });
    task
}

fn with_retry(mut task: Task, profile_id: &str, not_before: Timestamp) -> Task {
    task.attempts.push(spent(BUILD));
    task.retry = Some(Retry {
        profile: profile(profile_id),
        not_before,
    });
    task
}

fn with_question(mut task: Task, state: TaskState, text: &str) -> Task {
    task.state = state;
    task.questions.push(Question {
        text: text.to_owned(),
        asked_at: at(1_000),
        answer: None,
    });
    task
}

#[test]
fn late_validation_cannot_revive_a_duration_failed_task() {
    run(vec![case(
        "a late success after a duration overrun stays failed",
        state(vec![with_attempt(
            validating("t1"),
            Attempt {
                outcome: AttemptOutcome::Submitted,
                worktree: Some(lease("w1")),
                ..attempt(BUILD)
            },
        )]),
        vec![
            fact(
                1_000,
                FactKind::RunDurationExceeded {
                    task: task_id("t1"),
                },
            ),
            fact(2_000, passed("t1", "cb")),
        ],
    )
    .when("t1", TaskState::Failed, vec![])
    .checking(|state| subject(state, "t1").validations.is_empty())]);
}

#[test]
fn worktree_acquired_rework_respects_state_and_cap() {
    let closed_validated = || {
        with_attempt(
            validated("t1", "cb"),
            Attempt {
                outcome: AttemptOutcome::Submitted,
                worktree: Some(lease("w1")),
                profile: profile(BUILD),
                started_at: at(0),
                finished_at: Some(at(0)),
                session: None,
            },
        )
    };

    run(vec![
        case(
            "rework launches only while under the concurrency cap",
            {
                let mut state = state(vec![closed_validated(), running("t2")]);
                state.limits.max_concurrent_tasks = 1;
                state
            },
            vec![fact(
                1_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::DefaultBranchHead,
                    included: Vec::new(),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![release("t1", "w2")])
        .checking(|state| {
            subject(state, "t1").attempts.len() == 1
                && subject(state, "t2").state == TaskState::Running
        }),
        case(
            "a failed task is not restarted by worktree acquisition",
            state(vec![with_attempt(
                task("t1", TaskState::Failed),
                Attempt {
                    outcome: AttemptOutcome::Failed,
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                2_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::DefaultBranchHead,
                    included: Vec::new(),
                },
            )],
        )
        .when("t1", TaskState::Failed, vec![release("t1", "w2")])
        .checking(|state| subject(state, "t1").attempts.len() == 1),
        case(
            "a cancelled task is not restarted by worktree acquisition",
            state(vec![with_attempt(
                task("t1", TaskState::Cancelled),
                Attempt {
                    outcome: AttemptOutcome::Stopped,
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                3_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::DefaultBranchHead,
                    included: Vec::new(),
                },
            )],
        )
        .when("t1", TaskState::Cancelled, vec![release("t1", "w2")])
        .checking(|state| subject(state, "t1").attempts.len() == 1),
    ]);
}

#[test]
fn rate_limit_stops_a_live_session() {
    run(vec![case(
        "a rate-limited running session is stopped before retry queues",
        state(vec![running_with_session("t1", "s1", "w1")]),
        vec![fact(
            1_000,
            FactKind::ProviderRateLimited {
                task: task_id("t1"),
                profile: profile(BUILD),
            },
        )],
    )
    .when(
        "t1",
        TaskState::Approved,
        vec![
            Action::StopSession {
                task: task_id("t1"),
            },
            release("t1", "w1"),
            queue("t1", Some(at(1_000).plus(Duration::from_secs(30)))),
            Action::RenderChecklist,
        ],
    )
    .checking(|state| {
        holds(state, "t1", AttemptOutcome::Failed)
            && subject(state, "t1")
                .attempts
                .last()
                .is_some_and(|attempt| attempt.worktree.is_none())
    })]);
}

#[test]
fn merge_is_blocked_while_dependency_pins_are_stale() {
    run(vec![case(
        "a merged pr with a stale pin stays open and holds for the user",
        state(vec![
            depending_on(
                with_pull_request(
                    with_attempt(
                        task("t1", TaskState::PrOpen),
                        Attempt {
                            outcome: AttemptOutcome::Submitted,
                            worktree: Some(lease("w1")),
                            ..attempt(BUILD)
                        },
                    ),
                    7,
                    Checks::Passing,
                ),
                "t0",
                "c1",
            ),
            validated("t0", "c2"),
        ]),
        vec![fact(1_000, merged("t1"))],
    )
    .when(
        "t1",
        TaskState::PrOpen,
        vec![hold("t1"), Action::RenderChecklist],
    )
    .checking(|state| {
        subject(state, "t1")
            .attempts
            .last()
            .and_then(|attempt| attempt.worktree.clone())
            == Some(lease("w1"))
            && subject(state, "t1")
                .dependencies
                .first()
                .map(|edge| &edge.commit)
                == Some(&commit("c1"))
    })]);
}

#[test]
fn rejected_rework_does_not_repin_a_validated_task() {
    run(vec![
        case(
            "a capped rework leaves the old pin in place",
            {
                let mut state = state(vec![
                    depending_on(
                        with_attempt(
                            validated("t1", "cb"),
                            Attempt {
                                outcome: AttemptOutcome::Submitted,
                                worktree: Some(lease("w1")),
                                finished_at: Some(at(0)),
                                ..attempt(BUILD)
                            },
                        ),
                        "t0",
                        "c1",
                    ),
                    validated("t0", "c2"),
                    running("t2"),
                ]);
                state.limits.max_concurrent_tasks = 1;
                state
            },
            vec![fact(
                1_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::PinnedCommit(commit("c2")),
                    included: Vec::new(),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![release("t1", "w2")])
        .checking(|state| {
            subject(state, "t1").attempts.len() == 1
                && subject(state, "t1")
                    .dependencies
                    .first()
                    .map(|edge| &edge.commit)
                    == Some(&commit("c1"))
                && publication_still_blocked(state, "t1")
        }),
        case(
            "a multi-dep rework stays held when only the base pin is reported",
            state(vec![
                with_base(
                    depending_on(
                        depending_on(
                            with_attempt(
                                validated("t1", "cb"),
                                Attempt {
                                    outcome: AttemptOutcome::Submitted,
                                    worktree: Some(lease("w1")),
                                    finished_at: Some(at(0)),
                                    ..attempt(BUILD)
                                },
                            ),
                            "a",
                            "ca1",
                        ),
                        "b",
                        "cb1",
                    ),
                    "a",
                ),
                validated("a", "ca2"),
                validated("b", "cb2"),
            ]),
            vec![fact(
                2_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::PinnedCommit(commit("ca2")),
                    included: vec![Dependency {
                        task: task_id("a"),
                        commit: commit("ca2"),
                    }],
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![release("t1", "w2")])
        .checking(|state| {
            let edges = &subject(state, "t1").dependencies;
            edges
                .iter()
                .any(|edge| edge.task == task_id("a") && edge.commit == commit("ca1"))
                && edges
                    .iter()
                    .any(|edge| edge.task == task_id("b") && edge.commit == commit("cb1"))
                && subject(state, "t1").attempts.len() == 1
        }),
        case(
            "a multi-dep rework launches when every included pin is reported",
            state(vec![
                with_base(
                    depending_on(
                        depending_on(
                            with_attempt(
                                validated("t1", "cb"),
                                Attempt {
                                    outcome: AttemptOutcome::Submitted,
                                    worktree: Some(lease("w1")),
                                    finished_at: Some(at(0)),
                                    ..attempt(BUILD)
                                },
                            ),
                            "a",
                            "ca1",
                        ),
                        "b",
                        "cb1",
                    ),
                    "a",
                ),
                validated("a", "ca2"),
                validated("b", "cb2"),
            ]),
            vec![fact(
                3_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::PinnedCommit(commit("ca2")),
                    included: vec![
                        Dependency {
                            task: task_id("a"),
                            commit: commit("ca2"),
                        },
                        Dependency {
                            task: task_id("b"),
                            commit: commit("cb2"),
                        },
                    ],
                },
            )],
        )
        .when(
            "t1",
            TaskState::Running,
            vec![
                release("t1", "w1"),
                launch("t1", BUILD),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            let edges = &subject(state, "t1").dependencies;
            edges
                .iter()
                .any(|edge| edge.task == task_id("a") && edge.commit == commit("ca2"))
                && edges
                    .iter()
                    .any(|edge| edge.task == task_id("b") && edge.commit == commit("cb2"))
                && subject(state, "t1").attempts.len() == 2
        }),
    ]);
}

#[test]
fn unaccepted_worktree_acquired_releases_the_fact_lease() {
    run(vec![case(
        "a blocked multi-dep rework releases the unattached lease",
        state(vec![
            with_base(
                depending_on(
                    depending_on(
                        with_attempt(
                            validated("t1", "cb"),
                            Attempt {
                                outcome: AttemptOutcome::Submitted,
                                worktree: Some(lease("w1")),
                                finished_at: Some(at(0)),
                                ..attempt(BUILD)
                            },
                        ),
                        "a",
                        "ca1",
                    ),
                    "b",
                    "cb1",
                ),
                "a",
            ),
            validated("a", "ca2"),
            validated("b", "cb2"),
        ]),
        vec![fact(
            1_000,
            FactKind::WorktreeAcquired {
                task: task_id("t1"),
                lease: lease("w2"),
                baseline: Baseline::PinnedCommit(commit("ca2")),
                included: vec![Dependency {
                    task: task_id("a"),
                    commit: commit("ca2"),
                }],
            },
        )],
    )
    .when("t1", TaskState::Validated, vec![release("t1", "w2")])
    .checking(|state| {
        subject(state, "t1")
            .attempts
            .last()
            .and_then(|attempt| attempt.worktree.clone())
            == Some(lease("w1"))
    })]);
}

#[test]
fn duplicate_submit_does_not_requeue_validation() {
    run(vec![case(
        "a second submit while validating is ignored",
        state(vec![running_with_session("t1", "s1", "w1")]),
        vec![
            fact(1_000, submitted("t1", "cb")),
            fact(2_000, submitted("t1", "cb")),
        ],
    )
    .when("t1", TaskState::Validating, vec![])
    .checking(|state| {
        subject(state, "t1").attempts.len() == 1
            && holds(state, "t1", AttemptOutcome::Submitted)
    })]);
}

#[test]
fn rate_limit_retry_releases_the_worktree_but_exhaustion_keeps_it() {
    run(vec![
        case(
            "a retryable rate limit releases the closed attempt lease",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![fact(
                1_000,
                FactKind::ProviderRateLimited {
                    task: task_id("t1"),
                    profile: profile(BUILD),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Approved,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                release("t1", "w1"),
                queue("t1", Some(at(1_000).plus(Duration::from_secs(30)))),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1")
                .attempts
                .last()
                .is_some_and(|attempt| attempt.worktree.is_none())
        }),
        case(
            "exhausting retries keeps the lease for review",
            state(vec![{
                let mut task = with_attempts(task("t1", TaskState::Running), 2);
                task.attempts.push(Attempt {
                    session: Some(session("s1")),
                    worktree: Some(lease("w1")),
                    ..attempt(BUILD)
                });
                task
            }]),
            vec![fact(
                2_000,
                FactKind::ProviderRateLimited {
                    task: task_id("t1"),
                    profile: profile(BUILD),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                hold("t1"),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1")
                .attempts
                .last()
                .and_then(|attempt| attempt.worktree.clone())
                == Some(lease("w1"))
        }),
    ]);
}

fn publication_still_blocked(state: &ProjectState, id: &str) -> bool {
    subject(state, id)
        .dependencies
        .iter()
        .any(|dependency| {
            state
                .tasks
                .get(&dependency.task)
                .and_then(|task| task.validated_commit())
                != Some(&dependency.commit)
        })
}

#[test]
fn stale_pr_open_can_rework_revalidate_and_land() {
    run(vec![case(
        "a held pr-open dependent reworks against the new pin and lands",
        state(vec![
            depending_on(
                with_pull_request(
                    with_attempt(
                        task("t1", TaskState::PrOpen),
                        Attempt {
                            outcome: AttemptOutcome::Submitted,
                            worktree: Some(lease("w1")),
                            finished_at: Some(at(0)),
                            ..attempt(BUILD)
                        },
                    ),
                    7,
                    Checks::Passing,
                ),
                "t0",
                "c1",
            ),
            {
                let mut prerequisite = validated("t0", "c1");
                prerequisite.branch_head = Some(commit("c2"));
                prerequisite.state = TaskState::Validating;
                prerequisite
            },
        ]),
        vec![
            fact(1_000, passed("t0", "c2")),
            fact(2_000, merged("t1")),
            fact(
                3_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w2"),
                    baseline: Baseline::PinnedCommit(commit("c2")),
                    included: Vec::new(),
                },
            ),
            fact(4_000, submitted("t1", "cb2")),
            fact(5_000, passed("t1", "cb2")),
            fact(6_000, merged("t1")),
        ],
    )
    .when(
        "t1",
        TaskState::Landed,
        vec![release("t1", "w2"), Action::RenderChecklist],
    )
    .checking(|state| {
        subject(state, "t1")
            .dependencies
            .first()
            .is_some_and(|edge| edge.commit == commit("c2"))
            && subject(state, "t1").links.len() == 1
            && subject(state, "t1")
                .attempts
                .last()
                .is_some_and(|attempt| attempt.worktree.is_none())
            && subject(state, "t0").state == TaskState::Validated
    })]);
}

#[test]
fn rate_limit_ignores_tasks_that_are_not_in_flight() {
    run(vec![
        case(
            "a validated task ignores a late rate limit",
            state(vec![with_attempt(
                validated("t1", "c1"),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                1_000,
                FactKind::ProviderRateLimited {
                    task: task_id("t1"),
                    profile: profile(BUILD),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![])
        .checking(|state| {
            subject(state, "t1").retry.is_none()
                && subject(state, "t1")
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.worktree.clone())
                    == Some(lease("w1"))
        }),
        case(
            "a failed task under the attempt bound stays failed",
            state(vec![with_attempt(
                task("t1", TaskState::Failed),
                Attempt {
                    outcome: AttemptOutcome::Failed,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                2_000,
                FactKind::ProviderRateLimited {
                    task: task_id("t1"),
                    profile: profile(BUILD),
                },
            )],
        )
        .when("t1", TaskState::Failed, vec![])
        .checking(|state| subject(state, "t1").state == TaskState::Failed),
    ]);
}

#[test]
fn rework_releases_the_prior_attempt_lease() {
    run(vec![case(
        "accepted rework clears and releases the previous lease before launching",
        state(vec![
            depending_on(
                with_attempt(
                    validated("t1", "cb"),
                    Attempt {
                        outcome: AttemptOutcome::Submitted,
                        worktree: Some(lease("w1")),
                        finished_at: Some(at(0)),
                        ..attempt(BUILD)
                    },
                ),
                "t0",
                "c1",
            ),
            validated("t0", "c2"),
        ]),
        vec![fact(
            1_000,
            FactKind::WorktreeAcquired {
                task: task_id("t1"),
                lease: lease("w2"),
                baseline: Baseline::PinnedCommit(commit("c2")),
                included: Vec::new(),
            },
        )],
    )
    .when(
        "t1",
        TaskState::Running,
        vec![
            release("t1", "w1"),
            launch("t1", BUILD),
            Action::RenderChecklist,
        ],
    )
    .checking(|state| {
        subject(state, "t1").attempts.len() == 2
            && subject(state, "t1")
                .attempts
                .first()
                .is_some_and(|attempt| attempt.worktree.is_none())
            && subject(state, "t1")
                .attempts
                .last()
                .and_then(|attempt| attempt.worktree.clone())
                == Some(lease("w2"))
    })]);
}

#[test]
fn landing_clears_the_attempt_lease_and_is_idempotent() {
    run(vec![case(
        "a second merge after landing does not release again",
        state(vec![with_pull_request(
            with_attempt(
                task("t1", TaskState::PrOpen),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            ),
            42,
            Checks::Passing,
        )]),
        vec![fact(1_000, merged("t1")), fact(2_000, merged("t1"))],
    )
    .when("t1", TaskState::Landed, vec![])
    .checking(|state| {
        subject(state, "t1")
            .attempts
            .last()
            .is_some_and(|attempt| attempt.worktree.is_none())
    })]);
}

#[test]
fn rework_reuses_the_same_lease_without_releasing_it() {
    run(vec![case(
        "accepted same-lease rework moves the lease to the new attempt",
        state(vec![
            depending_on(
                with_attempt(
                    validated("t1", "cb"),
                    Attempt {
                        outcome: AttemptOutcome::Submitted,
                        worktree: Some(lease("w1")),
                        finished_at: Some(at(0)),
                        ..attempt(BUILD)
                    },
                ),
                "t0",
                "c1",
            ),
            validated("t0", "c2"),
        ]),
        vec![fact(
            1_000,
            FactKind::WorktreeAcquired {
                task: task_id("t1"),
                lease: lease("w1"),
                baseline: Baseline::PinnedCommit(commit("c2")),
                included: Vec::new(),
            },
        )],
    )
    .when(
        "t1",
        TaskState::Running,
        vec![launch("t1", BUILD), Action::RenderChecklist],
    )
    .checking(|state| {
        subject(state, "t1").attempts.len() == 2
            && subject(state, "t1")
                .attempts
                .first()
                .is_some_and(|attempt| attempt.worktree.is_none())
            && subject(state, "t1")
                .attempts
                .last()
                .and_then(|attempt| attempt.worktree.clone())
                == Some(lease("w1"))
    })]);
}

#[test]
fn live_worktree_acquired_releases_a_replaced_lease() {
    run(vec![case(
        "a second live acquire releases the prior different lease",
        state(vec![running_with_session("t1", "s1", "w1")]),
        vec![fact(
            1_000,
            FactKind::WorktreeAcquired {
                task: task_id("t1"),
                lease: lease("w2"),
                baseline: Baseline::DefaultBranchHead,
                included: Vec::new(),
            },
        )],
    )
    .when("t1", TaskState::Running, vec![release("t1", "w1")])
    .checking(|state| {
        subject(state, "t1")
            .attempts
            .last()
            .and_then(|attempt| attempt.worktree.clone())
            == Some(lease("w2"))
    })]);
}

#[test]
fn relayed_questions_do_not_leave_validating_or_terminal_states() {
    run(vec![
        case(
            "a late relayed question during validation is recorded only",
            state(vec![with_attempt(
                task("t1", TaskState::Validating),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                1_000,
                FactKind::QuestionAsked {
                    task: task_id("t1"),
                    text: "which database?".to_owned(),
                    relay: true,
                },
            )],
        )
        .when("t1", TaskState::Validating, vec![])
        .checking(|state| subject(state, "t1").questions.len() == 1),
        case(
            "a relayed question on a validated task stays validated",
            state(vec![validated("t1", "c1")]),
            vec![fact(
                2_000,
                FactKind::QuestionAsked {
                    task: task_id("t1"),
                    text: "which database?".to_owned(),
                    relay: true,
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![])
        .checking(|state| subject(state, "t1").questions.len() == 1),
    ]);
}

#[test]
fn merge_only_lands_from_pr_open() {
    run(vec![case(
        "a validated task without a pull request ignores merge",
        state(vec![with_attempt(
            validated("t1", "c1"),
            Attempt {
                outcome: AttemptOutcome::Submitted,
                worktree: Some(lease("w1")),
                finished_at: Some(at(0)),
                ..attempt(BUILD)
            },
        )]),
        vec![fact(1_000, merged("t1"))],
    )
    .when("t1", TaskState::Validated, vec![])
    .checking(|state| {
        subject(state, "t1")
            .attempts
            .last()
            .and_then(|attempt| attempt.worktree.clone())
            == Some(lease("w1"))
    })]);
}

#[test]
fn revalidation_with_an_existing_pr_only_pushes() {
    run(vec![case(
        "a task that already has a pull request pushes the new head only",
        state(vec![with_pull_request(
            with_attempt(
                task("t1", TaskState::Validating),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w2")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            ),
            7,
            Checks::Passing,
        )]),
        vec![fact(1_000, passed("t1", "cb2"))],
    )
    .when(
        "t1",
        TaskState::PrOpen,
        vec![
            Action::Push {
                task: task_id("t1"),
                commit: commit("cb2"),
            },
            Action::RenderChecklist,
        ],
    )
    .checking(|state| subject(state, "t1").links.len() == 1)]);
}

#[test]
fn multi_dependency_tasks_require_a_declared_base() {
    let edges = vec![
        Dependency {
            task: task_id("a"),
            commit: commit("ca"),
        },
        Dependency {
            task: task_id("b"),
            commit: commit("cb"),
        },
    ];

    let refused = fact(
        1_000,
        FactKind::TaskProposed {
            task: task_id("t1"),
            title: "depends on two".to_owned(),
            intent: "needs both".to_owned(),
            role: Role::Build,
            dependencies: edges.clone(),
            base_dependency: None,
        },
    );
    let (next, actions) = reduce(&base(), &refused);
    assert!(next.tasks.is_empty());
    assert!(actions.is_empty());

    let accepted = fact(
        2_000,
        FactKind::TaskProposed {
            task: task_id("t1"),
            title: "depends on two".to_owned(),
            intent: "needs both".to_owned(),
            role: Role::Build,
            dependencies: edges,
            base_dependency: Some(task_id("b")),
        },
    );
    let (next, actions) = reduce(&base(), &accepted);
    assert_eq!(actions, vec![Action::RenderChecklist]);
    let recorded = subject(&next, "t1");
    assert_eq!(recorded.base_dependency, Some(task_id("b")));
    assert_eq!(recorded.dependencies.len(), 2);

    run(vec![case(
        "acquire uses the declared base pin rather than the minimum id",
        state(vec![
            with_base(
                depending_on(
                    depending_on(task("t1", TaskState::Proposed), "a", "ca"),
                    "b",
                    "cb",
                ),
                "b",
            ),
            validated("a", "ca"),
            validated("b", "cb"),
        ]),
        vec![fact(3_000, approved("t1"))],
    )
    .when(
        "t1",
        TaskState::Running,
        vec![
            acquire("t1", Baseline::PinnedCommit(commit("cb"))),
            launch("t1", BUILD),
            Action::RenderChecklist,
        ],
    )]);
}
