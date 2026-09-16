#[path = "support/fake_forge.rs"]
mod fake_forge;
#[path = "support/fake_program.rs"]
mod fake_program;
#[path = "support/temp.rs"]
mod temp;

use depot_core::Checks;
use depotd::adapters::forge::{
    CredentialSource, Forge, GitHub, NewPullRequest, PrState, RepoSlug, TOKEN_FILE,
    resolve_credentials,
};
use fake_forge::FakeForge;
use fake_program::FakeProgram;
use temp::TempDir;

fn repo() -> RepoSlug {
    RepoSlug::new("acme", "widget")
}

fn pull_request(number: u64, sha: &str, state: &str, merged: Option<bool>) -> String {
    let merged = match merged {
        Some(merged) => format!("\"merged\":{merged},"),
        None => String::new(),
    };
    format!(
        "{{\"number\":{number},\"html_url\":\"https://github.com/acme/widget/pull/{number}\",\"title\":\"the work\",\"state\":\"{state}\",{merged}\"head\":{{\"sha\":\"{sha}\"}}}}"
    )
}

fn check_runs(total: u64, runs: &str) -> String {
    format!("{{\"total_count\":{total},\"check_runs\":[{runs}]}}")
}

fn success_run() -> &'static str {
    "{\"status\":\"completed\",\"conclusion\":\"success\"}"
}

fn failure_run() -> &'static str {
    "{\"status\":\"completed\",\"conclusion\":\"failure\"}"
}

#[test]
fn distinguishes_a_merged_pull_request_from_an_unmerged_one() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/7",
        200,
        &pull_request(7, "aaa111", "closed", Some(true)),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/commits/aaa111/check-runs",
        200,
        &check_runs(1, success_run()),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/8",
        200,
        &pull_request(8, "bbb222", "closed", Some(false)),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/commits/bbb222/check-runs",
        200,
        &check_runs(1, failure_run()),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/9",
        200,
        &pull_request(9, "ccc333", "open", None),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/commits/ccc333/check-runs",
        200,
        &check_runs(1, "{\"status\":\"in_progress\",\"conclusion\":null}"),
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let merged = github.pull_request(&repo(), 7).expect("the PR is read");
    assert_eq!(merged.state, PrState::Merged);
    assert_eq!(merged.checks, Checks::Passing);
    assert_eq!(merged.url, "https://github.com/acme/widget/pull/7");

    let closed = github.pull_request(&repo(), 8).expect("the PR is read");
    assert_eq!(closed.state, PrState::Closed);
    assert_eq!(closed.checks, Checks::Failing);

    let open = github.pull_request(&repo(), 9).expect("the PR is read");
    assert_eq!(open.state, PrState::Open);
    assert_eq!(open.checks, Checks::Pending);

    let recorded = forge_endpoint.request_to("/repos/acme/widget/pulls/7");
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.header("authorization"), Some("Bearer token-1"));
    assert_eq!(
        recorded.header("accept"),
        Some("application/vnd.github+json")
    );
    assert_eq!(
        forge_endpoint
            .request_to("/repos/acme/widget/commits/ccc333/check-runs")
            .query
            .as_deref(),
        Some("per_page=100&page=1")
    );
}

#[test]
fn pages_check_runs_until_every_run_is_read() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/11",
        200,
        &pull_request(11, "eee555", "open", None),
    );

    let page_one = (0..100)
        .map(|_| success_run().to_owned())
        .collect::<Vec<_>>()
        .join(",");
    forge_endpoint.route_query(
        "GET",
        "/repos/acme/widget/commits/eee555/check-runs",
        Some("per_page=100&page=1"),
        200,
        &check_runs(101, &page_one),
    );
    forge_endpoint.route_query(
        "GET",
        "/repos/acme/widget/commits/eee555/check-runs",
        Some("per_page=100&page=2"),
        200,
        &check_runs(101, failure_run()),
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let pull_request = github.pull_request(&repo(), 11).expect("the PR is read");
    assert_eq!(pull_request.checks, Checks::Failing);

    let queries: Vec<_> = forge_endpoint
        .requests()
        .into_iter()
        .filter(|request| request.path.ends_with("/check-runs"))
        .map(|request| request.query)
        .collect();
    assert_eq!(
        queries,
        vec![
            Some("per_page=100&page=1".to_owned()),
            Some("per_page=100&page=2".to_owned()),
        ]
    );
}

#[test]
fn refuses_a_truncated_check_run_list() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/12",
        200,
        &pull_request(12, "fff666", "open", None),
    );
    let page_one = (0..100)
        .map(|_| success_run().to_owned())
        .collect::<Vec<_>>()
        .join(",");
    forge_endpoint.route_query(
        "GET",
        "/repos/acme/widget/commits/fff666/check-runs",
        Some("per_page=100&page=1"),
        200,
        &check_runs(101, &page_one),
    );
    forge_endpoint.route_query(
        "GET",
        "/repos/acme/widget/commits/fff666/check-runs",
        Some("per_page=100&page=2"),
        200,
        &check_runs(101, ""),
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let error = github
        .pull_request(&repo(), 12)
        .expect_err("a truncated check-run list is refused");
    let message = error.to_string();
    assert!(message.contains("only returned 100"), "{message}");
    assert!(message.contains("never treated as passing"), "{message}");
}

