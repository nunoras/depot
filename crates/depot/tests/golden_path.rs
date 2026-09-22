#[path = "support/dispatch.rs"]
mod dispatch;
mod support;

use depot_core::{
    AttemptOutcome, Checks, CommitId, Dependency, ProfileId, SessionId, TaskId, TaskState,
    Timestamp, ValidationRecord, WorktreeLease,
};
use depotd::InstanceLock;
use depotd::evidence::MANAGED_MARKER;
use support::git;
use support::{
    ACCOUNT, BASE, BRANCH, BRANCH_TWO, Golden, HARNESS, LEASE, LEASE_TWO, MODEL, PROFILE,
    REPOSITORY, SESSION, SLUG, TASK, TASK_TWO, Validation, calls_to, validated_task,
};

const PROPOSED: &str = "task_proposed";
const APPROVED: &str = "task_approved";
const ACQUIRED: &str = "worktree_acquired";
const ACQUIRE_REQUESTED: &str = "worktree_acquire_requested";
const LAUNCH_REQUESTED: &str = "worker_turn_launch_requested";
const TURN_STARTED: &str = "worker_turn_started";
const LIVENESS: &str = "worker_liveness_changed";
const ASKED: &str = "question_asked";
const ANSWERED: &str = "question_answered";
const SUBMITTED: &str = "worker_submitted";
const VALIDATED: &str = "validation_finished";
const PUSHED: &str = "branch_pushed";
const OPENED: &str = "pull_request_opened";
const CHECKS: &str = "pull_request_checks_changed";
const MERGED: &str = "pull_request_merged";
const CLOSED_UNMERGED: &str = "pull_request_closed_unmerged";
const RELEASED: &str = "worktree_released";
const RELEASE_HELD: &str = "worktree_release_held";
const MERGE_REFUSED: &str = "pull_request_merge_refused";
const PUSH_FAILED: &str = "push_failed";
const REBASE_SCHEDULED: &str = "rebase_scheduled";
const RESTARTED: &str = "daemon_restarted";

#[test]
fn the_whole_journey_runs_from_proposal_to_a_released_worktree() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();

    let added = golden.depot_ok(&[
        "task",
        "add",
        "--title",
        "Wire the store",
        "--intent",
        "Persist the records in sqlite.",
        "--role",
        "build",
        "--project",
        SLUG,
    ]);
    assert_eq!(added, format!("added {TASK}\n"));

    let held = golden.status();
    assert!(held.contains("Held - awaiting approval (1)"), "{held}");
    assert!(held.contains("waits on: approval"), "{held}");
    assert_eq!(
        held,
        golden.checklist(),
        "the checklist is rendered from the records"
    );

    assert_eq!(
        golden.depot_ok(&["task", "approve", TASK, "--project", SLUG]),
        format!("approved {TASK}\n")
    );
    assert_eq!(
        golden.task().state,
        TaskState::Running,
        "approving a task starts it; the daemon then leases a worktree and launches the worker"
    );

    daemon.tick().expect("the daemon launches the worker");
    daemon.tick().expect("a further poll changes nothing");

    let launched = golden.task();
    assert_eq!(launched.state, TaskState::Running);
    assert_eq!(launched.attempts.len(), 1);
    assert_eq!(launched.attempts[0].session, Some(SessionId::new(SESSION)));
    assert_eq!(
        launched.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE))
    );
    assert_eq!(launched.attempts[0].profile, ProfileId::new(PROFILE));
    assert!(
        golden
            .status()
            .contains(&format!("a worker turn in worktree lease `{LEASE}`"))
    );

    let launch = golden.boxr.calls_to("--harness");
    assert_eq!(launch.len(), 1, "one worker is launched, {launch:?}");
    let brief = golden
        .store
        .coordinator_context(&golden.project)
        .expect("the coordinator context")
        .brief(&launched)
        .expect("the brief is rendered");
    assert_eq!(
        launch[0],
        vec![
            "--harness",
            HARNESS,
            "--model",
            MODEL,
            "--effort",
            "high",
            "--account",
            ACCOUNT,
            "--kind",
            "build",
            "--detach",
            &brief,
        ],
        "the launch names the resolved profile and carries the brief"
    );
    assert_eq!(
        golden.treehouse.calls(),
        vec![
            format!("get --lease --json --lease-holder depot:{TASK}"),
            "status --json".to_string(),
        ]
    );

    let submitted = golden.worker_commits_and_submits();
    assert_eq!(
        submitted.status.code(),
        Some(0),
        "the worker script failed: {}",
        support::stderr(&submitted)
    );
    assert!(
        support::stdout(&submitted).ends_with(&format!("submitted {TASK}\n")),
        "the worker reports its submission: {}",
        support::stdout(&submitted)
    );
    let commit = golden.head();
    assert_eq!(golden.task().state, TaskState::Validating);

    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates the commit and opens the pull request");

    let opened = golden.task();
    assert_eq!(opened.state, TaskState::PrOpen);
    assert_eq!(
        opened
            .pull_request()
            .map(|(number, _, checks)| (number, checks)),
        Some((1, depot_core::Checks::Passing))
    );
    assert_eq!(opened.validations.len(), 1);
    assert_eq!(opened.validations[0].commit, CommitId::new(commit.clone()));
    assert_eq!(opened.validations[0].exit_code, 0);
    assert_eq!(
        opened.validations[0].output_tail.trim(),
        support::PASSING_OUTPUT
    );
    assert_eq!(opened.branch_head, Some(CommitId::new(commit.clone())));
    assert_eq!(
        git::git(
            &golden.origin,
            &["rev-parse", &format!("refs/heads/{BRANCH}")]
        )
        .trim(),
        commit
    );

    let requests = golden.forge.requests();
    let pull_request = requests
        .iter()
        .find(|request| request.method == "POST")
        .expect("the daemon opened a pull request");
    assert_eq!(pull_request.path, "/repos/nunoras/depot/pulls");
    assert!(pull_request.body.contains("\"title\":\"Wire the store\""));
    assert!(!pull_request.body.contains("Persist the records in sqlite."));
    assert!(pull_request.body.contains(&format!("`{commit}`")));
    assert!(
        requests
            .iter()
            .any(|request| request.path.ends_with("/check-runs"))
    );

    let open = golden.status();
    assert!(open.contains("Pull request open (1)"), "{open}");
    assert!(open.contains("checks passing"), "{open}");
    assert!(!open.contains("pull request merged"), "{open}");
    assert_eq!(open, golden.checklist());

    golden.script_merge(&commit);
    daemon.tick().expect("the daemon observes the merge");

    let landed = golden.task();
    assert_eq!(landed.state, TaskState::Landed);
    assert_eq!(golden.pull_requests_opened(), 1);
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "the worktree returns to the pool once"
    );
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "get").len(),
        1,
        "the task leases one worktree"
    );

    let before = golden.events().len();
    let forge_calls = golden.forge.requests().len();
    daemon
        .tick()
        .expect("a poll after the merge changes nothing");
    assert_eq!(golden.task().state, TaskState::Landed);
    assert_eq!(
        golden.events().len(),
        before + 1,
        "only the poll itself is recorded"
    );
    assert_eq!(golden.forge.requests().len(), forge_calls);
    assert_eq!(golden.pull_requests_opened(), 1);

    let final_checklist = golden.status_history();
    assert!(final_checklist.contains("Landed (1)"), "{final_checklist}");
    assert!(
        final_checklist.contains("waits on: nothing"),
        "{final_checklist}"
    );
    let default_checklist = golden.status();
    assert!(
        !default_checklist.contains("## Landed"),
        "{default_checklist}"
    );
    assert!(
        default_checklist.contains("1 landed"),
        "{default_checklist}"
    );
    assert_eq!(default_checklist, golden.checklist());

    assert_eq!(
        golden.state_history(TASK),
        vec![
            PROPOSED,
            APPROVED,
            ACQUIRED,
            TURN_STARTED,
            SUBMITTED,
            VALIDATED,
            PUSHED,
            OPENED,
            CHECKS,
            MERGED,
            RELEASED,
        ]
    );
    assert!(golden.history(TASK).contains(&LIVENESS.to_string()));
}

#[test]
fn a_worker_question_is_relayed_answered_and_the_worker_resumes_with_the_answer() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    assert_eq!(golden.task().state, TaskState::Running);

    golden.boxr.report_finished();
    let asked = golden.worker_commits_and_asks("Which store?");
    assert_eq!(
        asked.status.code(),
        Some(0),
        "the worker script failed: {}",
        support::stderr(&asked)
    );
    assert!(
        support::stdout(&asked).ends_with(&format!("asked {TASK}\n")),
        "the worker reports its question: {}",
        support::stdout(&asked)
    );
    assert!(
        support::stderr(&asked).contains("waiting on a question"),
        "the worker is notified: {}",
        support::stderr(&asked)
    );

    let waiting = golden.task();
    assert_eq!(waiting.state, TaskState::WaitingOnQuestion);
    assert_eq!(waiting.questions.len(), 1);
    assert_eq!(waiting.questions[0].text, "Which store?");
    assert!(waiting.questions[0].answer.is_none());

    let relayed = golden.status();
    assert!(
        relayed.contains("Needs you - waiting on an answer (1)"),
        "{relayed}"
    );
    assert!(relayed.contains("Which store?"), "{relayed}");
    assert_eq!(relayed, golden.checklist());

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(inbox.contains("## For the user (1)"), "{inbox}");
    assert!(inbox.contains("Which store?"), "{inbox}");
    assert!(inbox.contains("waiting_on_question"), "{inbox}");

    daemon
        .tick()
        .expect("the daemon sees the dead worker and closes the attempt");
    assert_eq!(golden.task().state, TaskState::WaitingOnQuestion);
    assert_eq!(
        golden.task().attempts[0].outcome,
        AttemptOutcome::AwaitingAnswer,
        "a dead worker holds no open turn while the answer is owed"
    );
    assert!(golden.boxr.calls_to("resume").is_empty());
    assert!(golden.boxr.calls_to("stop").is_empty());

    let observed = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        observed.contains("the worker is gone"),
        "the inbox reports the liveness the daemon observed, got\n{observed}"
    );
    assert!(
        !observed.contains("the worker is live"),
        "a paused task's dead worker is not reported as live, got\n{observed}"
    );
    assert!(
        observed.contains("waiting_on_question"),
        "the task still owes its answer, got\n{observed}"
    );

    assert_eq!(
        golden.depot_ok(&[
            "task",
            "answer",
            TASK,
            "--text",
            "sqlite in the depot home.",
            "--by",
            "user",
            "--project",
            SLUG,
        ]),
        format!("answered {TASK}\n")
    );
    assert_eq!(golden.task().state, TaskState::Running);

    const NEXT_SESSION: &str = "b3c7e2";
    golden.boxr.respond_launches(&[SESSION, NEXT_SESSION]);
    golden.boxr.report_running();
    daemon
        .tick()
        .expect("the daemon answers a dead session with a fresh worker");
    assert!(
        golden.boxr.calls_to("resume").is_empty(),
        "a dead session is never resumed"
    );
    let launches = golden.boxr.calls_to("--harness");
    assert_eq!(
        launches.len(),
        2,
        "a fresh worker is launched, {launches:?}"
    );
    let prompt = launches[1].last().expect("the launch prompt");
    assert!(
        prompt.contains("Which store?") && prompt.contains("sqlite in the depot home."),
        "the relaunch brief carries the question and the answer: {prompt}"
    );
    let relaunched = golden.task();
    assert_eq!(relaunched.attempts.len(), 2);
    assert_eq!(
        relaunched.attempts[0].outcome,
        AttemptOutcome::AwaitingAnswer
    );
    assert_eq!(
        relaunched.attempts[1].session,
        Some(SessionId::new(NEXT_SESSION))
    );
    assert_eq!(
        relaunched.attempts[1].worktree,
        Some(WorktreeLease::new(LEASE)),
        "the fresh turn runs on the same lease"
    );

    daemon
        .tick()
        .expect("a further poll does not launch the worker again");
    assert_eq!(golden.boxr.calls_to("--harness").len(), 2);

    let submitted = golden.worker_submits();
    assert_eq!(
        submitted.status.code(),
        Some(0),
        "the worker script failed: {}",
        support::stderr(&submitted)
    );
    golden.script_pull_request(&golden.head());
    daemon.tick().expect("the daemon validates and publishes");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    assert_eq!(
        golden.state_history(TASK),
        vec![
            PROPOSED,
            APPROVED,
            ACQUIRED,
            TURN_STARTED,
            ASKED,
            ANSWERED,
            TURN_STARTED,
            SUBMITTED,
            VALIDATED,
            PUSHED,
            OPENED,
            CHECKS,
        ]
    );
}

