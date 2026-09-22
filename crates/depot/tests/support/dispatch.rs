use crate::support::{Golden, PROFILE, SLUG, TASK, Validation, fake_typesafe, git};
use depot_core::{ProfileId, Role};
use serde_json::{Value, json};

fn configure(golden: &Golden, url: &str, rules: &str, key: bool) {
    write_project_file(golden, rules);
    let mut settings = golden.home.load_settings().unwrap();
    settings.typesafe_base_url = url.into();
    golden.home.write_settings(&settings).unwrap();
    if key {
        fake_typesafe::key(&golden.home.secrets_dir());
    }
}

fn write_project_file(golden: &Golden, contents: &str) {
    let path = golden.repo.join(depotd::PROJECT_FILE_PATH);
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(path, format!("{original}\n{contents}\n")).unwrap();
    git::git(&golden.repo, &["add", depotd::PROJECT_FILE_PATH]);
    git::git(&golden.repo, &["commit", "-m", "configure dispatch rules"]);
    git::git(&golden.repo, &["push", "origin", "main"]);
}

fn create(golden: &Golden, role: bool) -> std::process::Output {
    let mut arguments = vec![
        "task",
        "add",
        "--title",
        "Repair persistence",
        "--intent",
        "Fix the store",
        "--project",
        SLUG,
    ];
    if role {
        arguments.extend(["--role", "build"]);
    }
    golden.depot(&arguments)
}

fn rules(candidates: &str) -> String {
    format!(
        "[[dispatch.rules]]\nwhen = {:?}\nrole = \"build\"\n{candidates}",
        fake_typesafe::CONDITION
    )
}

#[test]
fn dispatch_judgement_precedes_proposal_and_pins_the_profile_through_launch() {
    let golden = Golden::new(Validation::Passing);
    let server = fake_typesafe::endpoint(fake_typesafe::CONDITION, 0.95);
    configure(
        &golden,
        &server.base_url(),
        &rules(&format!("candidates = [{PROFILE:?}]")),
        true,
    );
    let output = create(&golden, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(golden.task().role, Role::Build);
    assert_eq!(
        golden.task().dispatch_profile,
        Some(ProfileId::new(PROFILE))
    );
    let events = golden.events();
    let judged = events
        .iter()
        .position(|event| event.kind == "task_dispatch_judged")
        .unwrap();
    assert_eq!(events[judged + 1].kind, "task_proposed");
    let payload: Value = serde_json::from_str(&events[judged].payload).unwrap();
    assert_eq!(payload["source"], "model_judgement");
    assert_eq!(payload["confidence"], 0.95);
    assert_eq!(payload["chosen_rule"], 0);
    assert_eq!(payload["model"], "jev-latest");
    assert_eq!(payload["model_version"], "jev-1.13.0");
    assert_eq!(payload["rules_hash"].as_str().unwrap().len(), 64);
    assert!(
        payload["rules_snapshot"]
            .as_str()
            .unwrap()
            .contains(PROFILE)
    );
    let request = server.request_to("/v1/systemone");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer typesafe-test-secret")
    );
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(
        body,
        json!({
            "model": "jev-latest",
            "state": {"project": SLUG, "title": "Repair persistence", "intent": "Fix the store"},
            "questions": {"dispatch": {"type": "choice", "instructions": "Which condition best matches this work?", "criteria": {fake_typesafe::CONDITION: null, fake_typesafe::NEUTRAL: null}}}
        })
    );
    let path = golden.repo.join(depotd::PROJECT_FILE_PATH);
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("role = \"build\"", "role = \"review\"");
    std::fs::write(path, text).unwrap();
    git::git(&golden.repo, &["add", depotd::PROJECT_FILE_PATH]);
    git::git(&golden.repo, &["commit", "-m", "move the dispatch role"]);
    git::git(&golden.repo, &["push", "origin", "main"]);
    golden.depot_ok(&["task", "approve", TASK, "--project", SLUG]);
    golden.daemon().tick().unwrap();
    assert_eq!(golden.task().attempts[0].profile, ProfileId::new(PROFILE));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_candidate_naming_no_machine_local_profile_refuses_creation_by_name() {
    let golden = Golden::new(Validation::Passing);
    let server = fake_typesafe::endpoint(fake_typesafe::CONDITION, 0.99);
    configure(
        &golden,
        &server.base_url(),
        &rules(&format!("candidates = [{PROFILE:?}, \"absent-profile\"]")),
        true,
    );
    let output = create(&golden, false);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("absent-profile"), "got {stderr}");
    assert!(golden.store.tasks(&golden.project.id).unwrap().is_empty());
}

