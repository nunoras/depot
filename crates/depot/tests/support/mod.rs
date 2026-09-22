#![allow(dead_code)]

#[path = "../../../depotd/tests/support/fake_forge.rs"]
pub mod fake_forge;
#[path = "../../../depotd/tests/support/fake_program.rs"]
pub mod fake_program;
#[path = "../../../depotd/tests/support/fake_typesafe.rs"]
pub mod fake_typesafe;
#[path = "../../../depotd/tests/support/git.rs"]
pub mod git;

mod fake_boxr;

pub use fake_boxr::FakeBoxr;

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use depot_core::{Task, TaskId};
use depotd::adapters::forge::GitHub;
use depotd::adapters::sessions::{Boxr, Sessions};
use depotd::adapters::worktrees::Treehouse;
use depotd::{
    Daemon, DepotHome, ForgeDelivery, HOME_ENV, InstanceLock, OnEventSettings, ProfileSettings,
    Project, RecordedEvent, Settings, ShellValidation, Store,
};
use depotd::{EventHook, NoEventHook, ShellEventHook};
use fake_forge::FakeForge;
use fake_program::FakeProgram;

pub const SLUG: &str = "example";
pub const TASK: &str = "t-1";
pub const TASK_TWO: &str = "t-2";
pub const BRANCH: &str = "feat/wire-the-store";
pub const BRANCH_TWO: &str = "feat/second-task";
pub const LEASE: &str = "7c1d0a5e";
pub const LEASE_TWO: &str = "8d2e1b6f";
pub const SESSION: &str = "4f2a91";
pub const PROFILE: &str = "wire-1";
pub const HARNESS: &str = "harness-wire-1";
pub const MODEL: &str = "wire-model";
pub const ACCOUNT: &str = "wire-account";
pub const REPOSITORY: &str = "nunoras/depot";
pub const BASE: &str = "ba5eba5eba5eba5eba5eba5eba5eba5eba5eba5e";
pub const TOKEN: &str = "depot-test-token";
pub const PASSING_OUTPUT: &str = "the change is good";
pub const FAILING_OUTPUT: &str = "2 tests failed";

const DEPOT: &str = env!("CARGO_BIN_EXE_depot");
const BOXR_HELP: &str = "\
Launch coding agents and record every session in a local ledger

Usage: boxr [OPTIONS] [PROMPT]

Commands:
  ps        List running sessions
  status    Report one session without blocking
  wait      Block until a session ends
  stop      End a session
  resume    Continue a finished session

Options:
      --harness <HARNESS>  Harness to launch
      --model <MODEL>      Model to launch
      --effort <EFFORT>    Effort level
      --account <PROFILE>  Account profile to launch with
      --kind <KIND>        Session kind
      --detach             Return the session id and leave the session running
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validation {
    Passing,
    Failing,
}

pub struct Golden {
    pub temp: tempfile::TempDir,
    pub home: DepotHome,
    pub store: Store,
    pub project: Project,
    pub repo: PathBuf,
    pub base: PathBuf,
    pub origin: PathBuf,
    pub lease: PathBuf,
    pub scripts: PathBuf,
    pub fakes: PathBuf,
    pub forge: FakeForge,
    pub boxr: FakeBoxr,
    pub treehouse: FakeProgram,
    _lock: Option<InstanceLock>,
}

impl Golden {
    pub fn new(validation: Validation) -> Self {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let base = temp.path().join("golden");
        fs::create_dir_all(&base).expect("the fixture base directory");
        let home = DepotHome::at(temp.path().join("depot-home"));
        home.ensure().expect("the depot home");
        home.write_settings(&settings()).expect("the settings");

        let origin = base.join("nunoras").join("depot.git");
        fs::create_dir_all(origin.parent().expect("the origin directory"))
            .expect("the origin directory");
        git::git(
            &base,
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                origin.to_str().expect("the origin is utf-8"),
            ],
        );

        let repo = base.join(SLUG);
        git::git(
            &base,
            &[
                "clone",
                &file_url(&origin),
                repo.to_str().expect("the repository is utf-8"),
            ],
        );
        configure(&repo);
        write_project(&repo, validation);
        git::git(&repo, &["add", "."]);
        git::git(
            &repo,
            &["commit", "-m", "the project the fixture runs against"],
        );
        git::git(&repo, &["push", "origin", "main"]);