#[test]
fn a_settled_question_reaches_a_dead_session_through_a_fresh_turn() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.boxr.report_finished();

    let asked = golden.worker_asks_settled("Which store?");
    assert_eq!(
        asked.status.code(),
        Some(0),
        "the worker script failed: {}",
        support::stderr(&asked)
    );
    assert!(
        support::stdout(&asked).ends_with(&format!("asked {TASK}\n")),
        "the worker reports its question: {}",
        support::stdout(&asked)
    );
    assert!(
        !support::stderr(&asked).contains("waiting on a question"),
        "a question the coordinator settles is not relayed: {}",
        support::stderr(&asked)
    );

    let settled = golden.task();
    assert_eq!(settled.state, TaskState::Running);
    assert_eq!(settled.questions.len(), 1);
    assert!(settled.questions[0].answer.is_none());

    daemon
        .tick()
        .expect("the daemon sees the finished session and closes the attempt");

    let paused = golden.task();
    assert_eq!(paused.state, TaskState::Running);
    assert_eq!(
        paused.attempts[0].outcome,
        AttemptOutcome::AwaitingAnswer,
        "a finished session holds no open turn while the answer is owed"
    );
    let open = golden.status();
    assert!(!open.contains("Blocked"), "{open}");
    golden.status_matches_checklist(&open);
    assert!(golden.boxr.calls_to("resume").is_empty());

    assert_eq!(
        golden.depot_ok(&[
            "task",
            "answer",
            TASK,
            "--text",
            "sqlite in the depot home.",
            "--by",
            "user",
            "--project",
            SLUG,
        ]),
        format!("answered {TASK}\n")
    );

    const NEXT_SESSION: &str = "c41d9a";
    golden.boxr.respond_launches(&[SESSION, NEXT_SESSION]);
    golden
        .boxr
        .respond_status_sequence(&["running", "running", "finished"]);
    golden.boxr.report_running();
    daemon
        .tick()
        .expect("the daemon answers a dead session with a fresh worker");
    assert!(
        golden.boxr.calls_to("resume").is_empty(),
        "a dead session is never resumed"
    );
    let launches = golden.boxr.calls_to("--harness");
    assert_eq!(
        launches.len(),
        2,
        "a fresh worker is launched, {launches:?}"
    );
    let prompt = launches[1].last().expect("the launch prompt");
    assert!(
        prompt.contains("Which store?") && prompt.contains("sqlite in the depot home."),
        "the relaunch brief carries the question and the answer: {prompt}"
    );
    let relaunched = golden.task();
    assert_eq!(relaunched.attempts.len(), 2);
    assert_eq!(
        relaunched.attempts[1].session,
        Some(SessionId::new(NEXT_SESSION))
    );

    daemon
        .tick()
        .expect("a further poll does not launch the worker again");
    assert_eq!(golden.boxr.calls_to("--harness").len(), 2);

    assert_eq!(
        golden.state_history(TASK),
        vec![
            PROPOSED,
            APPROVED,
            ACQUIRED,
            TURN_STARTED,
            ASKED,
            ANSWERED,
            TURN_STARTED,
        ]
    );
}

#[test]
fn the_checklist_names_no_missing_daemon_however_long_the_test_runs() {
    let golden = Golden::new(Validation::Passing);
    golden.propose();
    golden
        .daemon()
        .tick()
        .expect("the daemon launches the worker");

    let later = depot_core::Timestamp::from_millis(4_102_444_800_000);
    let rendered = depotd::render_status_at(
        &golden.home,
        &depotd::StatusSelection::Project(SLUG.to_string()),
        false,
        later,
    )
    .expect("the status renders");

    assert!(
        !rendered.contains("no daemon is driving this project"),
        "a live daemon covers the project at any later instant: {rendered}"
    );
}

#[test]
fn a_worker_resumes_only_once_every_open_question_is_answered() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.boxr.report_finished();

    let asked = golden.worker_asks_twice("Which store?", "Which port?");
    assert_eq!(
        asked.status.code(),
        Some(0),
        "the worker script failed: {}",
        support::stderr(&asked)
    );

    let waiting = golden.task();
    assert_eq!(waiting.state, TaskState::WaitingOnQuestion);
    assert_eq!(waiting.questions.len(), 2);
    assert!(
        waiting
            .questions
            .iter()
            .all(|question| question.answer.is_none())
    );

    assert_eq!(
        golden.depot_ok(&[
            "task",
            "answer",
            TASK,
            "--text",
            "8080.",
            "--by",
            "user",
            "--project",
            SLUG,
        ]),
        format!("answered {TASK}\n")
    );

    daemon
        .tick()
        .expect("the daemon leaves the worker stopped while a question is unanswered");
    assert_eq!(golden.task().state, TaskState::WaitingOnQuestion);
    assert!(
        golden.boxr.calls_to("resume").is_empty(),
        "an unanswered question holds the worker"
    );

    assert_eq!(
        golden.depot_ok(&[
            "task",
            "answer",
            TASK,
            "--text",
            "sqlite in the depot home.",
            "--by",
            "user",
            "--project",
            SLUG,
        ]),
        format!("answered {TASK}\n")
    );
    assert_eq!(golden.task().state, TaskState::Running);

    golden.boxr.report_running();
    daemon
        .tick()
        .expect("the daemon resumes once every question is answered");
    let resumed = golden.boxr.calls_to("resume");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        resumed[0],
        vec![
            "resume",
            "--detach",
            SESSION,
            "Your question \"Which store?\" was answered: sqlite in the depot home. Your question \"Which port?\" was answered: 8080. Continue the task.",
        ],
        "both answers reach the worker in one resume turn"
    );
    let resumed_session = golden
        .task()
        .attempts
        .last()
        .and_then(|attempt| attempt.session.clone());
    assert_eq!(
        resumed_session,
        Some(SessionId::new(format!("{SESSION}-child"))),
        "the daemon records the detached child boxr printed, not the parent"
    );

    daemon
        .tick()
        .expect("a further poll does not resume the worker again");
    assert_eq!(golden.boxr.calls_to("resume").len(), 1);
}

#[test]
fn a_failed_forge_read_leaves_the_task_open_and_retries_on_a_later_tick() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    golden.fail_pull_request_read();
    daemon
        .tick()
        .expect("a transient forge read failure does not stop the daemon");

    let open = golden.task();
    assert_eq!(open.state, TaskState::PrOpen);
    assert_eq!(open.attempts[0].outcome, AttemptOutcome::Submitted);
    assert!(!golden.history(TASK).contains(&MERGED.to_string()));
    assert!(golden.status().contains("Pull request open (1)"));

    golden.script_merge(&commit);
    daemon
        .tick()
        .expect("the daemon observes the merge on a later tick");

    let landed = golden.task();
    assert_eq!(landed.state, TaskState::Landed);
    assert!(golden.history(TASK).contains(&MERGED.to_string()));
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "the worktree returns to the pool once"
    );
}

#[test]
fn a_failed_validation_opens_no_pull_request_and_keeps_the_branch() {
    let golden = Golden::new(Validation::Failing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    let submitted = golden.worker_commits_and_submits();
    assert_eq!(submitted.status.code(), Some(0));
    let commit = golden.head();
    daemon.tick().expect("the daemon validates the commit");

    let failed = golden.task();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.validations.len(), 1);
    assert_eq!(failed.validations[0].commit, CommitId::new(commit.clone()));
    assert_eq!(failed.validations[0].exit_code, 1);
    assert!(
        failed.validations[0]
            .output_tail
            .contains(support::FAILING_OUTPUT),
        "the validation output is attached: {}",
        failed.validations[0].output_tail
    );
    assert_eq!(failed.branch_head, None);
    assert_eq!(
        failed.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE)),
        "a failed validation keeps the worktree"
    );

    assert_eq!(golden.pull_requests_opened(), 0);
    assert!(
        golden
            .forge
            .requests()
            .iter()
            .all(|request| request.method == "GET"),
        "no pull request is opened"
    );
    assert!(calls_to(&golden.treehouse.calls(), "return").is_empty());
    assert_eq!(calls_to(&golden.treehouse.calls(), "get").len(), 1);
    assert_eq!(git::head(&golden.lease), commit);
    assert_eq!(
        git::git(&golden.lease, &["branch", "--show-current"]).trim(),
        BRANCH
    );

    let history = golden.status_history();
    assert!(history.contains("Failed (1)"), "{history}");
    assert!(history.contains("waits on: a person"), "{history}");
    assert!(history.contains("exited 1"), "{history}");
    assert!(history.contains(&commit), "{history}");
    let blocked = golden.status();
    assert!(!blocked.contains("## Failed"), "{blocked}");
    assert!(blocked.contains("1 failed"), "{blocked}");
    assert_eq!(blocked, golden.checklist());

    assert_eq!(
        golden.state_history(TASK),
        vec![
            PROPOSED,
            APPROVED,
            ACQUIRED,
            TURN_STARTED,
            SUBMITTED,
            VALIDATED
        ]
    );
}

#[test]
fn a_restart_with_a_worktree_intent_completes_it_from_the_pool() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.reject_worktree_acquire();
    golden.propose();

    daemon
        .tick()
        .expect("a failing acquire is deferred rather than fatal");
    assert!(
        golden
            .history(TASK)
            .contains(&ACQUIRE_REQUESTED.to_string())
    );
    assert_eq!(calls_to(&golden.treehouse.calls(), "get").len(), 1);

    golden.allow_worktree_acquire();
    daemon
        .recover()
        .expect("recovery completes the acquire intent from the pool");

    let task = golden.task();
    assert_eq!(task.state, TaskState::Running);
    assert_eq!(task.attempts[0].worktree, Some(WorktreeLease::new(LEASE)));
    assert_eq!(task.attempts[0].session, Some(SessionId::new(SESSION)));
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "get").len(),
        1,
        "a lease that already exists is never acquired twice"
    );
    assert_eq!(golden.boxr.calls_to("--harness").len(), 1);
}

#[test]
fn a_restart_without_a_leased_worktree_retries_the_acquire() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.reject_worktree_acquire();
    golden.free_lease();
    golden.propose();

    daemon
        .tick()
        .expect("a failing acquire is deferred rather than fatal");
    assert_eq!(calls_to(&golden.treehouse.calls(), "get").len(), 1);

    golden.allow_worktree_acquire();
    daemon
        .recover()
        .expect("the launch waits until the pool reports the retried lease");

    let retried = golden.task();
    assert_eq!(retried.state, TaskState::Running);
    assert_eq!(
        retried.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE))
    );
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "get").len(),
        2,
        "a lease that does not exist is acquired again rather than suppressed"
    );

    golden.hold_lease();
    daemon
        .recover()
        .expect("the launch proceeds once the pool reports the lease");

    let running = golden.task();
    assert_eq!(running.attempts[0].session, Some(SessionId::new(SESSION)));
    assert_eq!(golden.boxr.calls_to("--harness").len(), 1);
}

#[test]
fn a_redirect_queued_mid_turn_reaches_the_worker_at_the_next_turn() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    assert_eq!(
        golden.depot_ok(&[
            "task",
            "redirect",
            TASK,
            "--text",
            "Skip the migration; the schema is frozen.",
            "--project",
            SLUG,
        ]),
        format!("redirected {TASK}\n")
    );
    assert_eq!(golden.task().state, TaskState::Running);
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_redirected".to_string())
    );

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("a new direction was queued for the worker"),
        "{inbox}"
    );
    assert!(
        inbox.contains("Skip the migration"),
        "the inbox shows the direction: {inbox}"
    );

    golden.boxr.report_running();
    daemon
        .tick()
        .expect("a running worker is not interrupted mid-turn");
    assert!(golden.boxr.calls_to("resume").is_empty());

    golden.boxr.report_finished();
    const REDIRECT_SESSION: &str = "d94b02";
    golden.boxr.respond_launches(&[SESSION, REDIRECT_SESSION]);
    golden
        .boxr
        .respond_status_sequence(&["running", "running", "finished", "finished", "finished"]);
    golden.boxr.respond(
        "status",
        &format!("session: {REDIRECT_SESSION}\nstate: running\n"),
        "",
        0,
    );
    daemon
        .tick()
        .expect("the daemon delivers the redirect when the turn ends");
    assert!(
        golden.boxr.calls_to("resume").is_empty(),
        "a dead session is never resumed"
    );
    let launches = golden.boxr.calls_to("--harness");
    assert_eq!(
        launches.len(),
        2,
        "a fresh worker is launched, {launches:?}"
    );
    let prompt = launches[1].last().expect("the launch prompt");
    assert!(
        prompt.contains("Skip the migration"),
        "the relaunch brief carries the redirect: {prompt}"
    );
    assert_eq!(golden.task().state, TaskState::Running);

    daemon
        .tick()
        .expect("a further poll does not deliver the redirect twice");
    assert_eq!(golden.boxr.calls_to("--harness").len(), 2);
}

#[test]
fn a_redirect_is_refused_for_a_task_that_is_not_running() {
    let golden = Golden::new(Validation::Passing);
    golden.depot_ok(&[
        "task",
        "add",
        "--title",
        "Wire the store",
        "--intent",
        "Persist the records in sqlite.",
        "--role",
        "build",
        "--project",
        SLUG,
    ]);
    let refused = golden.depot(&[
        "task",
        "redirect",
        TASK,
        "--text",
        "too late",
        "--project",
        SLUG,
    ]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        support::stderr(&refused).contains("cannot be redirected"),
        "{}",
        support::stderr(&refused)
    );
}

#[test]
fn a_redirect_at_a_finished_turn_needs_queue_and_reports_the_receipt() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.boxr.report_finished();
    let refused = golden.depot(&[
        "task",
        "redirect",
        TASK,
        "--text",
        "Skip the migration.",
        "--project",
        SLUG,
    ]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        support::stderr(&refused).contains("--queue"),
        "{}",
        support::stderr(&refused)
    );

    assert_eq!(
        golden.depot_ok(&[
            "task",
            "redirect",
            TASK,
            "--text",
            "Skip the migration.",
            "--queue",
            "--project",
            SLUG,
        ]),
        "queued, not yet delivered\n"
    );
    assert!(golden.checklist().contains("queued, not yet delivered"));

    const REDIRECT_SESSION: &str = "d94b02";
    golden.boxr.respond_launches(&[SESSION, REDIRECT_SESSION]);
    golden
        .boxr
        .respond_status_sequence(&["finished", "finished", "finished", "finished", "finished"]);
    golden.boxr.respond(
        "status",
        &format!("session: {REDIRECT_SESSION}\nstate: running\n"),
        "",
        0,
    );
    daemon
        .tick()
        .expect("the daemon delivers the redirect when the turn ends");
    assert!(
        golden.boxr.calls_to("resume").is_empty(),
        "a finished session is never resumed"
    );
    let launches = golden.boxr.calls_to("--harness");
    assert_eq!(launches.len(), 2, "a fresh worker is launched");
    let prompt = launches[1].last().expect("the launch prompt");
    assert!(
        prompt.contains("Skip the migration."),
        "the relaunch brief carries the redirect: {prompt}"
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_redirect_delivered".to_string()),
        "the delivery receipt is journalled"
    );
    let checklist = golden.checklist();
    assert!(checklist.contains("redirect (delivered)"), "{checklist}");
}

