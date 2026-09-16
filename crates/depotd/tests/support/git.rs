use std::path::{Path, PathBuf};
use std::process::Command;

pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.args([
        "-c",
        "user.name=depot",
        "-c",
        "user.email=depot@example.test",
        "-c",
        "commit.gpgsign=false",
    ]);
    command.args(args).current_dir(dir);
    let output = command.output().expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub struct Fixture {
    pub repo: PathBuf,
    pub origin: PathBuf,
    pub lease: PathBuf,
    pub first_commit: String,
}

pub fn repo_with_remote(base: &Path) -> Fixture {
    let origin = base.join("origin.git");
    let repo = base.join("repo");
    let lease = base.join("lease");

    let mut bare = Command::new("git");
    bare.args(["init", "--bare", "--initial-branch=main"])
        .arg(&origin);
    let output = bare.output().expect("git runs");
    assert!(output.status.success(), "the bare origin is created");

    let mut clone = Command::new("git");
    clone.arg("clone").arg(&origin).arg(&repo);
    let output = clone.output().expect("git runs");
    assert!(output.status.success(), "the repository is cloned");

    std::fs::write(repo.join("readme.md"), "depot\n").expect("the first file is written");
    git(&repo, &["add", "readme.md"]);
    git(&repo, &["commit", "-m", "first"]);
    git(&repo, &["push", "origin", "main"]);
    let first_commit = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();

    let mut clone = Command::new("git");
    clone.arg("clone").arg(&origin).arg(&lease);
    let output = clone.output().expect("git runs");
    assert!(output.status.success(), "the lease directory is cloned");

    Fixture {
        repo,
        origin,
        lease,
        first_commit,
    }
}

pub fn head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
}
