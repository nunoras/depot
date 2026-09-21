use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::SystemTime;

use depotd::adapters::process::Program;

pub const DIRECTORY: &str = "DEPOT_FAKE_BOXR_DIR";
const SEPARATOR: char = '\u{1f}';
const RECORD: char = '\u{1e}';

pub struct FakeBoxr {
    root: PathBuf,
    executable_directory: PathBuf,
}

impl FakeBoxr {
    pub fn new(root: &Path) -> Self {
        fs::create_dir_all(root).expect("the fake boxr directory is created");
        let executable_directory = root
            .parent()
            .expect("the fake boxr directory has a parent")
            .join("bin");
        fs::create_dir_all(&executable_directory).expect("the fake boxr bin directory is created");
        install(&binary(), &executable_directory.join(executable_name()));
        Self {
            root: root.to_owned(),
            executable_directory,
        }
    }

    pub fn directory_env(&self) -> (String, PathBuf) {
        (DIRECTORY.to_string(), self.root.clone())
    }

    pub fn program(&self) -> Program {
        Program::new(executable_name())
            .with_env(DIRECTORY, &self.root)
            .with_env("PATH", self.path())
    }

    pub fn respond(&self, key: &str, stdout: &str, stderr: &str, exit_code: u8) {
        let write = |suffix: &str, text: &str| {
            fs::write(self.root.join(format!("{key}.{suffix}")), text)
                .expect("a scripted fake boxr response is written");
        };
        write("stdout", stdout);
        write("stderr", stderr);
        write("exit", &format!("{exit_code}\n"));
    }

    pub fn respond_launches(&self, sessions: &[&str]) {
        for (index, session) in sessions.iter().enumerate() {
            let suffix = format!(".{}", index + 1);
            let write = |field: &str, text: &str| {
                fs::write(self.root.join(format!("--harness.{field}{suffix}")), text)
                    .expect("a scripted fake boxr response is written");
            };
            write("stdout", &format!("{session}\n"));
            write("stderr", "");
            write("exit", "0\n");
        }
    }

    pub fn report_running(&self) {
        self.respond(
            "status",
            &format!("session: {}\nstate: running\n", super::SESSION),
            "",
            0,
        );
    }

    pub fn report_finished(&self) {
        self.respond(
            "status",
            &format!("session: {}\nstate: finished\n", super::SESSION),
            "",
            0,
        );
    }

    pub fn respond_status_sequence(&self, states: &[&str]) {
        for (index, scripted) in states.iter().enumerate() {
            let suffix = format!(".{}", index + 1);
            let write = |field: &str, text: &str| {
                fs::write(self.root.join(format!("status.{field}{suffix}")), text)
                    .expect("a scripted status response is written");
            };
            write(
                "stdout",
                &format!("session: {}\nstate: {scripted}\n", super::SESSION),
            );
            write("stderr", "");
            write("exit", "0\n");
        }
    }

    pub fn resume_reports_running(&self) {
        self.respond(
            "resume.state",
            &format!("session: {}\nstate: running\n", super::SESSION),
            "",
            0,
        );
    }

    pub fn child_session(&self, child: &str) {
        fs::write(self.root.join("resume.child"), child)
            .expect("the fake resumed child id is written");
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        let Ok(recorded) = fs::read_to_string(self.root.join("calls.txt")) else {
            return Vec::new();
        };
        recorded
            .split(RECORD)
            .filter(|record| !record.is_empty())
            .map(|record| {
                record
                    .split(SEPARATOR)
                    .filter(|argument| !argument.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }

    pub fn calls_to(&self, command: &str) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .filter(|arguments| arguments.first().is_some_and(|first| first == command))
            .collect()
    }
}

fn executable_name() -> &'static str {
    if cfg!(windows) { "boxr.exe" } else { "boxr" }
}

#[cfg(unix)]
fn install(source: &Path, destination: &Path) {
    std::os::unix::fs::symlink(source, destination)
        .expect("the fake boxr executable is installed on PATH");
}

#[cfg(not(unix))]
fn install(source: &Path, destination: &Path) {
    fs::copy(source, destination).expect("the fake boxr executable is installed on PATH");
}

impl FakeBoxr {
    fn path(&self) -> OsString {
        let mut paths = vec![self.executable_directory.clone()];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        env::join_paths(paths).expect("the fake boxr PATH is joined")
    }
}

fn binary() -> PathBuf {
    let directory = env::current_exe()
        .expect("the test binary path")
        .parent()
        .expect("the test binary directory")
        .to_owned();
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(&directory).expect("the test binary directory is readable") {
        let entry = entry.expect("a directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("fake_boxr-") || !built_binary(&name) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(at, _)| modified > *at) {
            newest = Some((modified, entry.path()));
        }
    }
    newest
        .map(|(_, path)| path)
        .unwrap_or_else(|| compile(&directory))
}

fn built_binary(name: &str) -> bool {
    if cfg!(windows) {
        name.ends_with(".exe")
    } else {
        !name.contains('.')
    }
}

fn compile(directory: &Path) -> PathBuf {
    static COMPILED: OnceLock<PathBuf> = OnceLock::new();
    COMPILED
        .get_or_init(|| {
            let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fake_boxr.rs");
            let binary = directory.join(if cfg!(windows) {
                "fake_boxr_built.exe"
            } else {
                "fake_boxr_built"
            });
            let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
            let output = Command::new(rustc)
                .arg("--edition")
                .arg("2024")
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .output()
                .expect("rustc runs");
            assert!(
                output.status.success(),
                "the fake boxr could not be built, so run the suite with `cargo test` instead: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            binary
        })
        .clone()
}