#[test]
fn reads_a_pull_request_with_no_checks_as_unknown() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/7",
        200,
        &pull_request(7, "aaa111", "open", None),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/commits/aaa111/check-runs",
        200,
        "{\"total_count\":0,\"check_runs\":[]}",
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let pull_request = github.pull_request(&repo(), 7).expect("the PR is read");
    assert_eq!(pull_request.checks, Checks::Unknown);
}

#[test]
fn fails_loudly_when_github_refuses_the_read() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/404",
        404,
        "{\"message\":\"Not Found\"}",
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let error = github
        .pull_request(&repo(), 404)
        .expect_err("a missing pull request is reported");
    assert!(error.to_string().contains("nothing at"), "{error}");

    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/401",
        401,
        "{\"message\":\"Bad credentials\"}",
    );
    let error = github
        .pull_request(&repo(), 401)
        .expect_err("a refused credential is reported");
    let message = error.to_string();
    assert!(message.contains("refused the credentials"), "{message}");
    assert!(
        message.contains("/repos/acme/widget/pulls/401"),
        "{message}"
    );
}

#[test]
fn refuses_an_answer_it_cannot_read() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route("GET", "/repos/acme/widget/pulls/7", 200, "{\"number\":7}");
    forge_endpoint.route("GET", "/repos/acme/widget/pulls/8", 200, "not json at all");
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/9",
        200,
        &pull_request(9, "ccc333", "closed", None),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/10",
        200,
        &pull_request(10, "ddd444", "open", None),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/commits/ddd444/check-runs",
        200,
        &check_runs(1, "{\"status\":\"completed\",\"conclusion\":\"vibes\"}"),
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let error = github
        .pull_request(&repo(), 7)
        .expect_err("a pull request without a url is refused");
    assert!(error.to_string().contains("no html_url"), "{error}");

    let error = github
        .pull_request(&repo(), 8)
        .expect_err("an unreadable body is refused");
    assert!(
        error.to_string().contains("not what depot reads"),
        "{error}"
    );

    let error = github
        .pull_request(&repo(), 9)
        .expect_err("a closed pull request without a merged flag is refused");
    assert!(error.to_string().contains("no merged flag"), "{error}");

    let error = github
        .pull_request(&repo(), 10)
        .expect_err("an unknown check conclusion is refused");
    assert!(error.to_string().contains("vibes"), "{error}");
}

#[test]
fn opens_a_pull_request() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "POST",
        "/repos/acme/widget/pulls",
        201,
        "{\"number\":42,\"html_url\":\"https://github.com/acme/widget/pull/42\"}",
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let opened = github
        .open_pull_request(&NewPullRequest {
            repo: repo(),
            title: "Add the redirect".to_owned(),
            body: "intent: fix the login redirect\nvalidation: cargo test passed".to_owned(),
            head: "fm/task-7".to_owned(),
            base: "main".to_owned(),
        })
        .expect("the pull request opens");
    assert_eq!(opened.number, 42);
    assert_eq!(opened.url, "https://github.com/acme/widget/pull/42");

    let recorded = forge_endpoint.request_to("/repos/acme/widget/pulls");
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.header("content-type"), Some("application/json"));
    let body: serde_json::Value =
        serde_json::from_str(&recorded.body).expect("the request body is json");
    assert_eq!(body["title"], "Add the redirect");
    assert_eq!(body["head"], "fm/task-7");
    assert_eq!(body["base"], "main");
    assert_eq!(
        body["body"],
        "intent: fix the login redirect\nvalidation: cargo test passed"
    );
}

#[test]
fn refuses_an_open_that_github_rejects() {
    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "POST",
        "/repos/acme/widget/pulls",
        422,
        "{\"message\":\"Validation Failed\"}",
    );

    let github = GitHub::new(forge_endpoint.base_url(), "token-1");
    let error = github
        .open_pull_request(&NewPullRequest {
            repo: repo(),
            title: "Add the redirect".to_owned(),
            body: String::new(),
            head: "fm/task-7".to_owned(),
            base: "main".to_owned(),
        })
        .expect_err("a rejected open is reported");
    let message = error.to_string();
    assert!(message.contains("422"), "{message}");
    assert!(message.contains("Validation Failed"), "{message}");
}