#[test]
fn a_finished_session_is_answered_by_a_fresh_worker_instead_of_a_resume() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_asks("Which store?");
    golden.boxr.report_finished();
    assert_eq!(golden.task().state, TaskState::WaitingOnQuestion);
    golden.depot_ok(&[
        "task",
        "answer",
        TASK,
        "--text",
        "sqlite in the depot home.",
        "--by",
        "user",
        "--project",
        SLUG,
    ]);
    const NEXT_SESSION: &str = "e18c4f";
    golden.boxr.respond_launches(&[SESSION, NEXT_SESSION]);
    golden
        .boxr
        .respond_status_sequence(&["running", "finished"]);
    golden.boxr.report_running();

    daemon
        .tick()
        .expect("the daemon answers a finished session with a fresh worker");
    assert!(
        golden.boxr.calls_to("resume").is_empty(),
        "a finished session is never resumed"
    );
    assert!(golden.history(TASK).contains(&LAUNCH_REQUESTED.to_string()));
    assert_eq!(golden.task().state, TaskState::Running);
    let launches = golden.boxr.calls_to("--harness");
    assert_eq!(
        launches.len(),
        2,
        "a fresh worker is launched, {launches:?}"
    );
    let prompt = launches[1].last().expect("the launch prompt");
    assert!(
        prompt.contains("Which store?") && prompt.contains("sqlite in the depot home."),
        "the relaunch brief carries the question and the answer: {prompt}"
    );

    let task = golden.task();
    assert_eq!(task.attempts.len(), 2);
    assert_eq!(
        task.attempts[0].outcome,
        AttemptOutcome::AwaitingAnswer,
        "the dead turn is closed as awaiting the answer it received"
    );
    assert_eq!(task.attempts[1].session, Some(SessionId::new(NEXT_SESSION)));
    assert_eq!(task.attempts[1].worktree, Some(WorktreeLease::new(LEASE)));
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == TURN_STARTED)
            .count(),
        2,
        "the fresh turn starts when the relaunch lands"
    );
}

#[test]
fn a_resume_that_keeps_failing_surfaces_the_task_to_a_person() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_asks("Which store?");
    golden.boxr.report_running();
    golden.depot_ok(&[
        "task",
        "answer",
        TASK,
        "--text",
        "sqlite in the depot home.",
        "--by",
        "user",
        "--project",
        SLUG,
    ]);
    golden.boxr.respond("resume", "", "resume interrupted", 1);

    for _ in 0..3 {
        daemon
            .tick()
            .expect("a failed resume does not stop the daemon");
    }
    let retrying = golden.task();
    assert_eq!(retrying.state, TaskState::Running);
    assert_eq!(golden.boxr.calls_to("resume").len(), 3);
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == TURN_STARTED)
            .count(),
        1,
        "a running session is never taken as proof the resume landed"
    );

    daemon
        .tick()
        .expect("the exhausted ladder surfaces the task to a person");
    let task = golden.task();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(
        golden.boxr.calls_to("resume").len(),
        3,
        "the resume ladder is bounded"
    );
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == TURN_STARTED)
            .count(),
        1,
        "no turn start is fabricated for an answer that was never delivered"
    );

    let history = golden.status_history();
    assert!(history.contains("Failed (1)"), "{history}");
    let default = golden.status();
    assert!(!default.contains("## Failed"), "{default}");
    assert!(default.contains("1 failed"), "{default}");
    assert_eq!(default, golden.checklist());

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        user_section(&inbox).contains("a worker turn could not be resolved"),
        "{inbox}"
    );
}

#[test]
fn a_restart_with_a_launch_intent_surfaces_it_rather_than_launching_again() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden
        .boxr
        .respond("--harness", "", "launch interrupted", 1);
    golden.propose();

    daemon
        .tick()
        .expect("a failing launch is deferred rather than fatal");
    assert!(golden.history(TASK).contains(&LAUNCH_REQUESTED.to_string()));
    assert!(!golden.history(TASK).contains(&TURN_STARTED.to_string()));

    golden
        .boxr
        .respond("--harness", &format!("{SESSION}\n"), "", 0);
    daemon
        .recover()
        .expect("recovery surfaces the unresolved launch instead of launching again");

    let task = golden.task();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.attempts[0].session, None);
    assert_eq!(
        golden.boxr.calls_to("--harness").len(),
        1,
        "no duplicate worker"
    );
    assert!(task.attempts[0].worktree.is_some(), "the worktree is kept");

    let history = golden.status_history();
    assert!(history.contains("Failed (1)"), "{history}");
    let default = golden.status();
    assert!(!default.contains("## Failed"), "{default}");
    assert!(default.contains("1 failed"), "{default}");
    assert_eq!(default, golden.checklist());

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(inbox.contains("## For the user (1)"), "{inbox}");
    assert!(
        inbox.contains("a worker turn could not be resolved"),
        "{inbox}"
    );
}

#[test]
fn a_configuration_error_before_a_launch_holds_the_task_with_its_reason() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    golden.map_build_role(None);

    daemon
        .tick()
        .expect("a configuration problem holds the task instead of stopping the daemon");
    assert!(
        !golden.history(TASK).contains(&LAUNCH_REQUESTED.to_string()),
        "nothing was launched, so the journal holds no launch intent"
    );
    assert!(golden.boxr.calls_to("--harness").is_empty());

    let blocked = golden.task();
    assert_eq!(blocked.state, TaskState::Failed);
    assert!(
        blocked
            .failure
            .as_deref()
            .is_some_and(|reason| reason.contains("profile")),
        "the task names the configuration problem: {:?}",
        blocked.failure
    );
    assert_eq!(blocked.attempts[0].session, None);
    assert_eq!(
        blocked.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE))
    );

    golden.map_build_role(Some(PROFILE));
    golden.depot_ok(&["task", "retry", TASK, "--project", SLUG]);
    daemon
        .tick()
        .expect("the retried task launches once the configuration is right");

    let launched = golden.task();
    assert_eq!(launched.state, TaskState::Running);
    assert_eq!(
        launched
            .attempts
            .last()
            .and_then(|attempt| attempt.session.clone()),
        Some(SessionId::new(SESSION)),
        "the configuration is read again and the worker launches"
    );
    assert_eq!(
        golden.boxr.calls_to("--harness").len(),
        1,
        "the worker launches once"
    );
    assert!(golden.history(TASK).contains(&LAUNCH_REQUESTED.to_string()));
    assert!(golden.history(TASK).contains(&TURN_STARTED.to_string()));

    let running = golden.status();
    assert!(running.contains("Running (1)"), "{running}");
    assert!(!running.contains("Blocked"), "{running}");
    assert!(
        running.contains("attempt: in_flight"),
        "a running line carries the attempt observation, got\n{running}"
    );
    golden.status_matches_checklist(&running);

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("no profile is configured for the role build"),
        "the hold names the configuration problem, got\n{inbox}"
    );
}

#[test]
fn a_restart_with_a_task_in_flight_marks_it_unknown_and_launches_no_replacement() {
    let mut golden = Golden::new(Validation::Passing);

    {
        assert!(
            InstanceLock::acquire(&golden.home).is_err(),
            "a second daemon cannot run against the same home"
        );
        let daemon = golden.daemon();
        golden.propose();
        daemon.tick().expect("the daemon launches the worker");
    }

    let in_flight = golden.task();
    assert_eq!(in_flight.state, TaskState::Running);
    assert_eq!(in_flight.attempts.len(), 1);
    assert_eq!(in_flight.attempts[0].outcome, AttemptOutcome::InFlight);
    assert!(golden.history(TASK).contains(&LAUNCH_REQUESTED.to_string()));

    golden.restart_lock();
    let store = depotd::Store::open(&golden.home).expect("the restarted daemon reopens the store");
    let daemon = depotd::Daemon::new(
        &store,
        golden.project.clone(),
        depotd::adapters::sessions::Boxr::new(golden.boxr.program()),
        depotd::adapters::worktrees::Treehouse::new(golden.treehouse.program()),
        depotd::ShellValidation,
        depotd::ForgeDelivery::new(depotd::adapters::forge::GitHub::new(
            golden.forge.base_url(),
            support::TOKEN,
        )),
        depotd::NoEventHook,
    );

    golden.boxr.respond("status", "", "status interrupted", 1);
    daemon
        .recover()
        .expect("recovery records the unreadable session without dying");
    let marked = golden.task();
    assert_eq!(marked.state, TaskState::Running);
    assert_eq!(
        marked.attempts[0].outcome,
        AttemptOutcome::Unknown,
        "recovery marks the in-flight attempt unknown instead of guessing"
    );

    golden.boxr.report_running();
    daemon
        .recover()
        .expect("recovery reconciles from the records");

    let recovered = golden.task();
    assert_eq!(recovered.state, TaskState::Running);
    assert_eq!(recovered.attempts.len(), 1, "no replacement attempt");
    assert_eq!(recovered.attempts[0].outcome, AttemptOutcome::InFlight);
    assert_eq!(recovered.attempts[0].session, Some(SessionId::new(SESSION)));
    assert_eq!(
        recovered.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE))
    );

    assert_eq!(
        golden.boxr.calls_to("--harness").len(),
        1,
        "no duplicate worker"
    );
    assert_eq!(golden.boxr.calls_to("--version").len(), 1);
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "get").len(),
        1,
        "no duplicate worktree"
    );
    assert!(calls_to(&golden.treehouse.calls(), "return").is_empty());

    assert!(!golden.history(TASK).contains(&RESTARTED.to_string()));
    assert!(
        golden.events().iter().any(|event| event.kind == RESTARTED),
        "the restart is recorded on the journal"
    );
    assert!(golden.history(TASK).contains(&LIVENESS.to_string()));

    let running = golden.status();
    assert!(running.contains("Running (1)"), "{running}");
    assert!(!running.contains("Blocked"), "{running}");
    assert!(
        running.contains("attempt: in_flight"),
        "a running line carries the attempt observation, got\n{running}"
    );
    golden.status_matches_checklist(&running);
}

#[test]
fn an_idle_depot_issues_no_forge_calls() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();

    daemon.tick().expect("an empty project ticks");
    assert!(
        golden.forge.requests().is_empty(),
        "an idle depot polls nothing: {:?}",
        golden.forge.requests()
    );

    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    assert_eq!(golden.task().state, TaskState::Running);
    assert!(
        golden.forge.requests().is_empty(),
        "a task without a pull request is never polled: {:?}",
        golden.forge.requests()
    );
}

#[test]
fn a_poll_that_observes_no_change_produces_no_fact_and_takes_no_action() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);
    assert_eq!(
        golden.task().pull_request().map(|(_, _, checks)| checks),
        Some(Checks::Passing)
    );

    let facts = forge_facts(&golden);
    let events = golden.events().len();
    let opened = golden.pull_requests_opened();
    let acquired = calls_to(&golden.treehouse.calls(), "get").len();
    let returns = calls_to(&golden.treehouse.calls(), "return").len();
    let forge_calls = golden.forge.requests().len();

    daemon
        .tick()
        .expect("a second poll observes the same pull request");

    let unchanged = golden.task();
    assert_eq!(unchanged.state, TaskState::PrOpen);
    assert_eq!(
        unchanged.pull_request().map(|(_, _, checks)| checks),
        Some(Checks::Passing)
    );
    assert_eq!(
        golden.events().len(),
        events + 1,
        "only the tick's own polled marker is recorded"
    );
    assert_eq!(
        forge_facts(&golden),
        facts,
        "a poll that observes no change records no forge fact"
    );
    assert!(
        golden.forge.requests().len() > forge_calls,
        "the daemon still polls the open pull request"
    );
    assert_eq!(
        golden.pull_requests_opened(),
        opened,
        "no second pull request"
    );
    assert_eq!(
        golden.merge_requests(),
        0,
        "a project that did not opt in is never merged automatically"
    );
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "get").len(),
        acquired,
        "an unchanged poll acquires no worktree"
    );
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        returns,
        "an unchanged poll releases no worktree"
    );
}

#[test]
fn a_project_that_did_not_opt_in_never_merges_itself() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    for _ in 0..3 {
        daemon
            .tick()
            .expect("the daemon polls the open pull request");
    }

    assert_eq!(golden.task().state, TaskState::PrOpen);
    assert_eq!(
        golden.merge_requests(),
        0,
        "automatic merging stays off until the project enables it"
    );
    assert!(calls_to(&golden.treehouse.calls(), "return").is_empty());
}

#[test]
fn a_project_that_opts_in_merges_the_validated_pull_request_itself() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.set_auto_merge(true);
    golden.script_merge_endpoint();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, opens and then merges the pull request");

    let landed = golden.task();
    assert_eq!(landed.state, TaskState::Landed);
    assert_eq!(
        golden.merge_requests(),
        1,
        "the opted-in project merges the validated revision once"
    );
    assert!(golden.history(TASK).contains(&MERGED.to_string()));
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "landing the task returns its worktree once"
    );
    assert!(calls_to(&golden.treehouse.calls(), "get").len() == 1);

    let history = golden.status_history();
    assert!(history.contains("Landed (1)"), "{history}");
    let default = golden.status();
    assert!(!default.contains("## Landed"), "{default}");
    assert!(default.contains("1 landed"), "{default}");
    assert_eq!(default, golden.checklist());
}

