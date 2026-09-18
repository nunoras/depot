#[path = "support/dispatch.rs"]
mod dispatch;
mod support;

use depot_core::{
    AttemptOutcome, Checks, CommitId, ProfileId, SessionId, TaskState, WorktreeLease,
};
use depotd::InstanceLock;
use support::git;
use support::{
    ACCOUNT, BRANCH, Golden, HARNESS, LEASE, MODEL, PROFILE, SESSION, SLUG, TASK, Validation,
    calls_to,
};

const PROPOSED: &str = "task_proposed";
const APPROVED: &str = "task_approved";
const ACQUIRED: &str = "worktree_acquired";
const ACQUIRE_REQUESTED: &str = "worktree_acquire_requested";
const LAUNCH_REQUESTED: &str = "worker_turn_launch_requested";
const RESUME_REQUESTED: &str = "worker_turn_resume_requested";
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
            "worker",
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
    assert!(pull_request.body.contains("Persist the records in sqlite."));
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

    let final_checklist = golden.status();
    assert!(final_checklist.contains("Landed (1)"), "{final_checklist}");
    assert!(
        final_checklist.contains("waits on: nothing"),
        "{final_checklist}"
    );
    assert_eq!(final_checklist, golden.checklist());

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
    assert!(relayed.contains("Waiting on a question (1)"), "{relayed}");
    assert!(relayed.contains("Which store?"), "{relayed}");
    assert_eq!(relayed, golden.checklist());

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(inbox.contains("## For the user (1)"), "{inbox}");
    assert!(inbox.contains("Which store?"), "{inbox}");
    assert!(inbox.contains("waiting_on_question"), "{inbox}");

    daemon
        .tick()
        .expect("the daemon sees the paused worker and leaves it alone");
    assert_eq!(golden.task().state, TaskState::WaitingOnQuestion);
    assert_eq!(
        golden.task().attempts[0].outcome,
        AttemptOutcome::InFlight,
        "a paused worker is not a dead worker"
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

    golden.boxr.report_running();
    daemon.tick().expect("the daemon resumes the session");
    let resumed = golden.boxr.calls_to("resume");
    assert_eq!(resumed.len(), 1, "the worker is resumed once");
    assert_eq!(
        resumed[0],
        vec![
            "resume",
            SESSION,
            "Your question \"Which store?\" was answered: sqlite in the depot home. Continue the task.",
        ]
    );

    daemon
        .tick()
        .expect("a further poll does not resume the worker again");
    assert_eq!(golden.boxr.calls_to("resume").len(), 1);

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
fn a_settled_question_stays_open_and_resumes_the_worker_when_answered() {
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
        .expect("the daemon sees the finished session and leaves the settled task alone");

    let paused = golden.task();
    assert_eq!(paused.state, TaskState::Running);
    assert_eq!(
        paused.attempts[0].outcome,
        AttemptOutcome::InFlight,
        "a settled question is not a dead worker"
    );
    let open = golden.status();
    assert!(!open.contains("Blocked"), "{open}");
    assert_eq!(open, golden.checklist());
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

    golden.boxr.report_running();
    daemon
        .tick()
        .expect("the daemon resumes the worker with the answer");
    let resumed = golden.boxr.calls_to("resume");
    assert_eq!(resumed.len(), 1, "the worker is resumed once");
    assert_eq!(
        resumed[0],
        vec![
            "resume",
            SESSION,
            "Your question \"Which store?\" was answered: sqlite in the depot home. Continue the task.",
        ]
    );

    daemon
        .tick()
        .expect("a further poll does not resume the worker again");
    assert_eq!(golden.boxr.calls_to("resume").len(), 1);

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
            SESSION,
            "Your question \"Which store?\" was answered: sqlite in the depot home. Your question \"Which port?\" was answered: 8080. Continue the task.",
        ],
        "both answers reach the worker in one resume turn"
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

    let blocked = golden.status();
    assert!(
        blocked.contains("Blocked - needs a person (1)"),
        "{blocked}"
    );
    assert!(blocked.contains("waits on: a person"), "{blocked}");
    assert!(blocked.contains("exited 1"), "{blocked}");
    assert!(blocked.contains(&commit), "{blocked}");
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

    assert!(daemon.tick().is_err(), "the acquire is interrupted");
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

    assert!(daemon.tick().is_err(), "the acquire is interrupted");
    assert_eq!(calls_to(&golden.treehouse.calls(), "get").len(), 1);

    golden.allow_worktree_acquire();
    assert!(
        daemon.recover().is_err(),
        "the launch waits until the pool reports the retried lease"
    );

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
fn a_failed_resume_is_retried_on_the_next_tick_and_delivers_the_answer() {
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
    golden.boxr.respond("resume", "", "resume interrupted", 1);

    daemon
        .tick()
        .expect("a failed resume does not stop the daemon");
    assert!(golden.history(TASK).contains(&RESUME_REQUESTED.to_string()));
    assert_eq!(golden.boxr.calls_to("resume").len(), 1);
    assert_eq!(golden.task().state, TaskState::Running);
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == TURN_STARTED)
            .count(),
        1,
        "a resume that failed records no turn start"
    );

    golden.boxr.respond(
        "resume",
        &format!("session: {SESSION}\nstatus: running\n"),
        "",
        0,
    );
    daemon.tick().expect("the next tick retries the resume");

    let task = golden.task();
    assert_eq!(task.state, TaskState::Running);
    let resumed = golden.boxr.calls_to("resume");
    assert_eq!(resumed.len(), 2, "the answer is retried until it lands");
    assert_eq!(
        resumed[1],
        vec![
            "resume",
            SESSION,
            "Your question \"Which store?\" was answered: sqlite in the depot home. Continue the task.",
        ]
    );
    assert_eq!(
        golden
            .history(TASK)
            .iter()
            .filter(|kind| kind.as_str() == TURN_STARTED)
            .count(),
        2,
        "the turn starts when the resume lands"
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

    let blocked = golden.status();
    assert!(
        blocked.contains("Blocked - needs a person (1)"),
        "{blocked}"
    );
    assert_eq!(blocked, golden.checklist());

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

    assert!(daemon.tick().is_err(), "the launch is interrupted");
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

    let blocked = golden.status();
    assert!(
        blocked.contains("Blocked - needs a person (1)"),
        "{blocked}"
    );
    assert_eq!(blocked, golden.checklist());

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(inbox.contains("## For the user (1)"), "{inbox}");
    assert!(
        inbox.contains("a worker turn could not be resolved"),
        "{inbox}"
    );
}

