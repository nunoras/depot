use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use depotd::adapters::process::Program;

const RESPONSES: &str = "DEPOT_FAKE_DIR";

pub struct FakeProgram {
    dir: PathBuf,
    binary: PathBuf,
}

impl FakeProgram {
    pub fn new(dir: &Path, name: &str) -> Self {
        fs::create_dir_all(dir).expect("the fake response directory is created");
        Self {
            dir: dir.to_owned(),
            binary: script(name),
        }
    }

    pub fn program(&self) -> Program {
        Program::new(&self.binary).with_env(RESPONSES, &self.dir)
    }

    #[allow(dead_code)]
    pub fn directory_env(&self) -> (String, PathBuf) {
        (RESPONSES.to_string(), self.dir.clone())
    }

    #[allow(dead_code)]
    pub fn install_into(&self, directory: &Path) -> PathBuf {
        fs::create_dir_all(directory).expect("the install directory is created");
        let target = directory.join(
            self.binary
                .file_name()
                .expect("the fake binary has a file name"),
        );
        fs::copy(&self.binary, &target).expect("the fake binary is installed");
        target
    }

    pub fn respond(&self, key: &str, stdout: &str, stderr: &str, exit_code: i32) {
        fs::write(self.dir.join(format!("{key}.stdout")), stdout).expect("stdout is written");
        fs::write(self.dir.join(format!("{key}.stderr")), stderr).expect("stderr is written");
        fs::write(
            self.dir.join(format!("{key}.exit")),
            format!("{exit_code}\n"),
        )
        .expect("the exit code is written");
    }

    pub fn calls(&self) -> Vec<String> {
        let Ok(recorded) = fs::read_to_string(self.dir.join("calls.txt")) else {
            return Vec::new();
        };
        recorded
            .replace("\r\n", "\n")
            .split("\n\n")
            .filter(|call| !call.trim().is_empty())
            .map(|call| call.lines().collect::<Vec<_>>().join(" "))
            .collect()
    }
}

fn script(name: &str) -> PathBuf {
    static SCRIPTS: OnceLock<Mutex<BTreeMap<String, PathBuf>>> = OnceLock::new();
    let mut scripts = SCRIPTS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .expect("the fake scripts are shared between threads");

    if let Some(binary) = scripts.get(name) {
        return binary.clone();
    }

    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fake-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the fake program directory is created");
    let binary = dir.join(executable_name(name));
    fs::copy(executable(), &binary).expect("the fake program is installed");
    scripts.insert(name.to_owned(), binary.clone());
    binary
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn executable() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| built_beside_the_tests().unwrap_or_else(compile_beside_the_tests))
        .clone()
}

fn built_beside_the_tests() -> Option<PathBuf> {
    let directory = std::env::current_exe()
        .expect("the test binary path")
        .parent()
        .expect("the test binary directory")
        .to_owned();
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(directory).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("fake_command-") || !built_binary(&name) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(at, _)| modified > *at) {
            newest = Some((modified, entry.path()));
        }
    }
    newest.map(|(_, path)| path)
}

fn built_binary(name: &str) -> bool {
    if cfg!(windows) {
        name.ends_with(".exe")
    } else {
        !name.contains('.')
    }
}

fn compile_beside_the_tests() -> PathBuf {
    let directory = std::env::current_exe()
        .expect("the test binary path")
        .parent()
        .expect("the test binary directory")
        .to_owned();
    let source = directory.join("fake_command_source.rs");
    fs::write(&source, include_str!("../fake_command.rs"))
        .expect("the fake program source is written");
    let binary = directory.join(if cfg!(windows) {
        "fake_command_built.exe"
    } else {
        "fake_command_built"
    });
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = std::process::Command::new(rustc)
        .arg("--edition")
        .arg("2024")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("rustc runs");
    assert!(
        output.status.success(),
        "the fake program could not be built, so run the suite with `cargo test` instead: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}
