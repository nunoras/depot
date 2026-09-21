use depot_core::{ProjectId, Role, Task, TaskId, TaskState, Timestamp, delivery_branch};

fn task(title: &str, intent: &str, role: Role) -> Task {
    Task {
        id: TaskId::new("t-13"),
        project: ProjectId::new("depot"),
        title: title.to_owned(),
        intent: intent.to_owned(),
        role,
        dispatch_profile: None,
        state: TaskState::Approved,
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
        rework_of: None,
        retry: None,
        created_at: Timestamp::from_millis(0),
        updated_at: Timestamp::from_millis(0),
    }
}

fn taken(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn a_build_task_takes_a_feat_prefix_and_a_title_slug() {
    let task = task(
        "Takes value switch",
        "Switch the value over to the worker branch.",
        Role::Build,
    );
    assert_eq!(delivery_branch(&task, &[]), "feat/takes-value-switch");
}

#[test]
fn the_role_defaults_the_prefix_when_the_words_do_not() {
    let fix = task("Lease handback", "Hand the lease back.", Role::Fix);
    assert_eq!(delivery_branch(&fix, &[]), "fix/lease-handback");

    let review = task("Read the diff", "Read the diff.", Role::Review);
    assert_eq!(delivery_branch(&review, &[]), "refactor/read-the-diff");

    let plan = task("Map the work", "Map the work.", Role::Plan);
    assert_eq!(delivery_branch(&plan, &[]), "chore/map-the-work");
}

#[test]
fn the_words_in_the_task_outrank_the_role() {
    let fix = task("Lease handback", "Fix the broken handback.", Role::Build);
    assert_eq!(delivery_branch(&fix, &[]), "fix/lease-handback");

    let refactor = task("Lease handback", "Refactor the handback.", Role::Build);
    assert_eq!(delivery_branch(&refactor, &[]), "refactor/lease-handback");

    let docs = task("Write it down", "Update the docs.", Role::Build);
    assert_eq!(delivery_branch(&docs, &[]), "chore/write-it-down");
}

#[test]
fn a_slug_is_lowercase_and_kept_short() {
    let task = task(
        "Make the Store's SQLite Records Quite a Lot Better Than They Were Before, Honestly",
        "Improve persistence.",
        Role::Build,
    );
    assert_eq!(
        delivery_branch(&task, &[]),
        "feat/make-the-store-s-sqlite-records-quite-a"
    );
}

#[test]
fn an_unnamed_task_falls_back_to_the_task_id() {
    let task = task("///", "___", Role::Build);
    assert_eq!(delivery_branch(&task, &[]), "feat/t-13");
}

#[test]
fn a_collision_earns_a_short_suffix_and_a_free_name_does_not() {
    let task = task("Takes value switch", "Switch the value.", Role::Build);
    assert_eq!(
        delivery_branch(&task, &taken(&["feat/takes-value-switch"])),
        "feat/takes-value-switch-2"
    );
    assert_eq!(
        delivery_branch(
            &task,
            &taken(&["feat/takes-value-switch", "feat/takes-value-switch-2"])
        ),
        "feat/takes-value-switch-3"
    );
    assert_eq!(
        delivery_branch(&task, &taken(&["feat/something-else"])),
        "feat/takes-value-switch"
    );
}

#[test]
fn the_same_task_and_branches_return_the_same_name() {
    let task = task("Takes value switch", "Switch the value.", Role::Build);
    let taken = taken(&["feat/takes-value-switch"]);
    assert_eq!(
        delivery_branch(&task, &taken),
        delivery_branch(&task, &taken)
    );
}

#[test]
fn the_slug_comes_from_the_title_and_the_prefix_from_the_intent() {
    let fix = task("Lease handback", "Fix the broken handback.", Role::Build);
    let refactor = task("Lease handback", "Refactor the handback.", Role::Build);
    assert_eq!(
        delivery_branch(&fix, &[]),
        delivery_branch(&refactor, &[]).replacen("refactor/", "fix/", 1)
    );
}