#[test]
fn takes_the_credential_from_the_authenticated_gh_cli() {
    let dir = TempDir::new("forge-credential-gh");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("the depot home exists");
    let gh = FakeProgram::new(dir.path(), "gh");
    gh.respond("auth", "gh-token-9\n", "", 0);

    let credentials = resolve_credentials(&gh.program(), &home).expect("the gh CLI has a token");
    assert_eq!(credentials.token, "gh-token-9");
    assert_eq!(credentials.source, CredentialSource::GhCli);
    assert_eq!(gh.calls(), vec!["auth token"]);

    let forge_endpoint = FakeForge::start();
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/pulls/7",
        200,
        &pull_request(7, "aaa111", "open", None),
    );
    forge_endpoint.route(
        "GET",
        "/repos/acme/widget/commits/aaa111/check-runs",
        200,
        &check_runs(1, success_run()),
    );
    GitHub::new(forge_endpoint.base_url(), credentials.token)
        .pull_request(&repo(), 7)
        .expect("the PR is read with the gh token");
    assert_eq!(
        forge_endpoint
            .request_to("/repos/acme/widget/pulls/7")
            .header("authorization"),
        Some("Bearer gh-token-9")
    );
}

#[test]
fn falls_back_to_the_owner_only_token_file_in_the_depot_home() {
    let dir = TempDir::new("forge-credential-file");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("the depot home exists");
    let token_path = home.join(TOKEN_FILE);
    std::fs::write(&token_path, "file-token-3\n").expect("the token is written");
    owner_only(&token_path);

    let gh = FakeProgram::new(dir.path(), "gh");
    gh.respond("auth", "", "gh: not logged in to any hosts\n", 1);

    let credentials = resolve_credentials(&gh.program(), &home).expect("the token file is read");
    assert_eq!(credentials.token, "file-token-3");
    assert_eq!(credentials.source, CredentialSource::TokenFile(token_path));
}

#[test]
fn never_reads_a_credential_from_the_project() {
    let dir = TempDir::new("forge-credential-project");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).expect("the project exists");
    std::fs::write(project.join(TOKEN_FILE), "project-token\n").expect("the token is written");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("the depot home exists");
    let gh = FakeProgram::new(dir.path(), "gh");
    gh.respond("auth", "", "gh: not logged in to any hosts\n", 1);

    let error = resolve_credentials(&gh.program(), &home)
        .expect_err("a token in the project is not a credential");
    let message = error.to_string();
    assert!(message.contains("never from a project"), "{message}");
    assert!(
        message.contains(&home.join(TOKEN_FILE).display().to_string()),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn refuses_a_token_file_others_can_read() {
    let dir = TempDir::new("forge-credential-loose");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("the depot home exists");
    let token_path = home.join(TOKEN_FILE);
    std::fs::write(&token_path, "file-token-3\n").expect("the token is written");
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&token_path)
            .expect("the token file exists")
            .permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&token_path, permissions).expect("the token file is readable");
    }
    let gh = FakeProgram::new(dir.path(), "gh");
    gh.respond("auth", "", "gh: not logged in to any hosts\n", 1);

    let error = resolve_credentials(&gh.program(), &home)
        .expect_err("a world-readable token file is refused");
    let message = error.to_string();
    assert!(message.contains("readable beyond its owner"), "{message}");
    assert!(message.contains("owner-only"), "{message}");
    assert!(message.contains("authenticate `gh`"), "{message}");
    assert!(message.contains("environment"), "{message}");
}

#[cfg(windows)]
#[test]
fn refuses_a_token_file_others_can_read() {
    let dir = TempDir::new("forge-credential-loose-windows");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("the depot home exists");
    let token_path = home.join(TOKEN_FILE);
    std::fs::write(&token_path, "file-token-3\n").expect("the token is written");
    let granted = std::process::Command::new("icacls")
        .arg(&token_path)
        .args(["/grant", "Everyone:R"])
        .output()
        .expect("icacls runs");
    assert!(
        granted.status.success(),
        "icacls failed: {}",
        String::from_utf8_lossy(&granted.stderr)
    );
    let gh = FakeProgram::new(dir.path(), "gh");
    gh.respond("auth", "", "gh: not logged in to any hosts\n", 1);

    let error = resolve_credentials(&gh.program(), &home)
        .expect_err("a world-readable token file is refused");
    let message = error.to_string();
    assert!(message.contains("readable beyond its owner"), "{message}");
    assert!(message.contains("authenticate `gh`"), "{message}");
    assert!(message.contains("environment"), "{message}");
}

#[cfg(unix)]
fn owner_only(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .expect("the token file exists")
        .permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions).expect("the token file is owner-only");
}

#[cfg(windows)]
fn owner_only(path: &std::path::Path) {
    let reset = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r"])
        .output()
        .expect("icacls runs");
    assert!(
        reset.status.success(),
        "icacls inheritance reset failed: {}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let user = std::env::var("USERNAME").expect("USERNAME is set");
    for account in ["SYSTEM", "Administrators", user.as_str()] {
        let granted = std::process::Command::new("icacls")
            .arg(path)
            .args(["/grant", &format!("{account}:F")])
            .output()
            .expect("icacls runs");
        assert!(
            granted.status.success(),
            "icacls grant {account} failed: {}",
            String::from_utf8_lossy(&granted.stderr)
        );
    }
}

#[cfg(not(any(unix, windows)))]
fn owner_only(_path: &std::path::Path) {}