        let lease = base.join("pool").join("1");
        git::git(
            &base,
            &[
                "clone",
                &file_url(&origin),
                lease.to_str().expect("the lease is utf-8"),
            ],
        );
        configure(&lease);
        git::git(&lease, &["checkout", "-b", BRANCH]);

        let store = Store::open(&home).expect("the store opens");

        let fakes = temp.path().join("fakes");
        let boxr = FakeBoxr::new(&fakes.join("boxr"));
        boxr.respond("--version", "boxr 0.3.1\n", "", 0);
        boxr.respond("--help", BOXR_HELP, "", 0);
        boxr.respond("--harness", &format!("{SESSION}\n"), "", 0);
        boxr.respond("wait", "status: ok\n", "", 0);
        boxr.report_running();
        boxr.resume_reports_running();
        boxr.respond(
            "resume",
            &format!("session: {SESSION}\nstatus: running\n"),
            "",
            0,
        );
        boxr.respond(
            "stop",
            &format!("session: {SESSION}\nstate: stopped\n"),
            "",
            0,
        );
        Boxr::new(boxr.program())
            .capabilities()
            .expect("the fake boxr carries the surface depot requires");

        let treehouse = FakeProgram::new(&fakes.join("treehouse"), "treehouse");
        treehouse.respond("get", &lease_identity(&lease, LEASE), "", 0);
        treehouse.respond("return", "", "", 0);
        treehouse.respond("status", &pool(&lease, LEASE), "", 0);
        treehouse.install_into(&fakes.join("bin"));

        let forge = FakeForge::start();
        forge.route_query(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls"),
            Some(&format!("state=open&head=nunoras%3A{BRANCH}")),
            200,
            "[]",
        );

        let scripts = base.join("scripts");
        fs::create_dir_all(&scripts).expect("the scripts directory");

        let registered = Command::new(DEPOT)
            .args([
                "project",
                "add",
                repo.to_str().expect("the repository is utf-8"),
            ])
            .env(HOME_ENV, home.root())
            .current_dir(&repo)
            .output()
            .expect("the depot binary runs");
        assert_eq!(
            registered.status.code(),
            Some(0),
            "registering the project failed: {}",
            stderr(&registered)
        );

        let project = store
            .projects()
            .expect("the projects are read")
            .into_iter()
            .next()
            .expect("the project is registered");
        assert_eq!(project.slug, SLUG);

        let lock = depotd::InstanceLock::acquire(&home).expect("the daemon takes the single lock");
        lock.record_scope(std::slice::from_ref(&project))
            .expect("the daemon scope is recorded");
        hold_daemon_coverage(&home);

