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
    let binary = if cfg!(windows) {
        let binary = dir.join(format!("{name}.cmd"));
        fs::write(&binary, windows_script()).expect("the fake program is written");
        binary
    } else {
        let binary = dir.join(name);
        fs::write(&binary, unix_script()).expect("the fake program is written");
        make_executable(&binary);
        binary
    };
    scripts.insert(name.to_owned(), binary.clone());
    binary
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("the fake program exists")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("the fake program is executable");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn unix_script() -> &'static str {
    r#"#!/bin/sh
dir="$DEPOT_FAKE_DIR"
{
  for argument in "$@"; do printf '%s\n' "$argument"; done
  printf '\n'
} >> "$dir/calls.txt"
key="$1"
if [ -z "$key" ]; then key=default; fi
if [ -f "$dir/$key.stdout" ]; then cat "$dir/$key.stdout"; fi
if [ -f "$dir/$key.stderr" ]; then cat "$dir/$key.stderr" >&2; fi
if [ -f "$dir/$key.exit" ]; then exit "$(cat "$dir/$key.exit")"; fi
exit 0
"#
}

fn windows_script() -> &'static str {
    r#"@echo off
set "dir=%DEPOT_FAKE_DIR%"
set "key=%~1"
if "%key%"=="" set "key=default"
:arguments
if "%~1"=="" goto done
echo %~1>>"%dir%\calls.txt"
shift
goto arguments
:done
echo.>>"%dir%\calls.txt"
if exist "%dir%\%key%.stdout" type "%dir%\%key%.stdout"
if exist "%dir%\%key%.stderr" type "%dir%\%key%.stderr" 1>&2
set "code=0"
if exist "%dir%\%key%.exit" for /f "usebackq delims=" %%c in ("%dir%\%key%.exit") do set "code=%%c"
exit /b %code%
"#
}