#[test]
fn a_pull_request_closed_unmerged_holds_the_task_and_returns_the_worktree() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    golden.script_close_unmerged(&commit);
    daemon
        .tick()
        .expect("the daemon observes the pull request closed unmerged");

    let held = golden.task();
    assert_eq!(held.state, TaskState::Cancelled);
    assert_ne!(
        held.state,
        TaskState::Landed,
        "a closed pull request never lands the task"
    );
    assert!(
        golden.history(TASK).contains(&CLOSED_UNMERGED.to_string()),
        "the close is recorded"
    );
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "a closed pull request returns the worktree the branch no longer needs"
    );

    let history = golden.status_history();
    assert!(history.contains("Cancelled (1)"), "{history}");
    let default = golden.status();
    assert!(!default.contains("## Cancelled"), "{default}");
    assert!(default.contains("1 cancelled"), "{default}");
    assert_eq!(default, golden.checklist());
}

#[test]
fn stopping_a_running_task_returns_its_worktree_once() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    assert_eq!(golden.task().state, TaskState::Running);

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    let stopped = golden.task();
    assert_eq!(stopped.state, TaskState::Cancelled);
    assert_eq!(stopped.release_pending, vec![WorktreeLease::new(LEASE)]);
    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "the CLI writes the fact and executes no action"
    );

    daemon
        .tick()
        .expect("the daemon reconciles the owed release");

    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "stopping the task returns its worktree once"
    );
    assert!(golden.task().release_pending.is_empty());

    daemon.tick().expect("a second tick returns nothing more");
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "the lease is never returned twice"
    );
}

#[test]
fn acknowledging_a_failed_task_returns_its_worktree_once() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.boxr.respond(
        "status",
        &format!("session: {SESSION}\nstate: failed\nerror: \"the harness crashed\"\n"),
        "",
        0,
    );
    daemon.tick().expect("the failure is recorded");

    let failed = golden.task();
    assert_eq!(failed.state, TaskState::Failed);
    assert!(failed.release_pending.is_empty());
    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "a failed task keeps its lease until it is acknowledged or retried"
    );

    assert_eq!(
        golden.depot_ok(&["task", "acknowledge", TASK, "--project", SLUG]),
        format!("acknowledged {TASK}\n")
    );
    daemon
        .tick()
        .expect("the daemon reconciles the owed release");

    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "acknowledging the failure returns its worktree once"
    );
    assert!(golden.task().release_pending.is_empty());

    daemon.tick().expect("a second tick returns nothing more");
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "the lease is never returned twice"
    );
}

#[test]
fn the_startup_sweep_returns_a_lease_a_terminal_task_left_behind() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    assert_eq!(golden.task().state, TaskState::Cancelled);
    assert_eq!(
        golden.task().release_pending,
        vec![WorktreeLease::new(LEASE)]
    );
    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "the stop is written while no daemon acts on it"
    );

    golden
        .daemon()
        .recover()
        .expect("the startup sweep returns the lease");

    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "the sweep returns the lease the terminal task left behind"
    );
    assert!(golden.task().release_pending.is_empty());
}

#[test]
fn the_startup_sweep_returns_a_lease_the_records_do_not_name() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    let mut pre_upgrade = golden.task();
    pre_upgrade.release_pending.clear();
    pre_upgrade.release_held.clear();
    for attempt in &mut pre_upgrade.attempts {
        attempt.worktree = None;
    }
    golden
        .store
        .put_task(&pre_upgrade)
        .expect("the pre-upgrade record is written");
    assert!(calls_to(&golden.treehouse.calls(), "return").is_empty());

    golden
        .daemon()
        .recover()
        .expect("the startup sweep reads the pool");

    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "the pool names the lease the records forgot"
    );
    assert!(golden.task().release_pending.is_empty());
}

#[test]
fn a_pending_acquire_that_takes_the_owed_lease_back_does_not_return_it() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    golden.depot_ok(&["task", "retry", TASK, "--project", SLUG]);
    golden.reject_worktree_acquire();
    daemon
        .tick()
        .expect("a saturated pool defers the acquire instead of aborting the tick");

    golden.allow_worktree_acquire();
    daemon
        .tick()
        .expect("the pending acquire is completed from the pool");

    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "the lease a live attempt works in is never returned"
    );
    assert!(golden.task().release_pending.is_empty());
    assert_eq!(golden.task().state, TaskState::Running);
}

#[test]
fn a_running_task_keeps_the_lease_it_still_owes() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    let mut running = golden.task();
    running.release_pending.push(WorktreeLease::new(LEASE));
    golden
        .store
        .put_task(&running)
        .expect("the owed release is recorded");

    daemon.tick().expect("a running task keeps its lease");

    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "a lease a live attempt works in is never returned"
    );
    assert_eq!(
        golden.task().release_pending,
        vec![WorktreeLease::new(LEASE)]
    );
}

#[test]
fn a_retried_task_returns_the_lease_a_closed_earlier_attempt_still_names() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.boxr.respond(
        "status",
        &format!("session: {SESSION}\nstate: failed\nerror: \"the harness crashed\"\n"),
        "",
        0,
    );
    daemon.tick().expect("the failure is recorded");

    const RELAUNCHED_SESSION: &str = "b3c7e2";
    golden.boxr.respond_launches(&[SESSION, RELAUNCHED_SESSION]);
    golden.depot_ok(&["task", "retry", TASK, "--project", SLUG]);
    daemon
        .tick()
        .expect("the retry takes the lease the first attempt left behind");

    let retried = golden.task();
    assert_eq!(retried.attempts.len(), 2);
    assert_eq!(
        retried.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE)),
        "the failed attempt keeps its lease for the retry"
    );
    assert_eq!(
        retried.attempts[1].worktree,
        Some(WorktreeLease::new(LEASE)),
        "the retry works in the same lease"
    );

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    daemon.tick().expect("the stop reconciles the owed release");

    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "a closed attempt naming the lease never protects it from the release pass"
    );
    assert!(golden.task().release_pending.is_empty());
}

#[test]
fn a_lease_holding_unlanded_work_is_held_and_named_in_the_checklist() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    std::fs::write(golden.lease.join("unfinished.txt"), "half a change\n")
        .expect("the lease holds uncommitted work");

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    daemon
        .tick()
        .expect("the held release does not crash the tick");

    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "a lease holding work is never returned"
    );
    let held = golden.task();
    assert_eq!(held.state, TaskState::Cancelled);
    assert_eq!(held.release_pending, vec![WorktreeLease::new(LEASE)]);
    assert!(
        held.release_held
            .get(&WorktreeLease::new(LEASE))
            .is_some_and(|hold| hold.reason.contains("uncommitted")),
        "the hold names the reason: {:?}",
        held.release_held
    );

    let history = golden.status_history();
    assert!(
        history.contains("worktree lease `") && history.contains("held, not returned"),
        "{history}"
    );
    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(inbox.contains("is held, not returned"), "{inbox}");

    daemon.tick().expect("a second tick repeats no held fact");
    assert_eq!(
        golden
            .history(TASK)
            .into_iter()
            .filter(|kind| kind == RELEASE_HELD)
            .count(),
        1,
        "an unchanged hold is recorded once"
    );
}

#[test]
fn a_repeated_hold_for_the_same_lease_applies_no_second_change() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    std::fs::write(golden.lease.join("unfinished.txt"), "half a change\n")
        .expect("the lease holds uncommitted work");

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    daemon
        .tick()
        .expect("the held release does not crash the tick");

    let lease = WorktreeLease::new(LEASE);
    let mut held = golden.task();
    held.release_held
        .get_mut(&lease)
        .expect("the first hold is recorded")
        .at = Timestamp::from_millis(0);
    golden
        .store
        .put_task(&held)
        .expect("the aged hold is written");

    daemon
        .tick()
        .expect("the expired backoff retries the release");

    assert_eq!(
        golden
            .history(TASK)
            .into_iter()
            .filter(|kind| kind == RELEASE_HELD)
            .count(),
        1,
        "an unchanged hold is never recorded twice"
    );
    assert_eq!(
        golden.task().release_held.get(&lease).map(|hold| hold.at),
        Some(Timestamp::from_millis(0)),
        "the repeated hold applies no second change"
    );
}

#[test]
fn a_merge_of_a_revision_depot_never_validated_is_not_landed() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    let other = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f";
    golden.script_merge(other);
    daemon
        .tick()
        .expect("the daemon observes the merge of another revision");

    let held = golden.task();
    assert_ne!(
        held.state,
        TaskState::Landed,
        "a merge of a revision depot never validated is never recorded as landed"
    );
    assert_eq!(held.state, TaskState::Failed);
    assert_eq!(
        held.validated_commit(),
        Some(&CommitId::new(commit.clone()))
    );
    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "the unvalidated merge keeps the worktree for review"
    );

    let history = golden.status_history();
    assert!(history.contains("Failed (1)"), "{history}");
    let default = golden.status();
    assert!(!default.contains("## Landed"), "{default}");
    assert!(default.contains("1 failed"), "{default}");
    assert_eq!(default, golden.checklist());
}

#[test]
fn an_open_pull_request_depot_did_not_open_is_adopted_rather_than_duplicated() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_existing_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and adopts the existing pull request");

    let opened = golden.task();
    assert_eq!(opened.state, TaskState::PrOpen);
    assert_eq!(opened.pull_request().map(|(number, _, _)| number), Some(1));
    assert!(
        golden.history(TASK).contains(&OPENED.to_string()),
        "the poll records the pull request it found"
    );
    assert_eq!(
        golden.pull_requests_opened(),
        0,
        "an already open pull request is never opened again"
    );
    let refreshed = golden
        .forge
        .requests()
        .into_iter()
        .find(|request| request.method == "PATCH");
    assert!(
        refreshed.is_none(),
        "a body depot does not own is never overwritten: {:?}",
        refreshed.map(|request| request.body)
    );
    let checklist = golden.status();
    assert!(checklist.contains("Pull request open (1)"), "{checklist}");
    assert_eq!(checklist, golden.checklist());
}

#[test]
fn a_pull_request_depot_already_owns_is_refreshed_with_the_new_title_and_body() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_existing_pull_request_with_body(
        &commit,
        Some(&format!("{MANAGED_MARKER}\n\n## Why\n\nstale\n")),
    );
    daemon
        .tick()
        .expect("the daemon validates, pushes and refreshes the pull request it owns");

    assert_eq!(golden.task().state, TaskState::PrOpen);
    let refreshed = golden
        .forge
        .requests()
        .into_iter()
        .find(|request| request.method == "PATCH")
        .expect("a depot-managed pull request is refreshed");
    assert!(
        refreshed.body.contains(MANAGED_MARKER),
        "the refreshed body stays depot-managed: {}",
        refreshed.body
    );
    assert!(
        refreshed.body.contains("## Validation"),
        "the reused pull request carries the rendered body: {}",
        refreshed.body
    );
    assert!(
        !refreshed.body.contains("stale"),
        "the stale body is replaced: {}",
        refreshed.body
    );
}

#[test]
fn a_pull_request_branch_that_lags_the_validated_commit_is_pushed_forward() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);
    assert_eq!(
        git::git(
            &golden.origin,
            &["rev-parse", &format!("refs/heads/{BRANCH}")]
        )
        .trim(),
        commit
    );

    let reworked = golden.commit_in_lease("rework.txt", "the rework\n");
    let mut task = golden.task();
    task.validations.push(ValidationRecord {
        command: "cargo test".to_owned(),
        commit: CommitId::new(reworked.clone()),
        base_commit: None,
        exit_code: 0,
        duration: std::time::Duration::from_secs(5),
        output_tail: "ok".to_owned(),
    });
    golden
        .store
        .put_task(&task)
        .expect("the revalidation the lost push left behind is recorded");

    daemon
        .tick()
        .expect("the daemon derives the push the records still owe");

    assert_eq!(
        git::git(
            &golden.origin,
            &["rev-parse", &format!("refs/heads/{BRANCH}")]
        )
        .trim(),
        reworked,
        "the branch catches up to the commit depot validated"
    );
    let caught_up = golden.task();
    assert_eq!(caught_up.state, TaskState::PrOpen);
    assert_eq!(caught_up.branch_head, Some(CommitId::new(reworked.clone())));
    assert_eq!(
        caught_up.validated_commit(),
        Some(&CommitId::new(reworked.clone()))
    );
    assert_eq!(
        golden.pull_requests_opened(),
        1,
        "catching the branch up opens no second pull request"
    );
}

#[test]
fn a_fix_role_push_never_overwrites_a_force_pushed_remote_branch() {
    let golden = Golden::new(Validation::Passing);
    golden.map_role("fix", Some(PROFILE));
    let daemon = golden.daemon();

    let added = golden.depot_ok(&[
        "task",
        "add",
        "--title",
        "Fix the store",
        "--intent",
        "Repair the records in sqlite.",
        "--role",
        "fix",
        "--project",
        SLUG,
    ]);
    assert_eq!(added, format!("added {TASK}\n"));
    golden.depot_ok(&["task", "approve", TASK, "--project", SLUG]);
    golden.forge.route_query(
        "GET",
        &format!("/repos/{REPOSITORY}/pulls"),
        Some("state=open&head=nunoras%3Afix/fix-the-store"),
        200,
        "[]",
    );
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    let fix_branch = "fix/fix-the-store";
    let rival = golden.force_push_divergent(fix_branch);
    assert_ne!(rival, commit, "the remote branch was replaced");

    let reworked = golden.commit_in_lease("rework.txt", "the rework\n");
    let mut task = golden.task();
    task.validations.push(ValidationRecord {
        command: "cargo test".to_owned(),
        commit: CommitId::new(reworked.clone()),
        base_commit: None,
        exit_code: 0,
        duration: std::time::Duration::from_secs(5),
        output_tail: "ok".to_owned(),
    });
    golden
        .store
        .put_task(&task)
        .expect("the revalidation is recorded");

    daemon
        .tick()
        .expect("a rejected fix-role push is recorded, not fatal");

    let failed = golden.task();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.branch_head, Some(CommitId::new(commit.clone())));
    assert!(
        golden.history(TASK).contains(&PUSH_FAILED.to_string()),
        "the rejection is on the journal"
    );
    assert_ne!(
        git::git(
            &golden.origin,
            &["rev-parse", &format!("refs/heads/{fix_branch}")]
        )
        .trim(),
        reworked,
        "a fix-role push never replaces the branch at the forge"
    );
}