        Self {
            temp,
            home,
            store,
            project,
            repo,
            base,
            origin,
            lease,
            scripts,
            fakes,
            forge,
            boxr,
            treehouse,
            _lock: Some(lock),
        }
    }

    pub fn restart_lock(&mut self) {
        self._lock = None;
        for _ in 0..500 {
            match InstanceLock::acquire(&self.home) {
                Ok(lock) => {
                    self._lock = Some(lock);
                    return;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        panic!("the lock is free once the first daemon stops");
    }

    pub fn pass_validation_in_worktree(&self) {
        write_validation_script(&self.lease, Validation::Passing);
        git::git(&self.lease, &["add", "."]);
        git::git(&self.lease, &["commit", "-m", "make the project gate pass"]);
    }

    pub fn reject_worktree_acquire(&self) {
        self.treehouse.respond("get", "", "acquire interrupted", 1);
    }

    pub fn allow_worktree_acquire(&self) {
        self.treehouse
            .respond("get", &lease_identity(&self.lease, LEASE), "", 0);
    }

    pub fn free_lease(&self) {
        self.treehouse
            .respond("status", &free_pool(&self.lease), "", 0);
    }

    pub fn hold_lease(&self) {
        self.treehouse
            .respond("status", &pool(&self.lease, LEASE), "", 0);
    }

    pub fn map_build_role(&self, profile: Option<&str>) {
        self.map_role("build", profile);
    }

    pub fn map_role(&self, role: &str, profile: Option<&str>) {
        let path = self.repo.join(depotd::PROJECT_CONFIG_FILE_NAME);
        let text = fs::read_to_string(&path).expect("the project config is readable");
        let mut config =
            depotd::ProjectConfig::from_toml(&text).expect("the project config parses");
        config.profiles = profile
            .map(|profile| BTreeMap::from([(role.to_string(), profile.to_string())]))
            .unwrap_or_default();
        config
            .write(&self.repo)
            .expect("the project config is written");
    }

    pub fn force_push_divergent_branch(&self) -> String {
        self.force_push_divergent(BRANCH)
    }

    pub fn force_push_divergent(&self, branch: &str) -> String {
        let rival = self.base.join("rival");
        let _ = fs::remove_dir_all(&rival);
        git::git(
            &self.base,
            &[
                "clone",
                &file_url(&self.origin),
                rival.to_str().expect("the rival path is utf-8"),
            ],
        );
        configure(&rival);
        fs::write(rival.join("rival.txt"), "a rejected attempt\n")
            .expect("the rival file is written");
        git::git(&rival, &["add", "."]);
        git::git(&rival, &["commit", "-m", "a rejected attempt"]);
        git::git(
            &rival,
            &["push", "--force", "origin", &format!("HEAD:{branch}")],
        );
        git::git(&self.lease, &["fetch", "origin"]);
        git::git(
            &self.origin,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
        .trim()
        .to_owned()
    }

    pub fn set_auto_merge(&self, enabled: bool) {
        let path = self.repo.join(depotd::PROJECT_CONFIG_FILE_NAME);
        let text = fs::read_to_string(&path).expect("the project config is readable");
        let mut config =
            depotd::ProjectConfig::from_toml(&text).expect("the project config parses");
        config.pull_request.merge = Some(if enabled {
            depotd::MergePolicyConfig::AfterChecks
        } else {
            depotd::MergePolicyConfig::Manual
        });
        config
            .write(&self.repo)
            .expect("the project config is written");
    }

    pub fn set_pull_request_base(&self, base: &str) {
        let path = self.repo.join(depotd::PROJECT_CONFIG_FILE_NAME);
        let text = fs::read_to_string(&path).expect("the project config is readable");
        let mut config =
            depotd::ProjectConfig::from_toml(&text).expect("the project config parses");
        config.pull_request.base = base.to_owned();
        config
            .write(&self.repo)
            .expect("the project config is written");
    }

    pub fn set_describe_profile(&self, profile: &str) {
        let path = self.repo.join(depotd::PROJECT_CONFIG_FILE_NAME);
        let text = fs::read_to_string(&path).expect("the project config is readable");
        let mut config =
            depotd::ProjectConfig::from_toml(&text).expect("the project config parses");
        config.pull_request.describe_profile = Some(profile.to_owned());
        config
            .write(&self.repo)
            .expect("the project config is written");
    }

    pub fn delete_local_base(&self, base: &str) {
        git::git(&self.lease, &["branch", "-D", base]);
    }

    pub fn drop_profiles(&self) {
        self.home
            .write_settings(&Settings {
                profiles: BTreeMap::new(),
                ..settings()
            })
            .expect("the settings are rewritten without profiles");
    }

    pub fn clone_second_lease(&self, name: &str, branch: &str) -> PathBuf {
        let lease = self.base.join("pool").join(name);
        let _ = fs::remove_dir_all(&lease);
        git::git(
            &self.base,
            &[
                "clone",
                &file_url(&self.origin),
                lease.to_str().expect("the second lease is utf-8"),
            ],
        );
        configure(&lease);
        git::git(&lease, &["checkout", "-b", branch]);
        lease
    }

    pub fn hold_two_leases(&self, second: &Path, second_id: &str) {
        self.treehouse.respond(
            "status",
            &two_lease_pool(&self.lease, LEASE, second, second_id),
            "",
            0,
        );
    }

    pub fn hold_only_lease(&self, lease: &Path, id: &str) {
        self.treehouse
            .respond("status", &single_lease_pool(lease, id), "", 0);
    }

    pub fn free_first_and_hold_second(&self, second: &Path, second_id: &str) {
        self.treehouse.respond(
            "status",
            &free_first_leased_second_pool(&self.lease, second, second_id),
            "",
            0,
        );
    }

    pub fn script_pull_request_refused(&self) {
        self.forge.replace_route(
            "POST",
            &format!("/repos/{REPOSITORY}/pulls"),
            422,
            "{\"message\":\"No commits between main and the head branch\"}",
        );
    }

    pub fn daemon(
        &self,
    ) -> Daemon<'_, Boxr, Treehouse, ShellValidation, ForgeDelivery<GitHub>, Box<dyn EventHook>>
    {
        let settings = self.home.load_settings().expect("the settings");
        let hook: Box<dyn EventHook> = match &settings.on_event {
            Some(on_event) => Box::new(ShellEventHook::new(on_event.command.clone())),
            None => Box::new(NoEventHook),
        };
        Daemon::new(
            &self.store,
            self.project.clone(),
            Boxr::new(self.boxr.program()),
            Treehouse::new(self.treehouse.program()),
            ShellValidation,
            ForgeDelivery::new(GitHub::new(self.forge.base_url(), TOKEN)),
            hook,
        )
    }

    pub fn depot(&self, arguments: &[&str]) -> Output {
        Command::new(DEPOT)
            .args(arguments)
            .env(HOME_ENV, self.home.root())
            .env("PATH", with_program(&self.fakes.join("bin")))
            .env(self.boxr.directory_env().0, self.boxr.directory_env().1)
            .current_dir(&self.repo)
            .output()
            .expect("the depot binary runs")
    }

    pub fn depot_ok(&self, arguments: &[&str]) -> String {
        let output = self.depot(arguments);
        assert_eq!(
            output.status.code(),
            Some(0),
            "depot {arguments:?} exited with {}: {}",
            output.status.code().unwrap_or(-1),
            stderr(&output)
        );
        stdout(&output)
    }

    pub fn propose(&self) {
        let added = self.depot_ok(&[
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
        assert_eq!(
            self.depot_ok(&["task", "approve", TASK, "--project", SLUG]),
            format!("approved {TASK}\n")
        );
    }

    pub fn worker_commits_and_submits(&self) -> Output {
        self.worker_commits_and_submits_with("wire the store", "the work")
    }

    pub fn worker_commits_and_submits_with(&self, message: &str, contents: &str) -> Output {
        self.worker(&script(&[
            &format!("printf '{contents}\\n' > change.txt"),
            "git add change.txt",
            &format!("git commit -m \"{message}\""),
            &format!("depot submit --task {TASK} --project {SLUG}"),
        ]))
    }

    pub fn worker_submits(&self) -> Output {
        self.worker(&script(&[&format!(
            "depot submit --task {TASK} --project {SLUG}"
        )]))
    }

    pub fn worker_fast_forwards_and_submits(&self) -> Output {
        self.worker(&script(&[
            "git fetch origin",
            "git merge --ff-only origin/main",
            &format!("depot submit --task {TASK} --project {SLUG}"),
        ]))
    }

    pub fn worker_submits_outside_the_lease(&self) -> Output {
        self.worker_in(
            &self.repo,
            &script(&[&format!("depot submit --task {TASK} --project {SLUG}")]),
        )
    }

    pub fn worker_commits_and_asks(&self, question: &str) -> Output {
        self.worker(&script(&[
            "printf 'the work\\n' > change.txt",
            "git add change.txt",
            "git commit -m \"wire the store\"",
            &format!("depot ask --task {TASK} --project {SLUG} --relay \"{question}\""),
        ]))
    }

    pub fn worker_asks_settled(&self, question: &str) -> Output {
        self.worker(&script(&[&format!(
            "depot ask --task {TASK} --project {SLUG} \"{question}\""
        )]))
    }

    pub fn worker_asks_twice(&self, first: &str, second: &str) -> Output {
        self.worker(&script(&[
            &format!("depot ask --task {TASK} --project {SLUG} --relay \"{first}\""),
            &format!("depot ask --task {TASK} --project {SLUG} --relay \"{second}\""),
        ]))
    }

    pub fn fail_pull_request_read(&self) {
        self.forge.replace_route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            500,
            "{\"message\":\"Internal Server Error\"}",
        );
    }

    fn worker(&self, body: &str) -> Output {
        self.worker_in(&self.lease, body)
    }

    fn worker_in(&self, directory: &Path, body: &str) -> Output {
        let name = if cfg!(windows) {
            "worker.cmd"
        } else {
            "worker.sh"
        };
        let path = self.scripts.join(name);
        fs::write(&path, body).expect("the worker script is written");
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", path.to_str().expect("the worker path is utf-8")]);
            command
        } else {
            let mut command = Command::new("sh");
            command.arg(&path);
            command
        };
        command
            .current_dir(directory)
            .env(HOME_ENV, self.home.root())
            .env("DEPOT_TASK_ID", TASK)
            .env("DEPOT_ATTEMPT_ID", LEASE)
            .env(
                "PATH",
                with_programs(&[
                    &self.fakes.join("bin"),
                    Path::new(DEPOT).parent().expect("the depot directory"),
                ]),
            )
            .env(
                self.treehouse.directory_env().0,
                self.treehouse.directory_env().1,
            )
            .output()
            .expect("the worker script runs")
    }

    pub fn task(&self) -> Task {
        self.store
            .task(&self.project.id, &TaskId::new(TASK))
            .expect("the task is read")
            .expect("the task exists")
    }

    pub fn head(&self) -> String {
        git::head(&self.lease)
    }

    pub fn commit_in_lease(&self, file: &str, contents: &str) -> String {
        fs::write(self.lease.join(file), contents).expect("the file is written in the lease");
        git::git(&self.lease, &["add", file]);
        git::git(&self.lease, &["commit", "-m", "the rework"]);
        git::head(&self.lease)
    }

    pub fn advance_base_conflicting(&self, file: &str, contents: &str) -> String {
        fs::write(self.repo.join(file), contents).expect("the base file is written");
        git::git(&self.repo, &["add", file]);
        git::git(&self.repo, &["commit", "-m", "advance the base"]);
        git::git(&self.repo, &["push", "origin", "main"]);
        git::head(&self.repo)
    }

    pub fn checklist(&self) -> String {
        fs::read_to_string(self.home.project_home(&self.project.slug).checklist_path())
            .expect("the checklist is written by depot")
    }

    pub fn status(&self) -> String {
        self.refresh_daemon_heartbeat();
        self.depot_ok(&["status", "--project", SLUG])
    }

    pub fn status_history(&self) -> String {
        self.refresh_daemon_heartbeat();
        self.depot_ok(&["status", "--project", SLUG, "--history"])
    }

    fn refresh_daemon_heartbeat(&self) {
        if let Some(lock) = &self._lock {
            lock.refresh_heartbeat().expect("the daemon heartbeat");
        }
    }

    pub fn status_matches_checklist(&self, rendered: &str) {
        let without_observations: String = rendered
            .lines()
            .filter(|line| !line.contains("- attempt:"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(without_observations, self.checklist());
    }

    pub fn events(&self) -> Vec<RecordedEvent> {
        self.store.events(&self.project.id).expect("the journal")
    }

    pub fn history(&self, task: &str) -> Vec<String> {
        self.events()
            .into_iter()
            .filter(|event| event.task.as_ref().is_some_and(|id| id.as_str() == task))
            .map(|event| event.kind)
            .collect()
    }

    pub fn state_history(&self, task: &str) -> Vec<String> {
        self.history(task)
            .into_iter()
            .filter(|kind| {
                !matches!(
                    kind.as_str(),
                    "worker_liveness_changed"
                        | "worktree_acquire_requested"
                        | "worker_turn_launch_requested"
                        | "worker_turn_resume_requested"
                        | "worker_relaunch_requested"
                )
            })
            .collect()
    }

    pub fn script_pull_request(&self, commit: &str) {
        self.forge.route(
            "GET",
            &format!("/repos/{REPOSITORY}/commits/{commit}/check-runs"),
            200,
            &check_runs(),
        );
        self.forge.route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &pull_request(commit, BASE, "open", false, true),
        );
        self.forge.route(
            "POST",
            &format!("/repos/{REPOSITORY}/pulls"),
            201,
            &format!(
                "{{\"number\":1,\"html_url\":\"https://forge.test/{REPOSITORY}/pull/1\",\"state\":\"open\"}}"
            ),
        );
    }

    pub fn script_failing_checks(&self, commit: &str) {
        self.forge.replace_route(
            "GET",
            &format!("/repos/{REPOSITORY}/commits/{commit}/check-runs"),
            200,
            &check_runs_with("failure"),
        );
    }

    pub fn script_pull_request_base(&self, commit: &str, base: &str) {
        self.forge.replace_route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &pull_request(commit, base, "open", false, true),
        );
    }

    pub fn script_existing_pull_request(&self, commit: &str) {
        self.script_existing_pull_request_with_body(commit, None);
    }

    pub fn script_existing_pull_request_with_body(&self, commit: &str, body: Option<&str>) {
        self.forge.route(
            "GET",
            &format!("/repos/{REPOSITORY}/commits/{commit}/check-runs"),
            200,
            &check_runs(),
        );
        self.forge.route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &pull_request_with_body(commit, BASE, "open", false, true, body),
        );
        self.forge.route(
            "PATCH",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &format!("{{\"number\":1,\"html_url\":\"https://forge.test/{REPOSITORY}/pull/1\",\"state\":\"open\"}}"),
        );
        self.forge.replace_route_query(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls"),
            Some(&format!("state=open&head=nunoras%3A{BRANCH}")),
            200,
            &format!(
                "[{{\"number\":1,\"html_url\":\"https://forge.test/{REPOSITORY}/pull/1\",\"state\":\"open\"}}]"
            ),
        );
    }

    pub fn script_merge(&self, commit: &str) {
        self.forge.route(
            "GET",
            &format!("/repos/{REPOSITORY}/commits/{commit}/check-runs"),
            200,
            &check_runs(),
        );
        self.forge.replace_route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &pull_request(commit, BASE, "closed", true, false),
        );
    }

    pub fn script_close_unmerged(&self, commit: &str) {
        self.forge.replace_route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &pull_request(commit, BASE, "closed", false, false),
        );
    }

    pub fn script_merge_endpoint(&self) {
        self.forge.replace_route(
            "PUT",
            &format!("/repos/{REPOSITORY}/pulls/1/merge"),
            200,
            "{\"sha\":\"merged\",\"merged\":true,\"message\":\"Pull Request successfully merged\"}",
        );
    }

    pub fn script_conflicting_pull_request(&self, commit: &str) {
        self.script_pull_request(commit);
        self.script_conflicting_pull_request_at(commit, BASE);
    }

    pub fn script_conflicting_pull_request_at(&self, commit: &str, base: &str) {
        self.forge.replace_route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &pull_request(commit, base, "open", false, false),
        );
    }

    pub fn script_merged_pull_request(&self, commit: &str) {
        self.forge.route(
            "GET",
            &format!("/repos/{REPOSITORY}/commits/{commit}/check-runs"),
            200,
            &check_runs(),
        );
        self.forge.replace_route(
            "GET",
            &format!("/repos/{REPOSITORY}/pulls/1"),
            200,
            &pull_request(commit, BASE, "open", false, true),
        );
    }

    pub fn script_delete_branch_endpoint(&self) {
        self.forge.route(
            "DELETE",
            &format!("/repos/{REPOSITORY}/git/refs/heads/{BRANCH}"),
            204,
            "",
        );
    }

    pub fn branch_deletes(&self) -> usize {
        self.forge
            .requests()
            .into_iter()
            .filter(|request| request.method == "DELETE")
            .count()
    }

    pub fn worker_merges_and_submits(&self) -> Output {
        let merge = if cfg!(windows) {
            "git merge origin/main"
        } else {
            "git merge origin/main || true"
        };
        self.worker(&script(&[
            "git fetch origin",
            merge,
            "printf 'the resolved work\\n' > change.txt",
            "git add change.txt",
            "git commit --no-edit",
            &format!("depot submit --task {TASK} --project {SLUG}"),
        ]))
    }

    pub fn script_merge_endpoint_refused(&self) {
        self.forge.replace_route(
            "PUT",
            &format!("/repos/{REPOSITORY}/pulls/1/merge"),
            405,
            "{\"message\":\"Pull Request is not mergeable\"}",
        );
    }

    pub fn merge_requests(&self) -> usize {
        self.forge
            .requests()
            .into_iter()
            .filter(|request| request.method == "PUT")
            .count()
    }

    pub fn pull_requests_opened(&self) -> usize {
        self.forge
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST")
            .count()
    }
}

