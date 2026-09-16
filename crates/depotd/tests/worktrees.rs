#[path = "support/fake_program.rs"]
mod fake_program;
#[path = "support/git.rs"]
mod git;
#[path = "support/temp.rs"]
mod temp;

use std::path::Path;

use depot_core::{Baseline, CommitId, WorktreeLease};
use depotd::adapters::worktrees::{AcquireRequest, Lease, Treehouse, Worktrees};
use fake_program::FakeProgram;
use serde_json::json;
use temp::TempDir;

fn lease_json(path: &Path, lease_id: &str, holder: &str) -> String {
    json!({
        "path": path,
        "lease_id": lease_id,
        "lease_holder": holder,
        "leased_at": "2026-09-16T05:56:43Z",
    })
    .to_string()
}

fn pool_status_json(leased_path: &Path) -> String {
    json!([
        {
            "name": "1",
            "path": leased_path,
            "status": "leased",
            "flavor": "git",
            "lease_id": "7c1d0a5e",
            "lease_holder": "task-7",
            "leased_at": "2026-09-16T05:56:43Z",
            "processes": []
        },
        {
            "name": "2",
            "path": "/pool/2/repo",
            "status": "free",
            "flavor": "git",
            "lease_id": "",
            "lease_holder": "",
            "leased_at": "",
            "processes": []
        }
    ])
    .to_string()
}

fn request(repo: &std::path::Path, baseline: Baseline) -> AcquireRequest {
    AcquireRequest {
        repo: repo.to_owned(),
        holder: "task-7".to_owned(),
        baseline,
    }
}

#[test]
fn acquires_releases_and_reads_the_pool() {
    let dir = TempDir::new("worktrees-round-trip");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond(
        "get",
        &lease_json(&fixture.lease, "7c1d0a5e", "task-7"),
        "",
        0,
    );
    fake.respond("return", "", "", 0);
    fake.respond("status", &pool_status_json(&fixture.lease), "", 0);

    let treehouse = Treehouse::new(fake.program());
    let lease = treehouse
        .acquire(&request(&fixture.repo, Baseline::DefaultBranchHead))
        .expect("a worktree is leased");
    assert_eq!(lease.lease, WorktreeLease::new("7c1d0a5e"));
    assert_eq!(lease.path, fixture.lease);
    assert_eq!(lease.holder, "task-7");
    assert_eq!(lease.acquired_at, "2026-09-16T05:56:43Z");

    treehouse
        .release(&lease)
        .expect("a clean lease is returned");

    let pool = treehouse.pool(&fixture.repo).expect("the pool is read");
    assert_eq!(pool.len(), 2);
    assert_eq!(pool[0].name, "1");
    assert_eq!(pool[0].state, "leased");
    assert_eq!(pool[0].lease, Some(WorktreeLease::new("7c1d0a5e")));
    assert_eq!(pool[0].holder.as_deref(), Some("task-7"));
    assert_eq!(pool[1].state, "free");
    assert_eq!(pool[1].lease, None);

    assert_eq!(
        fake.calls(),
        vec![
            "get --lease --json --lease-holder task-7".to_owned(),
            format!("return {} --if-lease-id 7c1d0a5e", fixture.lease.display()),
            "status --json".to_owned(),
        ]
    );
    for call in fake.calls() {
        assert!(!call.contains("--force"), "{call} would reset the worktree");
    }
}

#[test]
fn refuses_to_release_a_lease_holding_uncommitted_work() {
    let dir = TempDir::new("worktrees-uncommitted");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond(
        "get",
        &lease_json(&fixture.lease, "7c1d0a5e", "task-7"),
        "",
        0,
    );
    fake.respond("return", "", "", 0);

    let treehouse = Treehouse::new(fake.program());
    let lease = treehouse
        .acquire(&request(&fixture.repo, Baseline::DefaultBranchHead))
        .expect("a worktree is leased");

    std::fs::write(fixture.lease.join("readme.md"), "half a change\n")
        .expect("the lease holds uncommitted work");

    let error = treehouse
        .release(&lease)
        .expect_err("a lease holding work is not released");
    let message = error.to_string();
    assert!(message.contains("7c1d0a5e"), "{message}");
    assert!(message.contains("uncommitted"), "{message}");
    assert!(message.contains("never reset or removed"), "{message}");
    assert_eq!(
        fake.calls(),
        vec!["get --lease --json --lease-holder task-7"],
        "depot must not ask treehouse to return a worktree holding work"
    );
}

