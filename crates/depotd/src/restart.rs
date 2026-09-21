use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::daemon::{DAEMON_LOCK_FILE_NAME, DaemonScope, daemon_scope};
use crate::error::{Error, Result};
use crate::home::DepotHome;

pub const DAEMON_LOG_FILE_NAME: &str = "depotd.log";
pub const DAEMON_STOP_FILE_NAME: &str = "depotd.stop";
const STOP_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartOptions {
    pub program: PathBuf,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restarted {
    pub stopped: Option<u32>,
    pub pid: u32,
    pub log: PathBuf,
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub directory: PathBuf,
    pub log: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StopRequest {
    pid: u32,
    started_at_millis: u64,
}

pub fn restart_daemon(home: &DepotHome, options: &RestartOptions) -> Result<Restarted> {
    home.ensure()?;
    let previous = daemon_scope(home);
    let stopped = match &previous {
        Some(scope) if lock_is_held(home) => Some(stop_daemon(home, scope, options.timeout)?),
        _ => None,
    };
    let projects = previous.map(|scope| scope.projects).unwrap_or_default();
    let spec = launch_spec(home, options.program.clone(), &projects);
    let pid = launch_detached(&spec)?;
    Ok(Restarted {
        stopped,
        pid,
        log: spec.log,
        projects,
    })
}

pub fn installed_daemon() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    let directory = current.parent().ok_or_else(|| {
        Error::Home(format!(
            "cannot locate the installed daemon beside {}",
            current.display()
        ))
    })?;
    let name = if cfg!(windows) {
        "depotd.exe"
    } else {
        "depotd"
    };
    let candidate = directory.join(name);
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(Error::Home(format!(
        "no {name} beside {}: install the daemon with `cargo install --locked --path crates/depotd`",
        current.display()
    )))
}

pub fn launch_spec(home: &DepotHome, program: PathBuf, projects: &[String]) -> LaunchSpec {
    let mut arguments = Vec::new();
    for project in projects {
        arguments.push("--project".to_string());
        arguments.push(project.clone());
    }
    LaunchSpec {
        program,
        arguments,
        directory: home.root().to_path_buf(),
        log: home.root().join(DAEMON_LOG_FILE_NAME),
    }
}

pub fn launch_detached(spec: &LaunchSpec) -> Result<u32> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&spec.log)?;
    let errors = log.try_clone()?;
    let child = Command::new(&spec.program)
        .args(&spec.arguments)
        .current_dir(&spec.directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors))
        .spawn()?;
    Ok(child.id())
}

pub fn stop_requested(home: &DepotHome, pid: u32, started_at_millis: u64) -> Result<bool> {
    let path = home.root().join(DAEMON_STOP_FILE_NAME);
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(false);
    };
    let Ok(request) = serde_json::from_slice::<StopRequest>(&bytes) else {
        return Ok(false);
    };
    Ok(request.pid == pid && request.started_at_millis == started_at_millis)
}

pub fn clear_stop_request(home: &DepotHome) -> Result<()> {
    let path = home.root().join(DAEMON_STOP_FILE_NAME);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn stop_daemon(home: &DepotHome, scope: &DaemonScope, timeout: Duration) -> Result<u32> {
    if scope.pid == 0 {
        return Err(Error::Home(format!(
            "a process holds {} but the lock record names no pid: stop it by hand",
            home.root().join(DAEMON_LOCK_FILE_NAME).display()
        )));
    }
    let request = StopRequest {
        pid: scope.pid,
        started_at_millis: scope.started_at_millis,
    };
    std::fs::write(
        home.root().join(DAEMON_STOP_FILE_NAME),
        serde_json::to_vec(&request).map_err(|error| Error::Io(error.into()))?,
    )?;

    let deadline = Instant::now() + timeout;
    loop {
        if !lock_is_held(home) {
            clear_stop_request(home)?;
            return Ok(scope.pid);
        }
        if Instant::now() >= deadline {
            return Err(Error::Home(format!(
                "depotd pid {} did not stop within {}s; it may be mid-tick, so the stop request is left in place: check it, or stop pid {} by hand",
                scope.pid,
                timeout.as_secs_f64(),
                scope.pid
            )));
        }
        std::thread::sleep(STOP_POLL);
    }
}

fn lock_is_held(home: &DepotHome) -> bool {
    let path = home.root().join(DAEMON_LOCK_FILE_NAME);
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
    else {
        return false;
    };
    file.try_lock_exclusive().is_err()
}
