mod support;

#[path = "support/git.rs"]
#[allow(dead_code)]
mod git;

use std::path::{Path, PathBuf};

use depot_core::{ProjectId, TaskState, Timestamp};
use depotd::{DepotHome, LocationKind, Project, Store, add_project, render_projects};

fn home(temp: &tempfile::TempDir) -> DepotHome {
    let home = DepotHome::at(temp.path().join("agni"));
    home.ensure().expect("the agni home");
    home
}

fn repo_with_origin(base: &Path, name: &str, origin: &str) -> PathBuf {
    let repo = base.join(name);
    std::fs::create_dir_all(&repo).expect("the repository directory");
    git::git(&repo, &["init", "--initial-branch=main"]);
    git::git(&repo, &["remote", "add", "origin", origin]);
    repo
}

#[test]
fn re_adding_a_path_whose_origin_moved_is_refused_without_a_second_project() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let home = home(&temp);
    let repo = repo_with_origin(&temp.path().join("repos"), "repo", "git@github.com:o/r.git");

    let first = add_project(&home, repo.to_str().expect("utf-8")).expect("registered");
    assert_eq!(first.project.id.as_str(), "github.com/o/r");

    git::git(
        &repo,
        &["remote", "set-url", "origin", "git@github.com:o/fork.git"],
    );
    let error = add_project(&home, repo.to_str().expect("utf-8"))
        .expect_err("a moved origin must not fork the project");
    let message = error.to_string();
    assert!(message.contains("already a clone"), "got {message}");
    assert!(message.contains("github.com/o/r"), "got {message}");

    let store = Store::open(&home).expect("the store opens");
    assert_eq!(
        store.projects().expect("projects").len(),
        1,
        "no second project is created"
    );
    let canonical = std::fs::canonicalize(&repo).expect("the repository canonicalises");
    let clone = store
        .clone_for_path(&canonical)
        .expect("the clone is read")
        .expect("the first clone is kept");
    assert_eq!(clone.project.as_str(), "github.com/o/r");
}

#[test]
fn a_project_without_a_clone_has_no_local_path_and_says_so() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let home = home(&temp);
    let store = Store::open(&home).expect("the store opens");
    let project = Project {
        id: ProjectId::new("github.com/o/r"),
        kind: LocationKind::Path,
        slug: "r".to_owned(),
        created_at: Timestamp::from_millis(0),
    };
    store.put_project(&project).expect("the project is stored");

    assert!(
        store
            .project_path(&project)
            .expect("the path is read")
            .is_none(),
        "a project with no clone has no local path"
    );
    let rendered = render_projects(&home).expect("projects render");
    assert!(rendered.contains("no local clone"), "{rendered}");
}

#[test]
fn acting_on_a_clone_whose_origin_moved_is_refused_while_a_task_is_in_flight() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let home = home(&temp);
    let repo = repo_with_origin(&temp.path().join("repos"), "repo", "git@github.com:o/r.git");
    let added = add_project(&home, repo.to_str().expect("utf-8")).expect("registered");
    let store = Store::open(&home).expect("the store opens");
    store
        .put_task(&support::simple_task(
            &added.project.id,
            "t-1",
            TaskState::Running,
            0,
        ))
        .expect("the task is stored");

    store
        .ensure_clone_origin(&added.project)
        .expect("a clone that matches its project is accepted");

    git::git(
        &repo,
        &["remote", "set-url", "origin", "git@github.com:o/other.git"],
    );
    let error = store
        .ensure_clone_origin(&added.project)
        .expect_err("a moved origin is refused while a task is in flight");
    let message = error.to_string();
    assert!(message.contains("github.com/o/r"), "got {message}");
    assert!(message.contains("github.com/o/other"), "got {message}");
    assert!(
        message.contains(&repo.display().to_string()),
        "the refusal names the clone, got {message}"
    );
}

#[test]
fn a_moved_origin_is_left_alone_when_no_task_is_in_flight() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let home = home(&temp);
    let repo = repo_with_origin(&temp.path().join("repos"), "repo", "git@github.com:o/r.git");
    let added = add_project(&home, repo.to_str().expect("utf-8")).expect("registered");
    let store = Store::open(&home).expect("the store opens");

    git::git(
        &repo,
        &["remote", "set-url", "origin", "git@github.com:o/other.git"],
    );
    store
        .ensure_clone_origin(&added.project)
        .expect("nothing is in flight, so nothing is refused");
}
