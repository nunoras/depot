use std::env;
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
    binary: PathBuf,
}

impl FakeBoxr {
    pub fn new(root: &Path) -> Self {
        fs::create_dir_all(root).expect("the fake boxr directory is created");
        Self {
            root: root.to_owned(),
            binary: binary(),
        }
    }

    pub fn program(&self) -> Program {
        Program::new(&self.binary).with_env(DIRECTORY, &self.root)
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
                .arg("2021")
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