#[test]
fn refuses_a_lease_holding_unpushed_commits_until_they_are_pushed() {
    let dir = TempDir::new("worktrees-unpushed");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond(
        "get",
        &lease_json(&fixture.lease, "7c1d0a5e", "task-7"),
        "",
        0,
    );
    fake.respond("return", "", "", 0);

    let treehouse = Treehouse::new(fake.program());
    let lease = treehouse
        .acquire(&request(&fixture.repo, Baseline::DefaultBranchHead))
        .expect("a worktree is leased");

    std::fs::write(fixture.lease.join("change.txt"), "the work\n").expect("the work is written");
    git::git(&fixture.lease, &["add", "change.txt"]);
    git::git(&fixture.lease, &["commit", "-m", "the work"]);

    let error = treehouse
        .release(&lease)
        .expect_err("a lease holding unpushed commits is not released");
    let message = error.to_string();
    assert!(message.contains("no remote branch"), "{message}");

    git::git(
        &fixture.lease,
        &["push", "origin", "HEAD:refs/heads/task-7"],
    );
    assert_eq!(
        git::git(&fixture.origin, &["rev-parse", "refs/heads/task-7"]).trim(),
        git::head(&fixture.lease)
    );

    treehouse
        .release(&lease)
        .expect("a lease whose commits are on the remote is returned");
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn pins_a_lease_to_the_dependency_commit_it_was_asked_for() {
    let dir = TempDir::new("worktrees-pinned");
    let fixture = git::repo_with_remote(dir.path());
    std::fs::write(fixture.repo.join("second.txt"), "second\n").expect("the file is written");
    git::git(&fixture.repo, &["add", "second.txt"]);
    git::git(&fixture.repo, &["commit", "-m", "second"]);
    git::git(&fixture.repo, &["push", "origin", "main"]);
    git::git(&fixture.lease, &["fetch", "origin"]);
    git::git(&fixture.lease, &["checkout", "--detach", "origin/main"]);
    assert_ne!(git::head(&fixture.lease), fixture.first_commit);

    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond(
        "get",
        &lease_json(&fixture.lease, "7c1d0a5e", "task-7"),
        "",
        0,
    );

    let treehouse = Treehouse::new(fake.program());
    let lease = treehouse
        .acquire(&request(
            &fixture.repo,
            Baseline::PinnedCommit(CommitId::new(fixture.first_commit.clone())),
        ))
        .expect("a worktree is leased");
    assert_eq!(lease.lease, WorktreeLease::new("7c1d0a5e"));
    assert_eq!(git::head(&fixture.lease), fixture.first_commit);
    assert_eq!(
        git::git(&fixture.lease, &["branch", "--show-current"]),
        "task-7\n"
    );
}

#[test]
fn returns_the_lease_when_the_pinned_commit_is_missing() {
    let dir = TempDir::new("worktrees-pin-miss");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond(
        "get",
        &lease_json(&fixture.lease, "7c1d0a5e", "task-7"),
        "",
        0,
    );
    fake.respond("return", "", "", 0);

    let missing = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let error = Treehouse::new(fake.program())
        .acquire(&request(
            &fixture.repo,
            Baseline::PinnedCommit(CommitId::new(missing)),
        ))
        .expect_err("a missing pin is refused");
    let message = error.to_string();
    assert!(
        message.contains(missing) || message.contains("checkout"),
        "{message}"
    );
    assert_eq!(
        fake.calls(),
        vec![
            "get --lease --json --lease-holder task-7".to_owned(),
            format!("return {} --if-lease-id 7c1d0a5e", fixture.lease.display()),
        ],
        "a failed pin must give the lease back"
    );
}

#[test]
fn fails_loudly_when_treehouse_has_no_worktree_to_lease() {
    let dir = TempDir::new("worktrees-exhausted");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond("get", "", "treehouse: the pool has no worktree free\n", 1);

    let error = Treehouse::new(fake.program())
        .acquire(&request(&fixture.repo, Baseline::DefaultBranchHead))
        .expect_err("an exhausted pool is reported");
    let message = error.to_string();
    assert!(message.contains("no worktree free"), "{message}");
    assert!(message.contains("--lease-holder task-7"), "{message}");
}

#[test]
fn refuses_a_lease_identity_that_carries_no_lease_id() {
    let dir = TempDir::new("worktrees-malformed");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond("get", "{\"path\":\"/pool/1/repo\"}", "", 0);

    let error = Treehouse::new(fake.program())
        .acquire(&request(&fixture.repo, Baseline::DefaultBranchHead))
        .expect_err("a lease without an id is refused");
    let message = error.to_string();
    assert!(message.contains("reported no lease_id"), "{message}");
    assert!(message.contains("get --lease --json"), "{message}");
}

#[test]
fn refuses_a_pool_status_that_is_not_a_list() {
    let dir = TempDir::new("worktrees-pool-malformed");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond("status", "{\"error\":\"not a list\"}", "", 0);

    let error = Treehouse::new(fake.program())
        .pool(&fixture.repo)
        .expect_err("a pool status that is not a list is refused");
    assert!(error.to_string().contains("not a list"), "{error}");
}

#[test]
fn refuses_unreadable_lease_json() {
    let dir = TempDir::new("worktrees-unreadable");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond("get", "leased /pool/1/repo\n", "", 0);

    let error = Treehouse::new(fake.program())
        .acquire(&request(&fixture.repo, Baseline::DefaultBranchHead))
        .expect_err("unreadable lease output is refused");
    assert!(error.to_string().contains("cannot read"), "{error}");
}

#[test]
fn keeps_a_lease_that_is_still_held_by_somebody_else_out_of_release() {
    let dir = TempDir::new("worktrees-holder");
    let fixture = git::repo_with_remote(dir.path());
    let fake = FakeProgram::new(dir.path(), "treehouse");
    fake.respond(
        "return",
        "",
        "treehouse: the lease is held by another holder\n",
        1,
    );

    let lease = Lease {
        lease: WorktreeLease::new("7c1d0a5e"),
        path: fixture.lease.clone(),
        holder: "task-7".to_owned(),
        acquired_at: "2026-09-16T05:56:43Z".to_owned(),
    };
    let error = Treehouse::new(fake.program())
        .release(&lease)
        .expect_err("a refused release is reported");
    let message = error.to_string();
    assert!(message.contains("another holder"), "{message}");
    assert!(message.contains("--if-lease-id 7c1d0a5e"), "{message}");
}