#[test]
fn dispatch_without_candidates_uses_the_existing_role_map() {
    let golden = Golden::new(Validation::Passing);
    let server = fake_typesafe::endpoint(fake_typesafe::CONDITION, 0.8);
    configure(&golden, &server.base_url(), &rules(""), true);
    let output = create(&golden, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        golden.task().dispatch_profile,
        Some(ProfileId::new(PROFILE))
    );
}

#[test]
fn dispatch_refusals_never_create_a_task_and_explicit_roles_bypass_the_model() {
    for (choice, confidence, floor, expected) in [
        (fake_typesafe::NEUTRAL, 0.99, 0.8, "no matching rule"),
        (fake_typesafe::CONDITION, 0.89, 0.9, "below floor"),
    ] {
        let golden = Golden::new(Validation::Passing);
        let server = fake_typesafe::endpoint(choice, confidence);
        configure(
            &golden,
            &server.base_url(),
            &format!("[dispatch]\nconfidence_floor = {floor}\n{}", rules("")),
            true,
        );
        let output = create(&golden, false);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(golden.store.tasks(&golden.project.id).unwrap().is_empty());
        assert!(create(&golden, true).status.success());
        assert_eq!(server.requests().len(), 1);
        assert!(
            golden
                .events()
                .iter()
                .all(|event| event.kind != "task_dispatch_judged")
        );
    }
}

#[test]
fn missing_keys_and_bad_rules_never_reach_the_endpoint() {
    for (config, key, expected) in [
        (rules(""), false, "typesafe-key"),
        (String::new(), true, "no dispatch.rules"),
        (
            "[dispatch]\nrules = []".into(),
            true,
            "dispatch.rules is empty",
        ),
        (
            "[dispatch]\nrules = \"bad\"".into(),
            true,
            "invalid dispatch.rules",
        ),
        (
            "[[dispatch.rules]]\nrole = \"build\"".into(),
            true,
            "invalid dispatch.rules",
        ),
        (
            format!("[dispatch]\nconfidence_floor = 1.1\n{}", rules("")),
            true,
            "confidence_floor",
        ),
    ] {
        let golden = Golden::new(Validation::Passing);
        let server = fake_typesafe::endpoint(fake_typesafe::CONDITION, 0.99);
        configure(&golden, &server.base_url(), &config, key);
        let output = create(&golden, false);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(create(&golden, true).status.success());
        assert!(server.requests().is_empty());
    }
}

#[test]
fn typesafe_api_failure_is_named_and_does_not_guess_a_role() {
    let golden = Golden::new(Validation::Passing);
    let server = fake_typesafe::endpoint(fake_typesafe::CONDITION, 0.99);
    server.replace_route("POST", "/v1/systemone", 429, "rate limited");
    configure(&golden, &server.base_url(), &rules(""), true);
    let output = create(&golden, false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTTP 429"));
    assert!(golden.store.tasks(&golden.project.id).unwrap().is_empty());
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn an_uncommitted_project_file_edit_never_wins_over_the_committed_one() {
    let golden = Golden::new(Validation::Passing);
    let server = fake_typesafe::endpoint(fake_typesafe::CONDITION, 0.95);
    configure(
        &golden,
        &server.base_url(),
        &rules(&format!("candidates = [{PROFILE:?}]")),
        true,
    );
    let gate = if cfg!(windows) {
        "validate.cmd"
    } else {
        "sh validate.sh"
    };
    let path = golden.repo.join(depotd::PROJECT_FILE_PATH);
    let committed = std::fs::read_to_string(&path).unwrap();
    let edited = committed
        .replace("role = \"build\"", "role = \"review\"")
        .replace(
            &format!("command = \"{gate}\""),
            "command = \"gate-from-the-working-tree\"",
        );
    assert!(edited.contains("role = \"review\""), "the role is edited");
    assert!(
        edited.contains("gate-from-the-working-tree"),
        "the gate is edited"
    );
    std::fs::write(&path, edited).unwrap();

    let output = create(&golden, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let task = golden.task();
    assert_eq!(
        task.role,
        Role::Build,
        "dispatch judges the committed rules, not the working tree"
    );
    golden.depot_ok(&["task", "approve", TASK, "--project", SLUG]);
    golden
        .daemon()
        .tick()
        .expect("the daemon launches the worker");
    let launch = golden.boxr.calls_to("--harness");
    assert_eq!(launch.len(), 1, "one worker is launched, {launch:?}");
    let brief = launch[0].last().expect("the launch carries the prompt");
    assert!(
        brief.contains(&format!("The command is `{gate}`")),
        "the brief carries the committed gate: {brief}"
    );
    assert!(
        !brief.contains("gate-from-the-working-tree"),
        "the brief ignores the uncommitted gate: {brief}"
    );
}
