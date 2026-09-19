use depot_core::{
    BlockingEvent, ProjectId, Role, Task, TaskId, TaskState, Timestamp, blocking_event,
};

fn task(state: TaskState) -> Task {
    Task {
        id: TaskId::new("t-1"),
        project: ProjectId::new("example/project"),
        title: "a task".to_owned(),
        intent: "intent".to_owned(),
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
        hold_pr: false,
        retry: None,
        acknowledged_at: None,
        created_at: Timestamp::from_millis(0),
        updated_at: Timestamp::from_millis(0),
    }
}

fn waiting_on_question() -> Task {
    let mut task = task(TaskState::WaitingOnQuestion);
    task.questions.push(depot_core::Question {
        text: "which store?".to_owned(),
        asked_at: Timestamp::from_millis(0),
        answer: None,
    });
    task
}

fn answered_question() -> Task {
    let mut task = waiting_on_question();
    task.questions[0].answer = Some(depot_core::Answer {
        text: "sqlite".to_owned(),
        by: depot_core::AnsweredBy::User,
        at: Timestamp::from_millis(1),
    });
    task
}

fn merge_refusal() -> Task {
    let mut task = task(TaskState::PrOpen);
    task.merge_refused = Some("the base moved".to_owned());
    task
}

#[test]
fn blocking_events_follow_the_task_state() {
    let cases: [(Task, Option<BlockingEvent>); 8] = [
        (waiting_on_question(), Some(BlockingEvent::Question)),
        (answered_question(), None),
        (task(TaskState::Failed), Some(BlockingEvent::Failed)),
        (merge_refusal(), Some(BlockingEvent::MergeRefused)),
        (task(TaskState::Landed), Some(BlockingEvent::Landed)),
        (task(TaskState::Running), None),
        (task(TaskState::PrOpen), None),
        (task(TaskState::Proposed), None),
    ];
    for (task, expected) in cases {
        assert_eq!(blocking_event(&task), expected);
    }
}

#[test]
fn event_names_are_stable() {
    assert_eq!(BlockingEvent::Question.name(), "question");
    assert_eq!(BlockingEvent::Failed.name(), "failed");
    assert_eq!(BlockingEvent::MergeRefused.name(), "merge_refused");
    assert_eq!(BlockingEvent::Landed.name(), "landed");
}