pub fn validated_task(
    project: &depot_core::ProjectId,
    id: &str,
    lease: &str,
    commit: &str,
) -> Task {
    Task {
        id: TaskId::new(id),
        project: project.clone(),
        title: format!("task {id}"),
        intent: format!("intent for {id}"),
        role: depot_core::Role::Build,
        dispatch_profile: None,
        state: depot_core::TaskState::Validated,
        dependencies: Vec::new(),
        base_dependency: None,
        attempts: vec![depot_core::Attempt {
            last_seen_at: None,
            session: None,
            profile: depot_core::ProfileId::new(PROFILE),
            worktree: Some(depot_core::WorktreeLease::new(lease)),
            started_at: depot_core::Timestamp::from_millis(0),
            finished_at: Some(depot_core::Timestamp::from_millis(1)),
            outcome: depot_core::AttemptOutcome::Submitted,
            base_merge: false,
        }],
        questions: Vec::new(),
        validations: vec![depot_core::ValidationRecord {
            command: "cargo test".to_owned(),
            commit: depot_core::CommitId::new(commit),
            base_commit: None,
            exit_code: 0,
            duration: std::time::Duration::from_secs(1),
            output_tail: "ok".to_owned(),
        }],
        submission: None,
        artifacts: Vec::new(),
        links: Vec::new(),
        branch_head: None,
        merge_refused: None,
        retry: None,
        created_at: depot_core::Timestamp::from_millis(0),
        updated_at: depot_core::Timestamp::from_millis(1),
        ..Task::default()
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn calls_to(calls: &[String], command: &str) -> Vec<String> {
    calls
        .iter()
        .filter(|call| call.split_whitespace().next() == Some(command))
        .cloned()
        .collect()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn settings() -> Settings {
    settings_with_on_event(None)
}

fn hold_daemon_coverage(home: &DepotHome) {
    let path = home.root().join(depotd::DAEMON_SCOPE_FILE_NAME);
    let mut scope: depotd::DaemonScope =
        serde_json::from_slice(&std::fs::read(&path).expect("the daemon scope record"))
            .expect("the daemon scope record parses");
    scope.heartbeat_millis = u64::MAX;
    std::fs::write(
        &path,
        serde_json::to_vec(&scope).expect("the daemon scope record encodes"),
    )
    .expect("the daemon scope record is rewritten");
}

pub fn settings_with_on_event(on_event: Option<OnEventSettings>) -> Settings {
    Settings {
        on_event,
        poll_interval_seconds: 30,
        profiles: BTreeMap::from([(
            PROFILE.to_string(),
            ProfileSettings {
                harness: HARNESS.to_string(),
                model: MODEL.to_string(),
                effort: "high".to_string(),
                account: ACCOUNT.to_string(),
            },
        )]),
        ..Settings::default()
    }
}

pub fn write_project(repo: &Path, validation: Validation) {
    write_validation_script(repo, validation);
    let script = if cfg!(windows) {
        "validate.cmd".to_owned()
    } else {
        "validate.sh".to_owned()
    };
    let validation_command = if cfg!(windows) {
        script
    } else {
        format!("sh {script}")
    };
    fs::write(
        repo.join(".depot.toml"),
        format!(
            "base_branch = \"main\"\n\n[profiles]\nbuild = \"{PROFILE}\"\nfix = \"{PROFILE}\"\n\n[validation]\ncommand = \"{validation_command}\"\n\n[pull_request]\nbase = \"main\"\nmerge = \"manual\"\n"
        ),
    )
    .expect("the committed project config is written");
}

pub fn write_validation_script(repo: &Path, validation: Validation) {
    let (output, exit_code) = match validation {
        Validation::Passing => (PASSING_OUTPUT, 0),
        Validation::Failing => (FAILING_OUTPUT, 1),
    };
    let (script, command) = if cfg!(windows) {
        (
            "validate.cmd".to_owned(),
            format!("@echo off\r\necho {output}\r\nexit /b {exit_code}\r\n"),
        )
    } else {
        (
            "validate.sh".to_owned(),
            format!("printf '{output}\\n'\nexit {exit_code}\n"),
        )
    };
    fs::write(repo.join(&script), command).expect("the validation script is written");
}

pub fn commit_file(directory: &Path, file: &str, contents: &str, message: &str) -> String {
    fs::write(directory.join(file), contents).expect("the file is written");
    git::git(directory, &["add", file]);
    git::git(directory, &["commit", "-m", message]);
    git::head(directory)
}

pub fn write_compare_validation_script(directory: &Path) {
    let (script, command) = if cfg!(windows) {
        (
            "validate.cmd".to_owned(),
            "@echo off\r\nfc /b base.txt impl.txt\r\nexit /b %errorlevel%\r\n".to_owned(),
        )
    } else {
        (
            "validate.sh".to_owned(),
            "cmp base.txt impl.txt\n".to_owned(),
        )
    };
    fs::write(directory.join(script), command).expect("the compare validation script is written");
}

fn configure(directory: &Path) {
    git::git(directory, &["config", "user.name", "depot"]);
    git::git(directory, &["config", "user.email", "depot@example.test"]);
    git::git(directory, &["config", "commit.gpgsign", "false"]);
}

fn file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    format!("file:///{path}")
}

fn script(lines: &[&str]) -> String {
    let newline = if cfg!(windows) { "\r\n" } else { "\n" };
    let header = if cfg!(windows) { "@echo off" } else { "set -e" };
    let mut body = String::from(header);
    for line in lines {
        body.push_str(newline);
        body.push_str(line);
    }
    body.push_str(newline);
    body
}

fn with_program(directory: &Path) -> OsString {
    with_programs(&[directory])
}

fn with_programs(directories: &[&Path]) -> OsString {
    let mut paths: Vec<PathBuf> = directories.iter().map(|path| path.to_path_buf()).collect();
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    env::join_paths(paths).expect("the path is joined")
}

fn lease_identity(lease: &Path, id: &str) -> String {
    format!(
        "{{\"path\":{},\"lease_id\":\"{id}\",\"lease_holder\":\"depot:{TASK}\",\"leased_at\":\"2026-09-17T00:00:00Z\"}}",
        quoted(lease)
    )
}

fn pool(lease: &Path, id: &str) -> String {
    format!(
        "[{{\"name\":\"1\",\"path\":{},\"status\":\"leased\",\"lease_id\":\"{id}\",\"lease_holder\":\"depot:{TASK}\"}},{{\"name\":\"2\",\"path\":{},\"status\":\"free\",\"lease_id\":\"\",\"lease_holder\":\"\"}}]",
        quoted(lease),
        quoted(&lease.with_file_name("2"))
    )
}

fn two_lease_pool(first: &Path, first_id: &str, second: &Path, second_id: &str) -> String {
    format!(
        "[{{\"name\":\"1\",\"path\":{},\"status\":\"leased\",\"lease_id\":\"{first_id}\",\"lease_holder\":\"depot:{TASK}\"}},{{\"name\":\"2\",\"path\":{},\"status\":\"leased\",\"lease_id\":\"{second_id}\",\"lease_holder\":\"depot:{TASK_TWO}\"}}]",
        quoted(first),
        quoted(second)
    )
}

fn single_lease_pool(lease: &Path, id: &str) -> String {
    format!(
        "[{{\"name\":\"2\",\"path\":{},\"status\":\"leased\",\"lease_id\":\"{id}\",\"lease_holder\":\"depot:{TASK_TWO}\"}}]",
        quoted(lease)
    )
}

fn free_first_leased_second_pool(first: &Path, second: &Path, second_id: &str) -> String {
    format!(
        "[{{\"name\":\"1\",\"path\":{},\"status\":\"free\",\"lease_id\":\"\",\"lease_holder\":\"\"}},{{\"name\":\"2\",\"path\":{},\"status\":\"leased\",\"lease_id\":\"{second_id}\",\"lease_holder\":\"depot:{TASK_TWO}\"}}]",
        quoted(first),
        quoted(second)
    )
}

fn free_pool(lease: &Path) -> String {
    format!(
        "[{{\"name\":\"1\",\"path\":{},\"status\":\"free\",\"lease_id\":\"\",\"lease_holder\":\"\"}},{{\"name\":\"2\",\"path\":{},\"status\":\"free\",\"lease_id\":\"\",\"lease_holder\":\"\"}}]",
        quoted(lease),
        quoted(&lease.with_file_name("2"))
    )
}

fn quoted(path: &Path) -> String {
    serde_json::Value::String(path.to_string_lossy().into_owned()).to_string()
}

fn check_runs() -> String {
    check_runs_with("success")
}

fn check_runs_with(conclusion: &str) -> String {
    format!(
        "{{\"total_count\":1,\"check_runs\":[{{\"status\":\"completed\",\"conclusion\":\"{conclusion}\"}}]}}"
    )
}

fn pull_request(commit: &str, base: &str, state: &str, merged: bool, mergeable: bool) -> String {
    pull_request_with_body(commit, base, state, merged, mergeable, None)
}

fn pull_request_with_body(
    commit: &str,
    base: &str,
    state: &str,
    merged: bool,
    mergeable: bool,
    body: Option<&str>,
) -> String {
    let merged = if state == "closed" {
        format!("\"merged\":{merged},")
    } else {
        String::new()
    };
    let body = match body {
        Some(body) => format!("\"body\":{},", serde_json::Value::String(body.to_owned())),
        None => String::new(),
    };
    format!(
        "{{\"number\":1,\"html_url\":\"https://forge.test/{REPOSITORY}/pull/1\",\"title\":\"Wire the store\",{body}\"state\":\"{state}\",{merged}\"mergeable\":{mergeable},\"head\":{{\"sha\":\"{commit}\",\"ref\":\"{BRANCH}\"}},\"base\":{{\"sha\":\"{base}\"}}}}"
    )
}