#[test]
fn a_push_rejection_fails_the_task_and_the_daemon_keeps_running() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    let rival = golden.force_push_divergent_branch();
    let reworked = golden.commit_in_lease("rework.txt", "the rework\n");
    let mut task = golden.task();
    task.validations.push(ValidationRecord {
        command: "cargo test".to_owned(),
        commit: CommitId::new(reworked.clone()),
        base_commit: None,
        exit_code: 0,
        duration: std::time::Duration::from_secs(5),
        output_tail: "ok".to_owned(),
    });
    golden
        .store
        .put_task(&task)
        .expect("the revalidation is recorded");

    daemon
        .tick()
        .expect("a rejected push is recorded, not fatal");

    let failed = golden.task();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.branch_head, Some(CommitId::new(commit.clone())));
    assert!(
        golden.history(TASK).contains(&PUSH_FAILED.to_string()),
        "the rejection is on the journal"
    );
    assert_ne!(
        git::git(
            &golden.origin,
            &["rev-parse", &format!("refs/heads/{BRANCH}")]
        )
        .trim(),
        reworked,
        "the rejected push changed nothing at the forge"
    );

    let history = golden.status_history();
    assert!(history.contains("Failed (1)"), "{history}");
    let default = golden.status();
    assert!(!default.contains("## Failed"), "{default}");
    assert!(default.contains("1 failed"), "{default}");
    assert_eq!(default, golden.checklist());

    daemon
        .tick()
        .expect("the daemon keeps scheduling after a push rejection");
    assert_eq!(golden.task().state, TaskState::Failed);
    assert!(!rival.is_empty());
}

#[test]
fn an_opted_in_project_does_not_merge_while_a_dependency_pin_is_stale() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    let mut prerequisite = golden.task();
    prerequisite.id = TaskId::new("t-9");
    prerequisite.state = TaskState::Landed;
    prerequisite.dependencies = Vec::new();
    prerequisite.branch_head = Some(CommitId::new("9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f"));
    prerequisite.validations = vec![ValidationRecord {
        command: "cargo test".to_owned(),
        commit: CommitId::new("9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f"),
        base_commit: None,
        exit_code: 0,
        duration: std::time::Duration::from_secs(5),
        output_tail: "ok".to_owned(),
    }];
    golden
        .store
        .put_task(&prerequisite)
        .expect("the dependency the task was pinned to is recorded");

    let mut dependent = golden.task();
    dependent.dependencies = vec![Dependency {
        task: TaskId::new("t-9"),
        commit: CommitId::new(commit.clone()),
    }];
    golden
        .store
        .put_task(&dependent)
        .expect("the stale pin is recorded on the task");

    golden.set_auto_merge(true);
    golden.script_merge_endpoint();
    daemon
        .tick()
        .expect("the daemon polls the open pull request with a stale pin");

    assert_eq!(
        golden.merge_requests(),
        0,
        "a stale dependency pin keeps the merge away even when the project opted in"
    );
    assert_eq!(golden.task().state, TaskState::PrOpen);
    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "a held task keeps its worktree"
    );
}

#[test]
fn a_refused_auto_merge_is_retried_only_when_the_observation_changes() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.set_auto_merge(true);
    golden.script_merge_endpoint_refused();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, opens the pull request and is refused the merge");

    assert_eq!(golden.task().state, TaskState::PrOpen);
    assert_eq!(
        golden.merge_requests(),
        1,
        "the opted-in project asks the forge to merge once"
    );
    assert_eq!(
        refusals(&golden),
        1,
        "the refused merge is recorded exactly once"
    );
    let held = golden.status();
    assert!(
        held.contains("auto-merge refused: ") && held.contains("405"),
        "the checklist names the refusal and its reason: {held}"
    );
    assert_eq!(held, golden.checklist());

    let facts = forge_facts(&golden);
    let events = golden.events().len();
    let returns = calls_to(&golden.treehouse.calls(), "return").len();
    daemon.tick().expect("an unchanged poll repeats nothing");

    assert_eq!(
        golden.merge_requests(),
        1,
        "an unchanged observation is never merged again"
    );
    assert_eq!(
        golden.events().len(),
        events + 1,
        "only the tick's own polled marker is recorded"
    );
    assert_eq!(
        forge_facts(&golden),
        facts,
        "an unchanged poll records no forge fact"
    );
    assert_eq!(golden.task().state, TaskState::PrOpen);
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        returns,
        "a refused merge keeps the worktree"
    );

    golden.script_pull_request_base(&commit, "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f");
    daemon.tick().expect("a moved base earns the next attempt");

    assert_eq!(
        golden.merge_requests(),
        2,
        "a changed observation earns exactly one more attempt"
    );
    assert_eq!(refusals(&golden), 2, "the second refusal is recorded");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    golden.script_pull_request_base(&commit, BASE);
    daemon
        .tick()
        .expect("a base that returns to an earlier shape is a change too");

    assert_eq!(
        golden.merge_requests(),
        3,
        "a value returning to an earlier refused shape earns the next attempt"
    );
    assert_eq!(refusals(&golden), 3, "the third refusal is recorded");

    let facts = forge_facts(&golden);
    let events = golden.events().len();
    daemon.tick().expect("an unchanged poll repeats nothing");

    assert_eq!(
        golden.merge_requests(),
        3,
        "an observation identical to the last attempt is never merged again"
    );
    assert_eq!(
        golden.events().len(),
        events + 1,
        "only the tick's own polled marker is recorded"
    );
    assert_eq!(forge_facts(&golden), facts);

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("the forge refused to merge pull request #1"),
        "the coordinator sees the refusal: {inbox}"
    );
    assert!(inbox.contains("405"), "the reason is in the inbox: {inbox}");

    golden.script_close_unmerged(&commit);
    daemon
        .tick()
        .expect("the daemon observes the pull request closed unmerged");

    let closed = golden.status();
    assert_eq!(golden.task().state, TaskState::Cancelled);
    assert!(
        !closed.contains("auto-merge refused"),
        "a closed pull request forgets the refusal: {closed}"
    );
    assert_eq!(closed, golden.checklist());
}

fn refusals(golden: &Golden) -> usize {
    golden
        .history(TASK)
        .iter()
        .filter(|kind| kind.as_str() == MERGE_REFUSED)
        .count()
}

fn forge_facts(golden: &Golden) -> Vec<String> {
    golden
        .events()
        .into_iter()
        .filter(|event| event.kind.starts_with("pull_request_"))
        .map(|event| event.kind)
        .collect()
}

fn user_section(rendered: &str) -> &str {
    let start = rendered
        .find("## For the user (")
        .expect("the inbox names the user section");
    let rest = &rendered[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn the_on_event_hook_fires_once_per_blocking_event() {
    let golden = Golden::new(Validation::Passing);
    let log = golden.temp.path().join("hook.log");
    let command = append_stdin_as_a_line_to(&log);
    golden
        .home
        .write_settings(&support::settings_with_on_event(Some(
            depotd::OnEventSettings {
                command,
                events: None,
            },
        )))
        .expect("the settings are written");
    let daemon = golden.daemon();

    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    assert!(
        !log.exists(),
        "no event fires while no task blocks on the user"
    );

    golden.boxr.report_finished();
    let asked = golden.worker_commits_and_asks("Which store?");
    assert_eq!(asked.status.code(), Some(0));
    assert_eq!(golden.task().state, TaskState::WaitingOnQuestion);

    daemon.tick().expect("the daemon fires the event hook");
    let events = hook_events(&log);
    assert_eq!(events.len(), 1, "one blocking event fires once: {events:?}");
    let event = &events[0];
    assert_eq!(event["project"], "example");
    assert_eq!(event["task"], TASK);
    assert_eq!(event["title"], "Wire the store");
    assert_eq!(event["event"], "question");
    assert_eq!(event["question"], "Which store?");
    assert!(event["recommended_default"].is_null());
    assert!(event["pull_request"].is_null());

    daemon
        .tick()
        .expect("a further poll does not fire the hook again");
    assert_eq!(hook_events(&log).len(), 1, "the same block fires once");
}

fn append_stdin_as_a_line_to(log: &std::path::Path) -> String {
    let log = log.display();
    if cfg!(windows) {
        format!("findstr \"^\" >> \"{log}\" & echo.>> \"{log}\"")
    } else {
        format!("cat >> {log}; echo >> {log}")
    }
}

fn hook_events(log: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log)
        .expect("the hook log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each hook line is a json payload"))
        .collect()
}

#[test]
fn a_conflicting_pull_request_is_resolved_by_a_fix_worker_and_lands() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.script_merge_endpoint();
    golden.script_delete_branch_endpoint();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    let base = golden.advance_base_conflicting("change.txt", "the base moved\n");
    golden.script_conflicting_pull_request_at(&commit, &base);
    golden.set_auto_merge(true);
    daemon
        .tick()
        .expect("the daemon observes the conflict and schedules the fix turn");

    let rebasing = golden.task();
    assert_eq!(rebasing.state, TaskState::Running);
    assert_eq!(rebasing.conflict_base, Some(CommitId::new(base)));
    assert!(golden.history(TASK).contains(&REBASE_SCHEDULED.to_string()));
    assert_eq!(
        golden.task().attempts.len(),
        2,
        "the fix turn is a second attempt on the same task"
    );
    assert!(
        golden
            .task()
            .attempts
            .last()
            .is_some_and(|attempt| attempt.base_merge),
        "the second attempt is marked as a fix turn"
    );
    assert_eq!(
        golden.merge_requests(),
        0,
        "a conflicting pull request is not merged"
    );

    let output = golden.worker_merges_and_submits();
    assert_eq!(
        output.status.code(),
        Some(0),
        "the fix worker merges the base and submits: {}",
        support::stderr(&output)
    );
    let merged = golden.head();
    golden.script_merged_pull_request(&merged);
    daemon
        .tick()
        .expect("the daemon validates the merged commit, merges and deletes the branch");

    let landed = golden.task();
    assert_eq!(landed.state, TaskState::Landed);
    assert!(
        golden.history(TASK).contains(&MERGED.to_string()),
        "the merged pull request lands"
    );
    assert_eq!(golden.merge_requests(), 1);
    assert_eq!(
        golden.branch_deletes(),
        1,
        "the daemon deletes the delivery branch after the merge"
    );
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "return").len(),
        1,
        "landing the fix turn returns the worktree"
    );

    let history = golden.status_history();
    assert!(history.contains("Landed (1)"), "{history}");
    let default = golden.status();
    assert!(!default.contains("## Landed"), "{default}");
    assert!(default.contains("1 landed"), "{default}");
    assert_eq!(default, golden.checklist());
}

#[test]
fn a_stale_conflict_against_an_older_base_never_launches_a_fix_turn() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    let mut task = golden.task();
    task.conflict_base = Some(CommitId::new(BASE));
    golden
        .store
        .put_task(&task)
        .expect("the stale conflict is recorded");
    golden.script_conflicting_pull_request(&commit);

    daemon
        .tick()
        .expect("the daemon re-evaluates the conflict against the fresh base");

    let settled = golden.task();
    assert_eq!(settled.state, TaskState::PrOpen);
    assert!(
        settled.conflict_base.is_none(),
        "a branch that already contains the base clears the stale conflict"
    );
    assert_eq!(
        settled.attempts.len(),
        1,
        "a branch that merges clean never gets a fix turn"
    );
    assert!(!golden.history(TASK).contains(&REBASE_SCHEDULED.to_string()));
}

#[test]
fn an_unverifiable_conflict_never_launches_a_fix_turn() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and opens the pull request");
    assert_eq!(golden.task().state, TaskState::PrOpen);

    golden.free_lease();
    golden.script_conflicting_pull_request(&commit);

    daemon
        .tick()
        .expect("the daemon cannot check the conflict and schedules nothing");

    let settled = golden.task();
    assert_eq!(settled.state, TaskState::PrOpen);
    assert_eq!(
        settled.attempts.len(),
        1,
        "an unverifiable conflict never gets a fix turn"
    );
    assert!(!golden.history(TASK).contains(&REBASE_SCHEDULED.to_string()));
}

