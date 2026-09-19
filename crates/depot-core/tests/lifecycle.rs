use std::collections::BTreeMap;
use std::time::Duration;

use depot_core::*;

type Check = Box<dyn Fn(&ProjectState) -> bool>;

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

fn running_with_lease(id: &str, lease_id: &str) -> Task {
    with_attempt(
        task(id, TaskState::Running),
        Attempt {
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
        coordinator: None,
        profiles: BTreeMap::from([(Role::Build, profile(BUILD)), (Role::Plan, profile(PLAN))]),
        fallback_profiles: Vec::new(),
        limits: Limits::default(),
        always_relay_questions: false,
        auto_merge: false,
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

    fn checking(mut self, check: impl Fn(&ProjectState) -> bool + 'static) -> Self {
        self.check = Some(Box::new(check));
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
        .when("t1", TaskState::Validated, vec![Action::RenderChecklist])
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
        .when("t1", TaskState::Validated, vec![Action::RenderChecklist])
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
        .when("t1", TaskState::Approved, vec![Action::RenderChecklist])
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
                fact(4_000, submitted("t1", "c1")),
                fact(5_000, passed("t1", "c1")),
            ],
        )
        .when(
            "t2",
            TaskState::Running,
            vec![
                Action::Push {
                    task: task_id("t1"),
                    commit: commit("c1"),
                },
                Action::OpenPullRequest {
                    task: task_id("t1"),
                    commit: commit("c1"),
                },
                acquire("t2", Baseline::DefaultBranchHead),
                launch("t2", BUILD),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            subject(state, "t1").state == TaskState::Validated
                && subject(state, "t2").attempts.len() == 1
                && holds(state, "t1", AttemptOutcome::Submitted)
        }),
        case(
            "opening a pr from a live running worker does not free the slot",
            {
                let mut state = state(vec![
                    running_with_session("t1", "s1", "w1"),
                    task("t2", TaskState::Approved),
                ]);
                state.limits.max_concurrent_tasks = 1;
                state
            },
            vec![fact(
                6_000,
                FactKind::PullRequestOpened {
                    task: task_id("t1"),
                    number: 1,
                    url: "https://example.com/1".to_owned(),
                },
            )],
        )
        .when("t2", TaskState::Approved, vec![])
        .checking(|state| {
            subject(state, "t1").state == TaskState::Running
                && holds(state, "t1", AttemptOutcome::InFlight)
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
                Action::StopSession {
                    task: task_id("t1"),
                },
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
                Action::StopSession {
                    task: task_id("t1"),
                },
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
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                hold("t1"),
                Action::RenderChecklist,
            ],
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
                Action::StopSession {
                    task: task_id("t1"),
                },
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
                Action::StopSession {
                    task: task_id("t1"),
                },
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
        .when("t1", TaskState::Running, vec![Action::RenderChecklist])
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
        .when("t1", TaskState::Running, vec![Action::RenderChecklist])
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
        .when(
            "t1",
            TaskState::WaitingOnQuestion,
            vec![Action::RenderChecklist],
        )
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
        .when("t1", TaskState::Running, vec![Action::RenderChecklist])
        .checking(|state| holds(state, "t1", AttemptOutcome::InFlight)),
        case(
            "a session gone while the task waits on a question stays paused",
            state(vec![with_question(
                running_with_session("t1", "s1", "w1"),
                TaskState::WaitingOnQuestion,
                "which database?",
            )]),
            vec![fact(
                7_000,
                FactKind::WorkerLivenessChanged {
                    task: task_id("t1"),
                    liveness: Liveness::Gone,
                },
            )],
        )
        .when("t1", TaskState::WaitingOnQuestion, Vec::new())
        .checking(|state| holds(state, "t1", AttemptOutcome::InFlight)),
        case(
            "a session gone after a settled question stays paused",
            state(vec![with_question(
                running_with_session("t1", "s1", "w1"),
                TaskState::Running,
                "which database?",
            )]),
            vec![fact(
                8_000,
                FactKind::WorkerLivenessChanged {
                    task: task_id("t1"),
                    liveness: Liveness::Gone,
                },
            )],
        )
        .when("t1", TaskState::Running, Vec::new())
        .checking(|state| holds(state, "t1", AttemptOutcome::InFlight)),
        case(
            "a worker turn that cannot be resolved is held for a person",
            state(vec![running_with_lease("t1", "w1")]),
            vec![fact(
                9_000,
                FactKind::WorkerTurnUnresolved {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| holds(state, "t1", AttemptOutcome::Failed)),
    ]);
}

#[test]
fn rule_11_a_landed_task_releases_its_worktree_and_renders() {
    run(vec![
        case(
            "the merge releases the lease and renders the checklist",
            state(vec![with_attempt(
                pr_open("t1", "merge-commit", 42),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    ..attempt(BUILD)
                },
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
fn unknown_task_worktree_acquired_releases_the_lease() {
    let state = base();
    let (next, actions) = reduce(
        &state,
        &fact(
            1_000,
            FactKind::WorktreeAcquired {
                task: task_id("ghost"),
                lease: lease("w-orphan"),
                baseline: Baseline::DefaultBranchHead,
                included: Vec::new(),
            },
        ),
    );
    assert_eq!(next, state);
    assert_eq!(
        actions,
        vec![Action::ReleaseWorktree {
            task: task_id("ghost"),
            lease: lease("w-orphan"),
        }]
    );
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
            dispatch_profile: None,
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
        .when("t1", TaskState::PrOpen, vec![Action::RenderChecklist])
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

fn pr_open(id: &str, commit_id: &str, number: u64) -> Task {
    let mut task = with_pull_request(validated(id, commit_id), number, Checks::Passing);
    task.state = TaskState::PrOpen;
    task.branch_head = Some(commit(commit_id));
    task
}

fn with_refusal(mut task: Task, reason: &str) -> Task {
    task.merge_refused = Some(reason.to_owned());
    task
}

fn refused_merge(task: &str, reason: &str) -> FactKind {
    FactKind::PullRequestMergeRefused {
        task: task_id(task),
        commit: commit("c1"),
        base: commit("b1"),
        checks: Checks::Passing,
        reason: reason.to_owned(),
    }
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
    run(vec![
        case(
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
        .checking(|state| subject(state, "t1").validations.is_empty()),
    ]);
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
    run(vec![
        case(
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
        }),
    ]);
}

#[test]
fn merge_is_blocked_while_dependency_pins_are_stale() {
    run(vec![
        case(
            "a merged pr with a stale pin stays open and holds for the user",
            state(vec![
                depending_on(
                    with_attempt(
                        pr_open("t1", "merge-commit", 7),
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
        }),
    ]);
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
    run(vec![
        case(
            "a blocked multi-dep rework releases a distinct unattached lease",
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
        }),
        case(
            "a blocked same-lease multi-dep rework keeps the held lease",
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
                    lease: lease("w1"),
                    baseline: Baseline::PinnedCommit(commit("ca2")),
                    included: vec![Dependency {
                        task: task_id("a"),
                        commit: commit("ca2"),
                    }],
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![])
        .checking(|state| {
            subject(state, "t1")
                .attempts
                .last()
                .and_then(|attempt| attempt.worktree.clone())
                == Some(lease("w1"))
        }),
        case(
            "a capped same-lease rework keeps the held lease",
            {
                let mut state = state(vec![
                    with_attempt(
                        validated("t1", "cb"),
                        Attempt {
                            outcome: AttemptOutcome::Submitted,
                            worktree: Some(lease("w1")),
                            finished_at: Some(at(0)),
                            ..attempt(BUILD)
                        },
                    ),
                    running("t2"),
                ]);
                state.limits.max_concurrent_tasks = 1;
                state
            },
            vec![fact(
                3_000,
                FactKind::WorktreeAcquired {
                    task: task_id("t1"),
                    lease: lease("w1"),
                    baseline: Baseline::DefaultBranchHead,
                    included: Vec::new(),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![])
        .checking(|state| {
            subject(state, "t1")
                .attempts
                .last()
                .and_then(|attempt| attempt.worktree.clone())
                == Some(lease("w1"))
                && subject(state, "t2").state == TaskState::Running
        }),
    ]);
}

#[test]
fn duplicate_submit_does_not_requeue_validation() {
    run(vec![
        case(
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
        }),
    ]);
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
    subject(state, id).dependencies.iter().any(|dependency| {
        state
            .tasks
            .get(&dependency.task)
            .and_then(|task| task.validated_commit())
            != Some(&dependency.commit)
    })
}

#[test]
fn stale_pr_open_can_rework_revalidate_and_land() {
    run(vec![
        case(
            "a held pr-open dependent reworks against the new pin and lands",
            state(vec![
                depending_on(
                    with_attempt(
                        pr_open("t1", "merge-commit", 7),
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
                fact(
                    5_500,
                    FactKind::BranchPushed {
                        task: task_id("t1"),
                        commit: commit("cb2"),
                    },
                ),
                fact(
                    6_000,
                    FactKind::PullRequestMerged {
                        task: task_id("t1"),
                        commit: commit("cb2"),
                    },
                ),
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
        }),
    ]);
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
    run(vec![
        case(
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
        }),
    ]);
}

#[test]
fn landing_clears_the_attempt_lease_and_is_idempotent() {
    run(vec![
        case(
            "a second merge after landing does not release again",
            state(vec![with_attempt(
                pr_open("t1", "merge-commit", 42),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(1_000, merged("t1")), fact(2_000, merged("t1"))],
        )
        .when("t1", TaskState::Landed, vec![])
        .checking(|state| {
            subject(state, "t1")
                .attempts
                .last()
                .is_some_and(|attempt| attempt.worktree.is_none())
        }),
    ]);
}

#[test]
fn rework_reuses_the_same_lease_without_releasing_it() {
    run(vec![
        case(
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
        }),
    ]);
}

#[test]
fn live_worktree_acquired_releases_a_replaced_lease() {
    run(vec![
        case(
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
                && subject(state, "t1")
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.worktree.clone())
                    == Some(lease("w2"))
        }),
    ]);
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
        .when("t1", TaskState::Validating, vec![Action::RenderChecklist])
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
        .when("t1", TaskState::Validated, vec![Action::RenderChecklist])
        .checking(|state| subject(state, "t1").questions.len() == 1),
    ]);
}

#[test]
fn auto_merge_waits_for_the_project_to_opt_in_and_for_the_validated_head() {
    let head = commit("c1");
    let mut state = state(vec![pr_open("t1", "c1", 42)]);

    assert!(
        !auto_merge_due(&state, subject(&state, "t1"), &head),
        "a project that did not opt in is never merged automatically"
    );

    state.auto_merge = true;
    assert!(auto_merge_due(&state, subject(&state, "t1"), &head));
    assert!(
        !auto_merge_due(&state, subject(&state, "t1"), &commit("c2")),
        "a head depot never validated is never merged automatically"
    );

    let mut failing = state.clone();
    let task = failing.tasks.get_mut(&task_id("t1")).expect("subject task");
    task.links = vec![Link::PullRequest {
        number: 42,
        url: "https://github.com/nunoras/depot/pull/42".to_owned(),
        checks: Checks::Failing,
    }];
    assert!(
        !auto_merge_due(&failing, subject(&failing, "t1"), &head),
        "failing checks are never merged automatically"
    );

    let mut prerequisite = validated("t0", "c9");
    prerequisite.branch_head = Some(commit("c9"));
    let mut blocked = state.clone();
    blocked.tasks.insert(task_id("t0"), prerequisite);
    blocked
        .tasks
        .get_mut(&task_id("t1"))
        .expect("subject task")
        .dependencies = vec![Dependency {
        task: task_id("t0"),
        commit: commit("c1"),
    }];
    assert!(
        !auto_merge_due(&blocked, subject(&blocked, "t1"), &head),
        "a stale dependency pin is never merged automatically"
    );
}

#[test]
fn a_refused_auto_merge_is_noted_on_the_task_and_forgotten_once_the_pull_request_leaves_it() {
    run(vec![
        case(
            "a refused merge is noted on the task",
            state(vec![pr_open("t1", "c1", 42)]),
            vec![fact(
                1_000,
                refused_merge("t1", "the forge refused the merge"),
            )],
        )
        .when("t1", TaskState::PrOpen, vec![Action::RenderChecklist])
        .checking(|state| {
            subject(state, "t1").merge_refused.as_deref() == Some("the forge refused the merge")
        }),
        case(
            "a new head keeps the last refusal until the next attempt",
            state(vec![with_refusal(
                pr_open("t1", "c1", 42),
                "the forge refused the merge",
            )]),
            vec![fact(
                3_000,
                FactKind::BranchPushed {
                    task: task_id("t1"),
                    commit: commit("c2"),
                },
            )],
        )
        .when("t1", TaskState::PrOpen, vec![Action::RenderChecklist])
        .checking(|state| {
            subject(state, "t1").merge_refused.as_deref() == Some("the forge refused the merge")
        }),
        case(
            "a closed pull request forgets the refusal",
            state(vec![with_refusal(
                pr_open("t1", "c1", 42),
                "the forge refused the merge",
            )]),
            vec![fact(
                4_000,
                FactKind::PullRequestClosedUnmerged {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Cancelled,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| subject(state, "t1").merge_refused.is_none()),
        case(
            "a landed task forgets the refusal",
            state(vec![with_refusal(
                pr_open("t1", "c1", 42),
                "the forge refused the merge",
            )]),
            vec![fact(
                5_000,
                FactKind::PullRequestMerged {
                    task: task_id("t1"),
                    commit: commit("c1"),
                },
            )],
        )
        .when("t1", TaskState::Landed, vec![Action::RenderChecklist])
        .checking(|state| subject(state, "t1").merge_refused.is_none()),
        case(
            "a stopped task forgets the refusal",
            state(vec![with_refusal(
                pr_open("t1", "c1", 42),
                "the forge refused the merge",
            )]),
            vec![fact(
                6_000,
                FactKind::TaskCancelled {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Cancelled, vec![Action::RenderChecklist])
        .checking(|state| subject(state, "t1").merge_refused.is_none()),
    ]);
}

#[test]
fn a_branch_behind_the_validated_commit_owes_the_push() {
    let mut task = pr_open("t1", "c1", 42);
    assert_eq!(task.push_owed(), None, "a pushed branch owes no push");

    task.validations.push(ValidationRecord {
        command: "cargo test".to_owned(),
        commit: commit("c2"),
        exit_code: 0,
        duration: Duration::from_secs(5),
        output_tail: "ok".to_owned(),
    });
    assert_eq!(
        task.push_owed(),
        Some(&commit("c2")),
        "a branch behind its newest passing validation owes that push"
    );
    assert_eq!(
        task.validated_commit(),
        None,
        "the branch has not caught up to the validated commit"
    );

    let unpushed = validated("t1", "c1");
    assert_eq!(unpushed.push_owed(), Some(&commit("c1")));
    assert_eq!(unpushed.validated_commit(), Some(&commit("c1")));
}

#[test]
fn a_merge_with_no_validated_revision_to_match_is_held_rather_than_landed() {
    run(vec![
        case(
            "a merged fact for a task with no passing validation is held for a person",
            state(vec![with_attempt(
                with_pull_request(task("t1", TaskState::PrOpen), 42, Checks::Passing),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                1_000,
                FactKind::PullRequestMerged {
                    task: task_id("t1"),
                    commit: commit("c1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| {
            subject(state, "t1").validated_commit().is_none()
                && subject(state, "t1")
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.worktree.clone())
                    == Some(lease("w1"))
        }),
    ]);
}

#[test]
fn a_merge_of_an_unvalidated_revision_is_held_rather_than_landed() {
    let mut task = with_pull_request(validated("t1", "c1"), 42, Checks::Passing);
    task.state = TaskState::PrOpen;
    task.branch_head = Some(commit("c1"));
    run(vec![
        case(
            "a merged head that is not the validated commit is held for a person",
            state(vec![with_attempt(
                task,
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                1_000,
                FactKind::PullRequestMerged {
                    task: task_id("t1"),
                    commit: commit("c2"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
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

#[test]
fn merge_only_lands_from_pr_open() {
    run(vec![
        case(
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
        }),
    ]);
}

#[test]
fn revalidation_with_an_existing_pr_only_pushes() {
    run(vec![
        case(
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
        .checking(|state| subject(state, "t1").links.len() == 1),
    ]);
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
            dispatch_profile: None,
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
            dispatch_profile: None,
            dependencies: edges,
            base_dependency: Some(task_id("b")),
        },
    );
    let (next, actions) = reduce(&base(), &accepted);
    assert_eq!(actions, vec![Action::RenderChecklist]);
    let recorded = subject(&next, "t1");
    assert_eq!(recorded.base_dependency, Some(task_id("b")));
    assert_eq!(recorded.dependencies.len(), 2);

    run(vec![
        case(
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
        ),
    ]);
}

#[test]
fn liveness_gone_does_not_demote_landed_tasks() {
    run(vec![
        case(
            "a gone signal after land is ignored",
            state(vec![with_attempt(
                task("t1", TaskState::Landed),
                Attempt {
                    outcome: AttemptOutcome::InFlight,
                    session: Some(session("s1")),
                    finished_at: None,
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                1_000,
                FactKind::WorkerLivenessChanged {
                    task: task_id("t1"),
                    liveness: Liveness::Gone,
                },
            )],
        )
        .when("t1", TaskState::Landed, vec![])
        .checking(|state| holds(state, "t1", AttemptOutcome::InFlight)),
    ]);
}

#[test]
fn pull_request_opened_from_validated_closes_a_live_attempt() {
    run(vec![
        case(
            "opening a pr from validated stops a lingering open session",
            state(vec![with_attempt(
                validated("t1", "c1"),
                Attempt {
                    outcome: AttemptOutcome::InFlight,
                    session: Some(session("s1")),
                    worktree: Some(lease("w1")),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(
                1_000,
                FactKind::PullRequestOpened {
                    task: task_id("t1"),
                    number: 9,
                    url: "https://example.com/9".to_owned(),
                },
            )],
        )
        .when(
            "t1",
            TaskState::PrOpen,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            holds(state, "t1", AttemptOutcome::Submitted)
                && subject(state, "t1").pull_request().is_some()
        }),
    ]);
}

#[test]
fn pull_request_closed_unmerged_stops_a_live_session() {
    run(vec![
        case(
            "closing an open pr stops the session before cancelling",
            state(vec![with_pull_request(
                with_attempt(
                    task("t1", TaskState::PrOpen),
                    Attempt {
                        outcome: AttemptOutcome::InFlight,
                        session: Some(session("s1")),
                        worktree: Some(lease("w1")),
                        ..attempt(BUILD)
                    },
                ),
                42,
                Checks::Failing,
            )]),
            vec![fact(
                1_000,
                FactKind::PullRequestClosedUnmerged {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Cancelled,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                hold("t1"),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| holds(state, "t1", AttemptOutcome::Stopped)),
    ]);
}

#[test]
fn retry_exhausted_gates_and_stops_in_flight_work() {
    run(vec![
        case(
            "an in-flight retry exhaustion stops the session",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![fact(
                1_000,
                FactKind::RetryExhausted {
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
        .checking(|state| holds(state, "t1", AttemptOutcome::Failed)),
        case(
            "a landed task ignores retry exhaustion",
            state(vec![task("t1", TaskState::Landed)]),
            vec![fact(
                2_000,
                FactKind::RetryExhausted {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Landed, vec![]),
        case(
            "an approved retrying task fails when retries are exhausted",
            state(vec![with_retry(
                task("t1", TaskState::Approved),
                "fallback-profile",
                at(31_000),
            )]),
            vec![fact(
                3_000,
                FactKind::RetryExhausted {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| subject(state, "t1").retry.is_none()),
    ]);
}

#[test]
fn merge_closes_an_open_attempt_before_landing() {
    run(vec![
        case(
            "landing stops a lingering open session on the pr",
            state(vec![with_attempt(
                pr_open("t1", "merge-commit", 42),
                Attempt {
                    outcome: AttemptOutcome::InFlight,
                    session: Some(session("s1")),
                    worktree: Some(lease("w1")),
                    ..attempt(BUILD)
                },
            )]),
            vec![fact(1_000, merged("t1"))],
        )
        .when(
            "t1",
            TaskState::Landed,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                release("t1", "w1"),
                Action::RenderChecklist,
            ],
        )
        .checking(|state| {
            holds(state, "t1", AttemptOutcome::Submitted)
                && subject(state, "t1")
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.worktree.is_none())
        }),
    ]);
}

#[test]
fn pull_request_opened_during_validation_only_attaches_the_link() {
    run(vec![
        case(
            "a pr opened while validating keeps validating and later failure holds",
            state(vec![with_attempt(
                task("t1", TaskState::Validating),
                Attempt {
                    outcome: AttemptOutcome::Submitted,
                    worktree: Some(lease("w1")),
                    finished_at: Some(at(0)),
                    ..attempt(BUILD)
                },
            )]),
            vec![
                fact(
                    1_000,
                    FactKind::PullRequestOpened {
                        task: task_id("t1"),
                        number: 3,
                        url: "https://example.com/3".to_owned(),
                    },
                ),
                fact(2_000, failed("t1", "cb")),
            ],
        )
        .when(
            "t1",
            TaskState::Failed,
            vec![hold("t1"), Action::RenderChecklist],
        )
        .checking(|state| {
            subject(state, "t1").pull_request().is_some()
                && subject(state, "t1").state == TaskState::Failed
        }),
    ]);
}

#[test]
fn answering_one_of_several_questions_keeps_waiting() {
    run(vec![
        case(
            "a second unanswered question blocks resume",
            state(vec![{
                let mut task = with_question(
                    running_with_session("t1", "s1", "w1"),
                    TaskState::WaitingOnQuestion,
                    "first?",
                );
                task.questions.push(Question {
                    text: "second?".to_owned(),
                    asked_at: at(1_500),
                    answer: None,
                });
                task
            }]),
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
            TaskState::WaitingOnQuestion,
            vec![Action::RenderChecklist],
        )
        .checking(|state| {
            subject(state, "t1")
                .questions
                .iter()
                .filter(|question| question.answer.is_none())
                .count()
                == 1
        }),
    ]);
}

#[test]
fn cancel_before_session_id_still_stops_the_launched_worker() {
    run(vec![
        case(
            "cancelling a session-less running attempt emits stop",
            state(vec![running("t1")]),
            vec![fact(
                1_000,
                FactKind::TaskCancelled {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Cancelled,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                Action::RenderChecklist,
            ],
        )
        .checking(|state| holds(state, "t1", AttemptOutcome::Stopped)),
    ]);
}

#[test]
fn cancelling_releases_any_lease_the_task_acquired() {
    run(vec![
        case(
            "cancelling a running attempt with a session and a lease stops the worker and releases the lease",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![fact(
                1_000,
                FactKind::TaskCancelled {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Cancelled,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                Action::ReleaseWorktree {
                    task: task_id("t1"),
                    lease: lease("w1"),
                },
                Action::RenderChecklist,
            ],
        )
        .checking(|state| subject(state, "t1").attempts[0].worktree.is_none()),
        case(
            "cancelling a running attempt that holds a lease but never launched releases the lease",
            state(vec![running_with_lease("t1", "w1")]),
            vec![fact(
                1_000,
                FactKind::TaskCancelled {
                    task: task_id("t1"),
                },
            )],
        )
        .when(
            "t1",
            TaskState::Cancelled,
            vec![
                Action::StopSession {
                    task: task_id("t1"),
                },
                Action::ReleaseWorktree {
                    task: task_id("t1"),
                    lease: lease("w1"),
                },
                Action::RenderChecklist,
            ],
        )
        .checking(|state| subject(state, "t1").attempts[0].worktree.is_none()),
    ]);
}

#[test]
fn daemon_restart_renders_when_attempts_become_unknown() {
    run(vec![
        case(
            "restart reconciliation renders the unknown outcome",
            state(vec![running_with_session("t1", "s1", "w1")]),
            vec![fact(1_000, FactKind::DaemonRestarted)],
        )
        .when("t1", TaskState::Running, vec![Action::RenderChecklist])
        .checking(|state| holds(state, "t1", AttemptOutcome::Unknown)),
    ]);
}

#[test]
fn branch_push_that_invalidates_validation_renders() {
    run(vec![
        case(
            "moving the branch head off a validated commit renders",
            state(vec![validated("t1", "c1")]),
            vec![fact(
                1_000,
                FactKind::BranchPushed {
                    task: task_id("t1"),
                    commit: commit("c2"),
                },
            )],
        )
        .when("t1", TaskState::Validated, vec![Action::RenderChecklist])
        .checking(|state| subject(state, "t1").validated_commit().is_none()),
    ]);
}

fn coordinator(id: &str, started_at: u64, context_tokens: u64) -> CoordinatorSession {
    CoordinatorSession {
        session: session(id),
        started_at: at(started_at),
        context_tokens,
    }
}

fn measured(tokens: u64) -> FactKind {
    FactKind::CoordinatorContextMeasured { tokens }
}

fn session_started(id: &str) -> FactKind {
    FactKind::CoordinatorSessionStarted {
        session: session(id),
    }
}

#[test]
fn rule_12_a_coordinator_session_rotates_past_the_configured_context_size() {
    let mut state = base();
    state.limits.coordinator_context_tokens = 100;
    state.coordinator = Some(coordinator("c1", 1_000, 0));

    let (next, actions) = reduce(&state, &fact(2_000, measured(40)));
    assert_eq!(
        actions,
        Vec::new(),
        "a measurement below the limit acts on nothing"
    );
    assert_eq!(next.coordinator, Some(coordinator("c1", 1_000, 40)));

    let (next, actions) = reduce(&next, &fact(3_000, measured(99)));
    assert_eq!(actions, Vec::new());
    assert_eq!(next.coordinator, Some(coordinator("c1", 1_000, 99)));

    let (next, actions) = reduce(&next, &fact(4_000, measured(100)));
    assert_eq!(
        actions,
        vec![Action::RotateCoordinator {
            session: session("c1"),
        }],
        "hitting the limit rotates the session"
    );
    assert!(
        next.coordinator.is_none(),
        "a rotated project holds no session until a fresh one starts"
    );

    let (next, actions) = reduce(&next, &fact(5_000, session_started("c2")));
    assert_eq!(
        actions,
        Vec::new(),
        "a fresh session is launched by the rotation itself"
    );
    assert_eq!(next.coordinator, Some(coordinator("c2", 5_000, 0)));
}

#[test]
fn rule_13_a_coordinator_measurement_is_ignored_without_a_live_session() {
    let mut state = base();
    state.limits.coordinator_context_tokens = 10;

    let (next, actions) = reduce(&state, &fact(1_000, measured(9_999)));
    assert_eq!(actions, Vec::new());
    assert!(next.coordinator.is_none());

    state.limits.coordinator_context_tokens = 0;
    state.coordinator = Some(coordinator("c1", 2_000, 0));
    let (next, actions) = reduce(&state, &fact(3_000, measured(u64::MAX)));
    assert_eq!(actions, Vec::new(), "a zero limit never rotates");
    assert_eq!(next.coordinator, Some(coordinator("c1", 2_000, u64::MAX)));
}

#[test]
fn rule_14_an_unmapped_role_is_refused_rather_than_defaulted() {
    let mut unmapped = base();
    unmapped
        .tasks
        .insert(task_id("t1"), task("t1", TaskState::Proposed));
    unmapped.profiles.remove(&Role::Build);

    let (next, actions) = reduce(&unmapped, &fact(1_000, approved("t1")));

    assert_eq!(next.tasks[&task_id("t1")].state, TaskState::Approved);
    assert_eq!(
        actions,
        vec![queue("t1", None), Action::RenderChecklist],
        "a role with no profile queues and never launches"
    );
    assert!(next.tasks[&task_id("t1")].attempts.is_empty());

    let mut fallbacks = base();
    fallbacks
        .tasks
        .insert(task_id("t1"), task("t1", TaskState::Proposed));
    fallbacks.profiles.remove(&Role::Build);
    fallbacks.fallback_profiles = vec![profile("fallback-profile")];

    let (next, actions) = reduce(&fallbacks, &fact(2_000, approved("t1")));

    assert_eq!(
        actions,
        vec![queue("t1", None), Action::RenderChecklist],
        "the fallback list only replaces a profile a task already holds, so it never fills a missing mapping"
    );
    assert_eq!(next.tasks[&task_id("t1")].state, TaskState::Approved);
}

#[test]
fn rule_15_an_acknowledgement_fades_a_failed_or_cancelled_task() {
    fn acknowledged_at(millis: u64) -> impl Fn(&ProjectState) -> bool {
        move |state: &ProjectState| subject(state, "t1").acknowledged_at == Some(at(millis))
    }

    run(vec![
        case(
            "a failed task records the acknowledgement",
            state(vec![task("t1", TaskState::Failed)]),
            vec![fact(
                1_000,
                FactKind::TaskAcknowledged {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Failed, vec![Action::RenderChecklist])
        .checking(acknowledged_at(1_000)),
        case(
            "a cancelled task records the acknowledgement",
            state(vec![task("t1", TaskState::Cancelled)]),
            vec![fact(
                2_000,
                FactKind::TaskAcknowledged {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Cancelled, vec![Action::RenderChecklist])
        .checking(acknowledged_at(2_000)),
        case(
            "a second acknowledgement changes nothing",
            state(vec![{
                let mut task = task("t1", TaskState::Failed);
                task.acknowledged_at = Some(at(1_000));
                task
            }]),
            vec![fact(
                3_000,
                FactKind::TaskAcknowledged {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Failed, vec![])
        .checking(acknowledged_at(1_000)),
        case(
            "a running task cannot be acknowledged",
            state(vec![running("t1")]),
            vec![fact(
                4_000,
                FactKind::TaskAcknowledged {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Running, vec![])
        .checking(|state| subject(state, "t1").acknowledged_at.is_none()),
    ]);
}

#[test]
fn rule_16_a_failed_or_cancelled_task_fades_when_superseded_or_acknowledged() {
    fn faded(want: bool) -> impl Fn(&ProjectState) -> bool {
        move |state: &ProjectState| task_faded(state, subject(state, "t1")) == want
    }

    run(vec![
        case(
            "a fresh failure does not fade",
            state(vec![task("t1", TaskState::Failed)]),
            vec![fact(1_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Failed, vec![])
        .checking(faded(false)),
        case(
            "a running replacement filed against the failure fades it",
            state(vec![
                task("t1", TaskState::Failed),
                with_base(task("t2", TaskState::Running), "t1"),
            ]),
            vec![fact(2_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Failed, vec![])
        .checking(faded(true)),
        case(
            "a dependency on the failure also fades it",
            state(vec![
                task("t1", TaskState::Cancelled),
                depending_on(task("t2", TaskState::Proposed), "t1", "c1"),
            ]),
            vec![fact(3_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Cancelled, vec![])
        .checking(faded(true)),
        case(
            "another failure filed against the failure does not fade it",
            state(vec![
                task("t1", TaskState::Failed),
                with_base(task("t2", TaskState::Failed), "t1"),
            ]),
            vec![fact(4_000, FactKind::Polled)],
        )
        .when("t1", TaskState::Failed, vec![])
        .checking(faded(false)),
        case(
            "an acknowledgement fades it without a replacement",
            state(vec![task("t1", TaskState::Failed)]),
            vec![fact(
                5_000,
                FactKind::TaskAcknowledged {
                    task: task_id("t1"),
                },
            )],
        )
        .when("t1", TaskState::Failed, vec![Action::RenderChecklist])
        .checking(faded(true)),
    ]);
}