#[test]
fn a_configuration_error_before_a_launch_leaves_the_task_launchable() {
    let golden = Golden::new(Validation::Passing);
    let daemon = golden.daemon();
    golden.propose();
    golden.map_build_role(None);

    assert!(
        daemon.tick().is_err(),
        "the daemon reports the configuration problem"
    );
    assert!(
        !golden.history(TASK).contains(&LAUNCH_REQUESTED.to_string()),
        "nothing was launched, so the journal holds no launch intent"
    );
    assert!(golden.boxr.calls_to("--harness").is_empty());

    let blocked = golden.task();
    assert_eq!(blocked.state, TaskState::Running);
    assert_eq!(blocked.attempts[0].session, None);
    assert_eq!(
        blocked.attempts[0].worktree,
        Some(WorktreeLease::new(LEASE))
    );

    golden.map_build_role(Some(PROFILE));
    daemon
        .tick()
        .expect("the daemon launches once the configuration is right");

    let launched = golden.task();
    assert_eq!(launched.state, TaskState::Running);
    assert_eq!(launched.attempts.len(), 1, "no replacement attempt");
    assert_eq!(
        launched.attempts[0].session,
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
    assert_eq!(running, golden.checklist());

    let inbox = golden.depot_ok(&["inbox", "--project", SLUG]);
    assert!(
        !inbox.contains("a worker turn could not be resolved"),
        "no worker may have been launched, got\n{inbox}"
    );
}

#[test]
fn a_restart_with_a_task_in_flight_marks_it_unknown_and_launches_no_replacement() {
    let golden = Golden::new(Validation::Passing);

    {
        let _lock = InstanceLock::acquire(&golden.home).expect("the daemon takes the single lock");
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

    let _lock = acquire_released_lock(&golden.home);
    let store = depotd::Store::open(&golden.home).expect("the restarted daemon reopens the store");
    let daemon = depotd::Daemon::new(
        &store,
        golden.project.clone(),
        depotd::adapters::sessions::Boxr::new(golden.boxr.program()),
        depotd::adapters::worktrees::Treehouse::new(golden.treehouse.program()),
        depotd::ShellValidation,
        depotd::ForgeDelivery::new(
            depotd::adapters::forge::GitHub::new(golden.forge.base_url(), support::TOKEN),
            "main",
        ),
        depotd::StderrNotifier,
    );

    golden.boxr.respond("status", "", "status interrupted", 1);
    assert!(
        daemon.recover().is_err(),
        "recovery cannot observe liveness"
    );
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
    assert_eq!(running, golden.checklist());
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

    let checklist = golden.status();
    assert!(checklist.contains("Landed (1)"), "{checklist}");
    assert!(!checklist.contains("Pull request open"), "{checklist}");
    assert_eq!(checklist, golden.checklist());
}

#[test]
fn a_pull_request_closed_unmerged_is_left_for_review() {
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
    assert!(
        calls_to(&golden.treehouse.calls(), "return").is_empty(),
        "review keeps the worktree"
    );

    let checklist = golden.status();
    assert!(checklist.contains("Cancelled (1)"), "{checklist}");
    assert!(!checklist.contains("Landed"), "{checklist}");
    assert_eq!(checklist, golden.checklist());
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

    let checklist = golden.status();
    assert!(
        checklist.contains("Blocked - needs a person (1)"),
        "{checklist}"
    );
    assert!(!checklist.contains("Landed"), "{checklist}");
    assert_eq!(checklist, golden.checklist());
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
    let checklist = golden.status();
    assert!(checklist.contains("Pull request open (1)"), "{checklist}");
    assert_eq!(checklist, golden.checklist());
}

fn forge_facts(golden: &Golden) -> Vec<String> {
    golden
        .events()
        .into_iter()
        .filter(|event| event.kind.starts_with("pull_request_"))
        .map(|event| event.kind)
        .collect()
}

fn acquire_released_lock(home: &depotd::DepotHome) -> InstanceLock {
    for _ in 0..500 {
        match InstanceLock::acquire(home) {
            Ok(lock) => return lock,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    panic!("the lock is free once the first daemon stops");
}

fn user_section(rendered: &str) -> &str {
    let start = rendered
        .find("## For the user (")
        .expect("the inbox names the user section");
    let rest = &rendered[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    &rest[..end]
}