#[test]
fn a_retry_of_a_failed_task_runs_a_fresh_attempt_on_the_lease_it_still_holds() {
    let golden = Golden::new(Validation::Failing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.worker_commits_and_submits();
    let failed_commit = golden.head();
    daemon.tick().expect("the daemon validates the commit");

    let failed = golden.task();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.attempts.len(), 1);
    assert_eq!(
        failed.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE)),
        "the failed task keeps its worktree"
    );
    assert_eq!(calls_to(&golden.treehouse.calls(), "get").len(), 1);

    support::write_project(&golden.repo, Validation::Passing);
    golden.pass_validation_in_worktree();
    const RELAUNCHED_SESSION: &str = "b3c7e2";
    golden.boxr.respond_launches(&[SESSION, RELAUNCHED_SESSION]);
    assert_eq!(
        golden.depot_ok(&["task", "retry", TASK, "--project", SLUG]),
        format!("retried {TASK}\n")
    );

    let retried = golden.task();
    assert_eq!(
        retried.state,
        TaskState::Running,
        "the retry puts the task straight back into the queue"
    );
    assert_eq!(retried.attempts.len(), 2);

    daemon
        .tick()
        .expect("the daemon reuses the lease the task still holds");
    daemon
        .tick()
        .expect("the daemon launches a fresh worker on it");

    let relaunched = golden.task();
    assert_eq!(relaunched.attempts.len(), 2);
    assert_eq!(
        relaunched.attempts[1].session,
        Some(SessionId::new(RELAUNCHED_SESSION))
    );
    assert_eq!(
        relaunched.attempts[1].worktree,
        Some(WorktreeLease::new(LEASE)),
        "the retry reuses the lease the task still holds"
    );
    assert_eq!(
        calls_to(&golden.treehouse.calls(), "get").len(),
        1,
        "a lease that still exists is never acquired twice"
    );
    assert!(calls_to(&golden.treehouse.calls(), "return").is_empty());
    assert_eq!(golden.boxr.calls_to("--harness").len(), 2);

    let submitted = golden.worker_commits_and_submits_with("fix the store", "the fixed work");
    assert_eq!(
        submitted.status.code(),
        Some(0),
        "the retried worker failed: {} {}",
        support::stdout(&submitted),
        support::stderr(&submitted)
    );
    let fixed_commit = golden.head();
    assert_ne!(fixed_commit, failed_commit);
    golden.script_pull_request(&fixed_commit);
    daemon
        .tick()
        .expect("the daemon validates the retried attempt and opens the pull request");

    let opened = golden.task();
    assert_eq!(opened.state, TaskState::PrOpen);
    assert_eq!(
        opened.branch_head,
        Some(CommitId::new(fixed_commit.clone()))
    );
    assert_eq!(
        golden.state_history(TASK),
        vec![
            PROPOSED,
            APPROVED,
            ACQUIRED,
            TURN_STARTED,
            SUBMITTED,
            VALIDATED,
            "task_retried",
            ACQUIRED,
            TURN_STARTED,
            SUBMITTED,
            VALIDATED,
            PUSHED,
            OPENED,
            CHECKS,
        ]
    );
}

#[test]
fn a_describe_failure_does_not_open_the_pull_request_and_holds_the_task() {
    let golden = Golden::new(Validation::Passing);
    let config = std::fs::read_to_string(golden.repo.join(".depot.toml")).expect("the config");
    std::fs::write(
        golden.repo.join(".depot.toml"),
        format!("{config}describe_profile = \"missing-profile\"\n"),
    )
    .expect("the describe profile is set");
    let daemon = golden.daemon();

    golden.depot_ok(&[
        "task",
        "add",
        "--title",
        "Wire the store",
        "--intent",
        "Persist the records in sqlite.",
        "--role",
        "build",
        "--project",
        SLUG,
    ]);
    golden.depot_ok(&["task", "approve", TASK, "--project", SLUG]);
    daemon.tick().expect("the daemon launches the worker");

    let submitted = golden.worker_commits_and_submits();
    assert_eq!(submitted.status.code(), Some(0));
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates, pushes and holds the task when describe fails");

    let held = golden.task();
    assert_eq!(held.state, TaskState::Failed);
    assert!(held.pull_request().is_none());
    assert_eq!(golden.pull_requests_opened(), 0);
    assert!(
        golden
            .history(TASK)
            .iter()
            .any(|kind| kind == "describe_failed"),
        "the failure is journalled: {:?}",
        golden.history(TASK)
    );

    let failed = golden.status();
    assert!(failed.contains("1 failed"), "{failed}");
    assert_eq!(failed, golden.checklist());

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(inbox.contains("the describe step failed"), "{inbox}");
}

#[test]
fn a_project_without_describe_profile_opens_the_validation_only_pull_request() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();

    golden.depot_ok(&[
        "task",
        "add",
        "--title",
        "Wire the store",
        "--intent",
        "Persist the records in sqlite.",
        "--role",
        "build",
        "--project",
        SLUG,
    ]);
    golden.depot_ok(&["task", "approve", TASK, "--project", SLUG]);
    daemon.tick().expect("the daemon launches the worker");

    let submitted = golden.worker_commits_and_submits();
    assert_eq!(submitted.status.code(), Some(0));
    let commit = golden.head();
    golden.script_pull_request(&commit);
    daemon
        .tick()
        .expect("the daemon validates and opens the pull request");

    let opened = golden.task();
    assert_eq!(opened.state, TaskState::PrOpen);
    assert_eq!(golden.pull_requests_opened(), 1);
    let request = golden
        .forge
        .requests()
        .into_iter()
        .find(|request| request.method == "POST")
        .expect("the pull request opens");
    assert!(request.body.contains("## Validation"), "{:#}", request.body);
    assert!(
        !request.body.contains("## Why"),
        "the opt-out body stays validation-only: {:#}",
        request.body
    );
}

#[test]
fn the_validation_runs_on_the_fetched_base_and_catches_a_cross_branch_failure() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    support::commit_file(&golden.repo, "base.txt", "a", "the base arrives");
    git::git(&golden.repo, &["push", "origin", "HEAD:main"]);
    git::git(&golden.lease, &["fetch", "origin"]);
    git::git(&golden.lease, &["merge", "--ff-only", "origin/main"]);

    support::commit_file(&golden.repo, "base.txt", "b", "the base moves on");
    git::git(&golden.repo, &["push", "origin", "HEAD:main"]);

    support::commit_file(
        &golden.lease,
        "impl.txt",
        "a",
        "the worker reads the old base",
    );
    support::write_compare_validation_script(&golden.lease);
    git::git(&golden.lease, &["add", "."]);
    git::git(&golden.lease, &["commit", "-m", "the project gate"]);
    let worker_commit = golden.head();
    golden.delete_local_base("main");

    let submitted = golden.worker_submits();
    assert_eq!(
        submitted.status.code(),
        Some(0),
        "the submission failed: {}",
        support::stderr(&submitted)
    );
    daemon
        .tick()
        .expect("the daemon validates the merged tree and holds the task");

    let failed = golden.task();
    assert_eq!(failed.state, TaskState::Failed);
    let record = failed.validations.last().expect("the validation ran");
    assert_ne!(
        record.exit_code, 0,
        "the merged tree fails the gate: {}",
        record.output_tail
    );
    assert!(
        !record.output_tail.trim().is_empty(),
        "a failed gate keeps its output: {}",
        record.output_tail
    );
    assert!(
        record.base_commit.is_some(),
        "the fetched base commit is recorded beside the evidence"
    );
    assert_eq!(
        golden.head(),
        worker_commit,
        "the worker worktree keeps its head"
    );
    assert_eq!(git::git(&golden.lease, &["status", "--porcelain"]), "");
    assert!(
        !git::git(&golden.lease, &["worktree", "list"]).contains("depot-validation"),
        "the scratch worktree is removed"
    );
    assert_eq!(golden.pull_requests_opened(), 0);
}

#[test]
fn a_merge_conflict_with_the_fetched_base_is_a_validation_failure() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    support::commit_file(&golden.repo, "base.txt", "start", "the base arrives");
    git::git(&golden.repo, &["push", "origin", "HEAD:main"]);
    git::git(&golden.lease, &["fetch", "origin"]);
    git::git(&golden.lease, &["merge", "--ff-only", "origin/main"]);

    support::commit_file(
        &golden.repo,
        "base.txt",
        "the base version",
        "the base changes",
    );
    git::git(&golden.repo, &["push", "origin", "HEAD:main"]);

    support::commit_file(
        &golden.lease,
        "base.txt",
        "the worker version",
        "the worker changes the same file",
    );
    let worker_commit = golden.head();
    golden.delete_local_base("main");

    let submitted = golden.worker_submits();
    assert_eq!(submitted.status.code(), Some(0));
    daemon
        .tick()
        .expect("the daemon reports the conflict as a validation failure");

    let failed = golden.task();
    assert_eq!(failed.state, TaskState::Failed);
    let record = failed
        .validations
        .last()
        .expect("the conflict is recorded as validation evidence");
    assert_ne!(record.exit_code, 0);
    assert!(
        record.output_tail.contains("CONFLICT"),
        "the conflict keeps its output: {}",
        record.output_tail
    );
    assert_eq!(golden.head(), worker_commit);
    assert!(
        !git::git(&golden.lease, &["worktree", "list"]).contains("depot-validation"),
        "the scratch worktree is removed after a conflict"
    );
    assert_eq!(golden.pull_requests_opened(), 0);
}

#[test]
fn a_project_describes_against_the_fetched_base_without_a_local_base_branch() {
    let golden = Golden::new(Validation::Passing);
    golden.set_describe_profile(PROFILE);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    let submitted = golden.worker_commits_and_submits();
    assert_eq!(submitted.status.code(), Some(0));
    let commit = golden.head();
    golden.script_pull_request(&commit);
    golden.delete_local_base("main");

    daemon
        .tick()
        .expect("the daemon fetches the base, describes and opens the pull request");

    let opened = golden.task();
    assert_eq!(opened.state, TaskState::PrOpen);
    assert_eq!(golden.pull_requests_opened(), 1);
    let request = golden
        .forge
        .requests()
        .into_iter()
        .find(|request| request.method == "POST")
        .expect("the pull request opens");
    assert!(request.body.contains("## Why"), "{:#}", request.body);
    assert!(
        request.body.contains("from the fetched base diff"),
        "the describe worker ran against the fetched base: {:#}",
        request.body
    );
}

#[test]
fn a_commit_already_on_the_base_branch_lands_without_a_pull_request() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    git::git(&golden.lease, &["push", "origin", "HEAD:main"]);

    daemon
        .tick()
        .expect("the daemon sees the commit on the base and lands the task");

    let landed = golden.task();
    assert_eq!(landed.state, TaskState::Landed);
    assert!(
        golden
            .history(TASK)
            .contains(&"task_landed_on_base".to_string()),
        "the landing is on the journal: {:?}",
        golden.history(TASK)
    );
    assert!(
        !golden.history(TASK).contains(&PUSHED.to_string()),
        "a landed commit is never pushed as a delivery branch"
    );
    assert_eq!(
        golden.pull_requests_opened(),
        0,
        "a landed commit opens no pull request"
    );
    assert!(
        !calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "the landing releases the worktree"
    );
    assert_eq!(
        landed.validated_commit().map(CommitId::as_str),
        Some(commit.as_str())
    );
}

#[test]
fn a_commit_the_base_does_not_contain_is_pushed_and_opens_a_pull_request() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    let commit = golden.head();
    golden.script_pull_request(&commit);

    daemon
        .tick()
        .expect("the daemon validates, pushes and opens the pull request");

    let opened = golden.task();
    assert_eq!(opened.state, TaskState::PrOpen);
    assert!(
        golden.history(TASK).contains(&PUSHED.to_string()),
        "a novel commit is pushed"
    );
    assert!(
        golden.history(TASK).contains(&OPENED.to_string()),
        "a novel commit opens a pull request"
    );
    assert_eq!(golden.pull_requests_opened(), 1);
}

#[test]
fn a_base_that_cannot_be_read_holds_the_task_instead_of_landing_it() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    golden.set_pull_request_base("no-such-base");

    daemon
        .tick()
        .expect("a base that cannot be read is recorded, not fatal");

    let held = golden.task();
    assert_eq!(held.state, TaskState::Failed);
    assert!(
        golden
            .history(TASK)
            .contains(&"validation_failed".to_string()),
        "the failure is on the journal: {:?}",
        golden.history(TASK)
    );
    assert!(
        !golden
            .history(TASK)
            .contains(&"task_landed_on_base".to_string()),
        "a base read that failed never lands the task"
    );
    assert!(
        !golden.history(TASK).contains(&PUSHED.to_string()),
        "a base read that failed never pushes the branch"
    );
    assert_eq!(golden.pull_requests_opened(), 0);

    daemon.tick().expect("later ticks stay alive");
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "validation_failed")
            .count(),
        1,
        "a held task records one failure rather than one per tick"
    );

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(inbox.contains("the validation could not run"), "{inbox}");
}

#[test]
fn a_refused_pull_request_holds_only_its_task_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();
    golden.script_pull_request_refused();

    let second = golden.clone_second_lease("2", BRANCH_TWO);
    std::fs::write(second.join("second.txt"), "the second task\n")
        .expect("the second worktree file is written");
    git::git(&second, &["add", "."]);
    git::git(&second, &["commit", "-m", "the second task"]);
    let second_commit = git::head(&second);
    git::git(&second, &["push", "origin", "HEAD:main"]);
    golden.hold_two_leases(&second, LEASE_TWO);
    golden
        .store
        .put_task(&validated_task(
            &golden.project.id,
            TASK_TWO,
            LEASE_TWO,
            &second_commit,
        ))
        .expect("the second validated task is recorded");

    daemon
        .tick()
        .expect("one refused pull request does not stop the tick");

    let refused = golden.task();
    assert_eq!(refused.state, TaskState::Failed);
    assert!(
        golden
            .history(TASK)
            .contains(&"delivery_failed".to_string()),
        "the refused pull request is on the journal: {:?}",
        golden.history(TASK)
    );
    assert!(
        !golden.history(TASK).contains(&OPENED.to_string()),
        "the forge answered 422, so the refused pull request is never mislabelled as opened: {:?}",
        golden.history(TASK)
    );

    let landed = golden
        .store
        .task(&golden.project.id, &TaskId::new(TASK_TWO))
        .expect("the second task is read")
        .expect("the second task exists");
    assert_eq!(
        landed.state,
        TaskState::Landed,
        "the other task still reaches its end"
    );

    daemon.tick().expect("later ticks stay alive");
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "delivery_failed")
            .count(),
        1,
        "a held task records one failure rather than one per tick"
    );

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("422"),
        "the reason names the forge answer: {inbox}"
    );
}

fn zero_run_duration(golden: &Golden) {
    golden
        .home
        .write_settings(&depotd::Settings {
            run_duration_minutes: 0,
            ..support::settings()
        })
        .expect("the run duration is set to zero");
}

