use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Program {
    path: PathBuf,
    env: Vec<(OsString, OsString)>,
}

impl Program {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            env: Vec::new(),
        }
    }

    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn run(&self, args: &[String], at: Option<&Path>) -> Result<Output, ProcessError> {
        let mut command = Command::new(&self.path);
        command.args(args);
        command.envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some(directory) = at {
            command.current_dir(directory);
        }
        let output = command.output().map_err(|source| ProcessError::Spawn {
            program: self.path.clone(),
            source,
        })?;
        Ok(Output {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    pub fn run_ok(&self, args: &[String], at: Option<&Path>) -> Result<Output, ProcessError> {
        let output = self.run(args, at)?;
        if output.succeeded() {
            Ok(output)
        } else {
            Err(ProcessError::Failed {
                command: self.command_line(args),
                status: output.status,
                stderr: output.stderr.trim().to_owned(),
            })
        }
    }

    pub fn command_line(&self, args: &[String]) -> String {
        let mut line = self.path.display().to_string();
        for arg in args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn succeeded(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

#[derive(Debug)]
pub enum ProcessError {
    Spawn {
        program: PathBuf,
        source: io::Error,
    },
    Failed {
        command: String,
        status: Option<i32>,
        stderr: String,
    },
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessError::Spawn { program, source } => {
                write!(f, "could not run {}: {source}", program.display())
            }
            ProcessError::Failed {
                command,
                status,
                stderr,
            } => match (status, stderr.is_empty()) {
                (Some(code), true) => write!(f, "{command} exited with status {code}"),
                (Some(code), false) => write!(f, "{command} exited with status {code}: {stderr}"),
                (None, true) => write!(f, "{command} was killed by a signal"),
                (None, false) => write!(f, "{command} was killed by a signal: {stderr}"),
            },
        }
    }
}

impl std::error::Error for ProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProcessError::Spawn { source, .. } => Some(source),
            ProcessError::Failed { .. } => None,
        }
    }
}