#[test]
fn a_run_past_its_duration_stops_the_session_once_and_holds_the_task() {
    let golden = Golden::new(Validation::Passing);
    zero_run_duration(&golden);
    let daemon = golden.daemon();
    golden.propose();
    daemon
        .tick()
        .expect("the launch meets the deadline in the same tick");

    let task = golden.task();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(
        task.attempts.last().map(|attempt| attempt.outcome),
        Some(AttemptOutcome::Stopped)
    );
    let exceeded = |golden: &Golden| {
        golden
            .history(TASK)
            .into_iter()
            .filter(|kind| kind == "run_duration_exceeded")
            .count()
    };
    assert_eq!(exceeded(&golden), 1);
    assert_eq!(
        golden.boxr.calls_to("stop").len(),
        1,
        "the exact deadline stops the session exactly once"
    );

    daemon.tick().expect("a later tick stays alive");
    assert_eq!(exceeded(&golden), 1, "the overrun is recorded once");
    assert_eq!(golden.boxr.calls_to("stop").len(), 1);
}

#[test]
fn a_run_duration_stop_that_fails_defers_its_task_instead_of_stopping_the_daemon() {
    let golden = Golden::new(Validation::Passing);
    zero_run_duration(&golden);
    let daemon = golden.daemon();
    golden.propose();
    golden
        .boxr
        .respond("stop", "", "boxr could not stop the session", 1);

    daemon
        .tick()
        .expect("a failed stop is deferred rather than fatal");
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_deferred".to_string()),
        "the failed stop is recorded: {:?}",
        golden.history(TASK)
    );
}

#[test]
fn a_failed_stop_is_retried_until_the_session_is_gone() {
    let golden = Golden::new(Validation::Passing);
    zero_run_duration(&golden);
    let daemon = golden.daemon();
    golden.propose();
    golden
        .boxr
        .respond("stop", "", "boxr could not stop the session", 1);

    daemon
        .tick()
        .expect("a failed stop is deferred rather than fatal");
    assert_eq!(golden.boxr.calls_to("stop").len(), 1);

    golden.boxr.respond("stop", "", "", 0);
    daemon.tick().expect("the next tick retries the stop");
    assert_eq!(
        golden.boxr.calls_to("stop").len(),
        2,
        "a session still reported running is stopped again"
    );

    daemon.tick().expect("a stopped session is left alone");
    assert_eq!(
        golden.boxr.calls_to("stop").len(),
        2,
        "the retry stops once the session is gone"
    );
}

#[test]
fn a_terminal_session_failure_records_its_reason_for_the_checklist_and_inbox() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.boxr.respond(
        "status",
        &format!("session: {SESSION}\nstate: failed\nerror: \"the harness crashed\"\n"),
        "",
        0,
    );
    daemon.tick().expect("the failure is recorded");

    let task = golden.task();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.failure.as_deref(), Some("the harness crashed"));
    let history = golden.status_history();
    assert!(
        history.contains("failure: the harness crashed"),
        "{history}"
    );

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("the worker session failed: the harness crashed"),
        "{inbox}"
    );

    daemon.tick().expect("later ticks stay alive");
    assert_eq!(
        golden
            .history(TASK)
            .into_iter()
            .filter(|kind| kind == "worker_session_failed")
            .count(),
        1,
        "the failure is recorded once rather than every tick"
    );
}

#[test]
fn a_rate_limited_session_queues_a_bounded_retry_without_a_bare_gone() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.boxr.respond(
        "status",
        &format!(
            "session: {SESSION}\nstate: failed\nlimitHit: true\nerror: \"weekly usage limit reached\"\n"
        ),
        "",
        0,
    );
    daemon.tick().expect("the rate limit is recorded");

    let task = golden.task();
    assert_eq!(task.state, TaskState::Approved);
    assert!(task.retry.is_some(), "the bounded retry is queued");
    let history = golden.history(TASK);
    assert_eq!(
        history
            .iter()
            .filter(|kind| kind.as_str() == "provider_rate_limited")
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|kind| kind.as_str() == LIVENESS)
            .count(),
        1,
        "the launch observation is the only liveness fact; no bare gone overwrites the limit"
    );

    daemon.tick().expect("a later tick stays alive");
    assert_eq!(
        golden
            .history(TASK)
            .into_iter()
            .filter(|kind| kind == "provider_rate_limited")
            .count(),
        1
    );
}

#[test]
fn a_resume_that_reports_the_parent_id_is_retried_instead_of_recorded() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_asks("Which store?");
    assert_eq!(
        golden.depot_ok(&[
            "task",
            "answer",
            TASK,
            "--text",
            "sqlite.",
            "--by",
            "user",
            "--project",
            SLUG,
        ]),
        format!("answered {TASK}\n")
    );
    golden.boxr.child_session(SESSION);

    daemon
        .tick()
        .expect("a parent reported as the child is refused without failing the tick");
    assert_eq!(
        golden
            .task()
            .attempts
            .last()
            .and_then(|attempt| attempt.session.clone()),
        Some(SessionId::new(SESSION)),
        "the parent session is never recorded as the resumed child"
    );
    assert_eq!(golden.boxr.calls_to("resume").len(), 1);
    assert_eq!(
        golden
            .history(TASK)
            .into_iter()
            .filter(|kind| kind == "worker_turn_started")
            .count(),
        1,
        "no second turn start is recorded for the refused child"
    );

    daemon.tick().expect("the refused resume is retried");
    assert_eq!(
        golden.boxr.calls_to("resume").len(),
        2,
        "the answer stays owed until a real child is recorded"
    );
}

#[test]
fn repeated_identical_questions_are_delivered_as_two_occurrences() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.boxr.report_finished();

    golden.worker_asks_twice("Which store?", "Which store?");
    for answer in ["first answer.", "second answer."] {
        assert_eq!(
            golden.depot_ok(&[
                "task",
                "answer",
                TASK,
                "--text",
                answer,
                "--by",
                "user",
                "--project",
                SLUG,
            ]),
            format!("answered {TASK}\n")
        );
    }

    golden.boxr.report_running();
    daemon
        .tick()
        .expect("the daemon resumes with both occurrences");
    let resumed = golden.boxr.calls_to("resume");
    assert_eq!(resumed.len(), 1, "one resume carries both answers");
    let prompt = resumed[0].last().expect("the resume prompt");
    assert_eq!(
        prompt.matches("Which store?").count(),
        2,
        "the identical question is delivered as two occurrences: {prompt}"
    );
    assert!(
        prompt.contains("first answer.") && prompt.contains("second answer."),
        "both answers reach the worker: {prompt}"
    );
}

#[test]
fn an_answer_delivered_to_a_previous_session_survives_a_later_fresh_launch() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.depot_ok(&[
        "ask",
        "--task",
        TASK,
        "--relay",
        "Which store?",
        "--project",
        SLUG,
    ]);
    golden.depot_ok(&[
        "task",
        "answer",
        TASK,
        "--text",
        "sqlite.",
        "--by",
        "user",
        "--project",
        SLUG,
    ]);
    daemon
        .tick()
        .expect("the daemon resumes and delivers the first answer");
    assert_eq!(golden.boxr.calls_to("resume").len(), 1);

    golden.depot_ok(&[
        "ask",
        "--task",
        TASK,
        "--relay",
        "Which port?",
        "--project",
        SLUG,
    ]);
    golden.depot_ok(&[
        "task",
        "answer",
        TASK,
        "--text",
        "8080.",
        "--by",
        "user",
        "--project",
        SLUG,
    ]);

    golden.boxr.report_finished();
    daemon
        .tick()
        .expect("the dead session is answered with a fresh worker");

    let launches = golden.boxr.calls_to("--harness");
    assert_eq!(launches.len(), 2, "a fresh worker is launched");
    let prompt = launches[1].last().expect("the relaunch prompt");
    assert!(
        prompt.contains("Which store?") && prompt.contains("sqlite."),
        "an answer already delivered to the previous session is carried again: {prompt}"
    );
    assert!(
        prompt.contains("Which port?") && prompt.contains("8080."),
        "the newer answer reaches the same fresh turn: {prompt}"
    );
}

#[test]
fn a_submitted_commit_that_is_not_the_worktree_head_holds_only_its_task() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    let second = golden.clone_second_lease("2", BRANCH_TWO);
    std::fs::write(second.join("second.txt"), "the second task\n")
        .expect("the second worktree file is written");
    git::git(&second, &["add", "."]);
    git::git(&second, &["commit", "-m", "the second task"]);
    let second_commit = git::head(&second);
    git::git(&second, &["push", "origin", "HEAD:main"]);
    golden.hold_two_leases(&second, LEASE_TWO);
    golden
        .store
        .put_task(&validated_task(
            &golden.project.id,
            TASK_TWO,
            LEASE_TWO,
            &second_commit,
        ))
        .expect("the second validated task is recorded");

    let wrong = CommitId::new("0000000000000000000000000000000000000000");
    daemon
        .worker_submitted(TaskId::new(TASK), wrong)
        .expect("the submission is recorded");

    daemon
        .tick()
        .expect("a mismatched submission does not stop the tick");

    let held = golden.task();
    assert_eq!(held.state, TaskState::Failed);
    assert!(
        golden
            .history(TASK)
            .contains(&"validation_failed".to_string()),
        "the mismatch is a recorded fact: {:?}",
        golden.history(TASK)
    );

    let landed = golden
        .store
        .task(&golden.project.id, &TaskId::new(TASK_TWO))
        .expect("the second task is read")
        .expect("the second task exists");
    assert_eq!(
        landed.state,
        TaskState::Landed,
        "the other task still reaches its end"
    );
}

#[test]
fn a_submit_outside_the_leased_worktree_is_refused_and_records_nothing() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    let refused = golden.worker_submits_outside_the_lease();
    assert_ne!(
        refused.status.code(),
        Some(0),
        "a submit outside the lease is refused"
    );
    let message = support::stderr(&refused);
    assert!(
        message.contains("must run in the task's leased worktree"),
        "{message}"
    );
    let lease = std::fs::canonicalize(&golden.lease).expect("the lease canonicalizes");
    assert!(message.contains(&lease.display().to_string()), "{message}");

    assert_eq!(
        golden.task().state,
        TaskState::Running,
        "a refused submit leaves the task running"
    );
    assert!(
        !golden.history(TASK).contains(&SUBMITTED.to_string()),
        "a refused submit records no fact: {:?}",
        golden.history(TASK)
    );
}

#[test]
fn a_lease_missing_from_the_pool_fails_only_its_task_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    golden.worker_commits_and_submits();

    let second = golden.clone_second_lease("2", BRANCH_TWO);
    std::fs::write(second.join("second.txt"), "the second task\n")
        .expect("the second worktree file is written");
    git::git(&second, &["add", "."]);
    git::git(&second, &["commit", "-m", "the second task"]);
    let second_commit = git::head(&second);
    git::git(&second, &["push", "origin", "HEAD:main"]);
    golden.hold_only_lease(&second, LEASE_TWO);
    golden
        .store
        .put_task(&validated_task(
            &golden.project.id,
            TASK_TWO,
            LEASE_TWO,
            &second_commit,
        ))
        .expect("the second validated task is recorded");

    daemon
        .tick()
        .expect("a lease missing from the pool does not stop the tick");

    let held = golden.task();
    assert_eq!(held.state, TaskState::Failed);
    assert!(
        golden
            .history(TASK)
            .contains(&"validation_failed".to_string()),
        "the missing lease is a recorded fact: {:?}",
        golden.history(TASK)
    );
    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("could not run") && inbox.contains("not present in the pool"),
        "the reason reaches the inbox: {inbox}"
    );

    let landed = golden
        .store
        .task(&golden.project.id, &TaskId::new(TASK_TWO))
        .expect("the second task is read")
        .expect("the second task exists");
    assert_eq!(
        landed.state,
        TaskState::Landed,
        "the other task still reaches its end"
    );
}

#[test]
fn an_unreadable_session_fails_only_its_task_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    let second = golden.clone_second_lease("2", BRANCH_TWO);
    std::fs::write(second.join("second.txt"), "the second task\n")
        .expect("the second worktree file is written");
    git::git(&second, &["add", "."]);
    git::git(&second, &["commit", "-m", "the second task"]);
    let second_commit = git::head(&second);
    git::git(&second, &["push", "origin", "HEAD:main"]);
    golden.hold_two_leases(&second, LEASE_TWO);
    golden
        .store
        .put_task(&validated_task(
            &golden.project.id,
            TASK_TWO,
            LEASE_TWO,
            &second_commit,
        ))
        .expect("the second validated task is recorded");

    golden.boxr.respond("status", "", "boxr is down", 1);

    daemon
        .tick()
        .expect("an unreadable session does not stop the tick");

    let tolerated = golden.task();
    assert_eq!(
        tolerated.state,
        TaskState::Running,
        "one unreadable poll does not fail the task"
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_deferred".to_string()),
        "the unreadable session is recorded: {:?}",
        golden.history(TASK)
    );
    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("boxr is down"),
        "the reason reaches the inbox: {inbox}"
    );
    let checklist = golden.checklist();
    assert!(
        checklist.contains("worker turn deferred 1 time:") && checklist.contains("boxr is down"),
        "the deferral is visible while the task still reads as running: {checklist}"
    );

    for _ in 0..3 {
        daemon
            .tick()
            .expect("an unreadable session does not stop the tick");
    }

    let held = golden.task();
    assert_eq!(
        held.state,
        TaskState::Failed,
        "the task is held once the bounded retries run out"
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_unresolved".to_string()),
        "the unreadable session is a recorded fact: {:?}",
        golden.history(TASK)
    );
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "worker_turn_deferred")
            .count(),
        3,
        "the poll is retried a bounded number of times"
    );

    let landed = golden
        .store
        .task(&golden.project.id, &TaskId::new(TASK_TWO))
        .expect("the second task is read")
        .expect("the second task exists");
    assert_eq!(
        landed.state,
        TaskState::Landed,
        "the other task still reaches its end"
    );
}

#[test]
fn a_recovered_worker_turn_clears_its_deferral() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.boxr.respond("status", "", "boxr is down", 1);
    daemon
        .tick()
        .expect("an unreadable session does not stop the tick");
    assert!(
        golden.checklist().contains("worker turn deferred 1 time:"),
        "the deferral is visible while the outage lasts: {}",
        golden.checklist()
    );

    golden.boxr.report_running();
    daemon
        .tick()
        .expect("a recovered session does not stop the tick");

    let recovered = golden.task();
    assert_eq!(recovered.state, TaskState::Running);
    assert!(
        recovered.turn_deferral.is_none(),
        "recovery clears the deferral: {:?}",
        recovered.turn_deferral
    );
    assert!(
        !golden.checklist().contains("worker turn deferred"),
        "the checklist drops the cleared line: {}",
        golden.checklist()
    );
}

#[test]
fn a_missing_profile_holds_only_its_task_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    golden.drop_profiles();

    daemon
        .tick()
        .expect("a missing profile holds the task instead of stopping the daemon");

    let held = golden.task();
    assert_eq!(
        held.state,
        TaskState::Failed,
        "the task is held when its profile cannot be resolved"
    );
    assert!(
        held.failure
            .as_deref()
            .is_some_and(|reason| reason.contains("not defined in machine-local settings")),
        "the task names the config problem: {:?}",
        held.failure
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_unresolved".to_string()),
        "the config problem is a recorded fact: {:?}",
        golden.history(TASK)
    );
}

#[test]
fn an_unreadable_worktree_pool_skips_the_lease_pass_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    assert!(
        !golden.task().release_pending.is_empty(),
        "the stopped task owes its lease"
    );

    golden
        .treehouse
        .respond("status", "", "treehouse is down", 1);
    daemon
        .tick()
        .expect("an unreadable pool skips the lease pass instead of stopping the tick");

    assert!(
        !golden.task().release_pending.is_empty(),
        "the lease stays owed until the pool is readable"
    );
}

#[test]
fn a_launch_with_a_lease_missing_from_the_pool_fails_only_its_task_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();

    golden.propose();
    let mut stalled = golden.task();
    stalled.state = TaskState::Running;
    let attempt = stalled.attempts.last_mut().expect("the attempt");
    attempt.worktree = Some(WorktreeLease::new("absent"));
    golden
        .store
        .put_task(&stalled)
        .expect("the in-flight task is recorded");
    let second = golden.clone_second_lease("2", BRANCH_TWO);
    std::fs::write(second.join("second.txt"), "the second task\n")
        .expect("the second worktree file is written");
    git::git(&second, &["add", "."]);
    git::git(&second, &["commit", "-m", "the second task"]);
    let second_commit = git::head(&second);
    git::git(&second, &["push", "origin", "HEAD:main"]);
    golden.hold_two_leases(&second, LEASE_TWO);
    golden
        .store
        .put_task(&validated_task(
            &golden.project.id,
            TASK_TWO,
            LEASE_TWO,
            &second_commit,
        ))
        .expect("the second validated task is recorded");

    for _ in 0..4 {
        daemon
            .tick()
            .expect("a lease missing from the pool does not stop the tick");
    }

    let held = golden.task();
    assert_eq!(
        held.state,
        TaskState::Failed,
        "the task is held once the bounded retries run out"
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_unresolved".to_string()),
        "the missing lease is a recorded fact: {:?}",
        golden.history(TASK)
    );
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "worker_turn_deferred")
            .count(),
        3,
        "the launch is retried a bounded number of times"
    );
    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("not present in the pool"),
        "the reason reaches the inbox: {inbox}"
    );

    let landed = golden
        .store
        .task(&golden.project.id, &TaskId::new(TASK_TWO))
        .expect("the second task is read")
        .expect("the second task exists");
    assert_eq!(
        landed.state,
        TaskState::Landed,
        "the other task still reaches its end"
    );
}

#[test]
fn a_failing_worker_launch_holds_only_its_task_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden
        .boxr
        .respond("--harness", "", "boxr launch is down", 1);
    golden.propose();

    let second = golden.clone_second_lease("2", BRANCH_TWO);
    std::fs::write(second.join("second.txt"), "the second task\n")
        .expect("the second worktree file is written");
    git::git(&second, &["add", "."]);
    git::git(&second, &["commit", "-m", "the second task"]);
    let second_commit = git::head(&second);
    git::git(&second, &["push", "origin", "HEAD:main"]);
    golden.hold_two_leases(&second, LEASE_TWO);
    golden
        .store
        .put_task(&validated_task(
            &golden.project.id,
            TASK_TWO,
            LEASE_TWO,
            &second_commit,
        ))
        .expect("the second validated task is recorded");

    daemon
        .tick()
        .expect("a failing worker launch does not stop the tick");

    let deferred = golden.task();
    assert_eq!(
        deferred.state,
        TaskState::Running,
        "the first failed launch does not fail the task outright"
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_deferred".to_string()),
        "the failed launch is a recorded fact: {:?}",
        golden.history(TASK)
    );

    daemon
        .tick()
        .expect("the unresolved launch does not stop the tick");

    let held = golden.task();
    assert_eq!(
        held.state,
        TaskState::Failed,
        "the task is held once the launch intent cannot be resolved"
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_unresolved".to_string()),
        "the failed launch is a recorded fact: {:?}",
        golden.history(TASK)
    );
    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("boxr launch is down"),
        "the reason reaches the inbox: {inbox}"
    );

    let landed = golden
        .store
        .task(&golden.project.id, &TaskId::new(TASK_TWO))
        .expect("the second task is read")
        .expect("the second task exists");
    assert_eq!(
        landed.state,
        TaskState::Landed,
        "the other task still reaches its end"
    );
}

#[test]
fn a_failing_worktree_acquire_holds_only_its_task_and_the_tick_survives() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.reject_worktree_acquire();
    golden.propose();

    let second = golden.clone_second_lease("2", BRANCH_TWO);
    std::fs::write(second.join("second.txt"), "the second task\n")
        .expect("the second worktree file is written");
    git::git(&second, &["add", "."]);
    git::git(&second, &["commit", "-m", "the second task"]);
    let second_commit = git::head(&second);
    git::git(&second, &["push", "origin", "HEAD:main"]);
    golden.free_first_and_hold_second(&second, LEASE_TWO);
    golden
        .store
        .put_task(&validated_task(
            &golden.project.id,
            TASK_TWO,
            LEASE_TWO,
            &second_commit,
        ))
        .expect("the second validated task is recorded");

    for _ in 0..4 {
        daemon
            .tick()
            .expect("a failing worktree acquire does not stop the tick");
    }

    let held = golden.task();
    assert_eq!(
        held.state,
        TaskState::Failed,
        "the task is held once the bounded retries run out"
    );
    assert!(
        golden
            .history(TASK)
            .contains(&"worker_turn_unresolved".to_string()),
        "the failed acquire is a recorded fact: {:?}",
        golden.history(TASK)
    );
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "worker_turn_deferred")
            .count(),
        3,
        "the acquire is retried a bounded number of times"
    );
    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        inbox.contains("acquire interrupted"),
        "the reason reaches the inbox: {inbox}"
    );

    let landed = golden
        .store
        .task(&golden.project.id, &TaskId::new(TASK_TWO))
        .expect("the second task is read")
        .expect("the second task exists");
    assert_eq!(
        landed.state,
        TaskState::Landed,
        "the other task still reaches its end"
    );
}

#[test]
fn a_retried_task_starts_its_deferral_ladder_from_zero() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.reject_worktree_acquire();
    golden.free_lease();
    golden.propose();

    for _ in 0..4 {
        daemon
            .tick()
            .expect("a failing worktree acquire does not stop the tick");
    }
    assert_eq!(golden.task().state, TaskState::Failed);

    golden.depot_ok(&["task", "retry", TASK, "--project", SLUG]);
    daemon
        .tick()
        .expect("the retried task defers on its own ladder");

    let retried = golden.task();
    assert_eq!(
        retried.state,
        TaskState::Running,
        "a retry earns a fresh ladder instead of inheriting the spent one"
    );
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "worker_turn_deferred")
            .count(),
        4,
        "the retried attempt records its own first deferral"
    );
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "worker_turn_unresolved")
            .count(),
        1,
        "the retry does not hold the task again immediately"
    );
}

#[test]
fn one_tick_spends_one_rung_of_the_deferral_ladder() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.free_lease();
    golden.propose();
    let mut approved = golden.task();
    approved.state = TaskState::Approved;
    approved.attempts.clear();
    golden
        .store
        .put_task(&approved)
        .expect("the approved task is recorded");

    daemon
        .tick()
        .expect("a launch that cannot find its lease does not stop the tick");

    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == "worker_turn_deferred")
            .count(),
        1,
        "one tick spends one rung, however many paths reach the same deferred turn"
    );
}

fn origin_repo(base: &std::path::Path, name: &str, origin: &str) -> std::path::PathBuf {
    let repo = base.join(name);
    std::fs::create_dir_all(&repo).expect("the repository directory");
    git::git(&repo, &["init", "--initial-branch=main"]);
    git::git(&repo, &["remote", "add", "origin", origin]);
    repo
}

fn plain_repo(base: &std::path::Path, name: &str) -> std::path::PathBuf {
    let repo = base.join(name);
    std::fs::create_dir_all(&repo).expect("the repository directory");
    git::git(&repo, &["init", "--initial-branch=main"]);
    repo
}

fn run_depot(home: &depotd::DepotHome, arguments: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_depot"))
        .args(arguments)
        .env(depotd::HOME_ENV, home.root())
        .env_remove(depotd::LEGACY_HOME_ENV)
        .env_remove("DEPOT_TASK_ID")
        .env_remove("DEPOT_ATTEMPT_ID")
        .output()
        .expect("the depot binary runs")
}

#[test]
fn the_three_forms_of_one_origin_register_as_one_project_with_three_clones() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let home = depotd::DepotHome::at(temp.path().join("agni"));
    home.ensure().expect("the agni home");
    let base = temp.path().join("repos");
    let first = origin_repo(&base, "first", "git@github.com:O/R.git");
    let second = origin_repo(&base, "second", "https://github.com/o/r/");
    let third = origin_repo(&base, "third", "ssh://git@github.com/o/r");

    for (index, repo) in [&first, &second, &third].into_iter().enumerate() {
        let output = run_depot(&home, &["project", "add", repo.to_str().expect("utf-8")]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if index > 0 {
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("added clone"),
                "a second clone must print that it added a clone, got {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    let store = depotd::Store::open(&home).expect("the store opens");
    let projects = store.projects().expect("the projects are read");
    assert_eq!(projects.len(), 1, "one origin is one project");
    assert_eq!(projects[0].id.as_str(), "github.com/o/r");
    assert_eq!(
        store
            .clones_for_project(&projects[0].id)
            .expect("the clones are read")
            .len(),
        3
    );
}

#[test]
fn a_repository_without_an_origin_registers_as_local_only_and_says_so() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let home = depotd::DepotHome::at(temp.path().join("agni"));
    home.ensure().expect("the agni home");
    let base = temp.path().join("repos");
    let repo = plain_repo(&base, "scratch");

    let output = run_depot(&home, &["project", "add", repo.to_str().expect("utf-8")]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("local-only"),
        "a repository without an origin must say it is local-only, got {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let store = depotd::Store::open(&home).expect("the store opens");
    let projects = store.projects().expect("the projects are read");
    assert_eq!(projects.len(), 1);
    let canonical = std::fs::canonicalize(&repo).expect("the repository canonicalises");
    assert_eq!(projects[0].id.as_str(), canonical.to_string_lossy());
    let clone = store
        .clone_for_project(&projects[0].id)
        .expect("the clone is read")
        .expect("a clone is recorded");
    assert_eq!(clone.origin, None, "a local-only clone has no origin");
}

#[test]
fn project_repoint_changes_the_identity_keeps_history_and_refuses_while_a_task_is_in_flight() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    daemon.tick().expect("the daemon launches the worker");
    assert_eq!(golden.task().state, TaskState::Running);

    let refused = golden.depot(&[
        "project",
        "repoint",
        SLUG,
        "--origin",
        "git@github.com:nunoras/depot.git",
    ]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("in flight"),
        "a repoint under a running task must be refused, got {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    golden.depot_ok(&["task", "stop", TASK, "--project", SLUG]);
    let stale = golden.depot(&[
        "project",
        "repoint",
        SLUG,
        "--origin",
        "git@github.com:nunoras/depot.git",
    ]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("git remote set-url"),
        "a repoint whose clone still says the old origin is refused, got {}",
        String::from_utf8_lossy(&stale.stderr)
    );
    git::git(
        &golden.repo,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:nunoras/depot.git",
        ],
    );
    let repointed = golden.depot_ok(&[
        "project",
        "repoint",
        SLUG,
        "--origin",
        "git@github.com:nunoras/depot.git",
    ]);
    assert!(
        repointed.contains("github.com/nunoras/depot"),
        "{repointed}"
    );

    let store = depotd::Store::open(&golden.home).expect("the store opens");
    let project = store
        .project(&depot_core::ProjectId::new("github.com/nunoras/depot"))
        .expect("the project is read")
        .expect("the rekeyed project exists");
    assert_eq!(project.slug, SLUG);
    assert!(
        store
            .task(&project.id, &TaskId::new(TASK))
            .expect("the task is read")
            .is_some(),
        "repointing keeps the task history"
    );
}
