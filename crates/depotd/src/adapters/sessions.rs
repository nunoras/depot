use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use depot_core::{ProfileId, SessionId};

use crate::adapters::process::{Output, ProcessError, Program};
use crate::adapters::toon::Document;

pub const MINIMUM_BOXR_VERSION: &str = "0.2.0";
pub const REQUIRED_BOXR_COMMANDS: [&str; 5] = ["ps", "status", "wait", "stop", "resume"];
pub const REQUIRED_BOXR_FLAGS: [&str; 1] = ["--detach"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProfile {
    pub account: ProfileId,
    pub harness: String,
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    pub directory: PathBuf,
    pub profile: SessionProfile,
    pub kind: Option<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub version: String,
    pub surface: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Running,
    Finished,
    Stopped,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Failed,
    Interrupted,
    StillRunning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session: SessionId,
    pub state: SessionState,
    pub harness: String,
    pub model: String,
}

pub trait Sessions {
    fn capabilities(&self) -> Result<Capabilities, SessionError>;
    fn launch(&self, request: &LaunchRequest) -> Result<SessionId, SessionError>;
    fn resume(&self, session: &SessionId, prompt: &str) -> Result<(), SessionError>;
    fn status(&self, session: &SessionId) -> Result<SessionState, SessionError>;
    fn wait(
        &self,
        session: &SessionId,
        timeout: Option<Duration>,
    ) -> Result<TurnOutcome, SessionError>;
    fn stop(&self, session: &SessionId) -> Result<(), SessionError>;
    fn list(&self) -> Result<Vec<SessionSummary>, SessionError>;
}

#[derive(Debug, Clone)]
pub struct Boxr {
    program: Program,
}

impl Boxr {
    pub fn new(program: Program) -> Self {
        Self { program }
    }

    fn output(
        &self,
        args: &[String],
        at: Option<&std::path::Path>,
    ) -> Result<Output, SessionError> {
        self.program.run_ok(args, at).map_err(SessionError::Command)
    }

    fn document(&self, command: &str, output: &Output) -> Result<Document, SessionError> {
        Document::parse(&output.stdout).map_err(|error| SessionError::MalformedOutput {
            command: command.to_owned(),
            detail: error.to_string(),
        })
    }
}

impl Sessions for Boxr {
    fn capabilities(&self) -> Result<Capabilities, SessionError> {
        let version_output = self
            .program
            .run_ok(&["--version".to_owned()], None)
            .map_err(SessionError::Command)?;
        let version = Version::parse(&version_output.stdout).ok_or_else(|| {
            SessionError::MalformedOutput {
                command: self.program.command_line(&["--version".to_owned()]),
                detail: format!("no version in {:?}", version_output.stdout_trimmed()),
            }
        })?;

        let help = self
            .program
            .run_ok(&["--help".to_owned()], None)
            .map_err(SessionError::Command)?;

        let missing: Vec<String> = REQUIRED_BOXR_COMMANDS
            .iter()
            .chain(REQUIRED_BOXR_FLAGS.iter())
            .filter(|required| !mentions(&help.stdout, required))
            .map(|required| (*required).to_owned())
            .collect();
        if !missing.is_empty() {
            return Err(SessionError::MissingCapabilities {
                version: version.to_string(),
                missing,
            });
        }
        if version < Version::parse(MINIMUM_BOXR_VERSION).expect("the minimum boxr version parses")
        {
            return Err(SessionError::BoxrTooOld {
                version: version.to_string(),
                required: MINIMUM_BOXR_VERSION.to_owned(),
            });
        }

        Ok(Capabilities {
            version: version.to_string(),
            surface: REQUIRED_BOXR_COMMANDS
                .iter()
                .chain(REQUIRED_BOXR_FLAGS.iter())
                .map(|required| (*required).to_owned())
                .collect(),
        })
    }

    fn launch(&self, request: &LaunchRequest) -> Result<SessionId, SessionError> {
        let mut args = vec![
            "--harness".to_owned(),
            request.profile.harness.clone(),
            "--model".to_owned(),
            request.profile.model.clone(),
            "--effort".to_owned(),
            request.profile.effort.clone(),
            "--account".to_owned(),
            request.profile.account.as_str().to_owned(),
        ];
        if let Some(kind) = &request.kind {
            args.push("--kind".to_owned());
            args.push(kind.clone());
        }
        args.push("--detach".to_owned());
        args.push(request.prompt.clone());

        let command = self.program.command_line(&args);
        let output = self.output(&args, Some(&request.directory))?;
        let id = match bare_session_id(&output.stdout) {
            Some(id) => id,
            None => {
                let document = self.document(&command, &output)?;
                document
                    .scalar("session")
                    .or_else(|| document.scalar("id"))
                    .map(str::to_owned)
                    .ok_or(SessionError::MissingSessionId {
                        command: command.clone(),
                    })?
            }
        };
        match id.trim() {
            "" => Err(SessionError::MissingSessionId { command }),
            id => Ok(SessionId::new(id)),
        }
    }

    fn resume(&self, session: &SessionId, prompt: &str) -> Result<(), SessionError> {
        let args = vec![
            "resume".to_owned(),
            session.as_str().to_owned(),
            prompt.to_owned(),
        ];
        self.output(&args, None)?;
        Ok(())
    }

    fn status(&self, session: &SessionId) -> Result<SessionState, SessionError> {
        let args = vec!["status".to_owned(), session.as_str().to_owned()];
        let command = self.program.command_line(&args);
        let output = self.output(&args, None)?;
        let document = self.document(&command, &output)?;
        let state = document
            .scalar("state")
            .ok_or_else(|| SessionError::MalformedOutput {
                command: command.clone(),
                detail: format!("no state field in {:?}", output.stdout_trimmed()),
            })?;
        session_state(&command, state)
    }

    fn wait(
        &self,
        session: &SessionId,
        timeout: Option<Duration>,
    ) -> Result<TurnOutcome, SessionError> {
        let mut args = vec!["wait".to_owned(), session.as_str().to_owned()];
        if let Some(timeout) = timeout {
            args.push("--timeout".to_owned());
            args.push(timeout.as_secs().to_string());
        }
        let command = self.program.command_line(&args);
        let output = self.output(&args, None)?;
        let document = self.document(&command, &output)?;
        let status = document
            .scalar("status")
            .ok_or_else(|| SessionError::MalformedOutput {
                command: command.clone(),
                detail: format!("no status field in {:?}", output.stdout_trimmed()),
            })?;
        match status {
            "ok" => Ok(TurnOutcome::Completed),
            "failed" => Ok(TurnOutcome::Failed),
            "interrupted" => Ok(TurnOutcome::Interrupted),
            "running" => Ok(TurnOutcome::StillRunning),
            other => Err(SessionError::UnknownState {
                command,
                state: other.to_owned(),
            }),
        }
    }

    fn stop(&self, session: &SessionId) -> Result<(), SessionError> {
        let args = vec!["stop".to_owned(), session.as_str().to_owned()];
        self.output(&args, None)?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        let args = vec!["ps".to_owned()];
        let command = self.program.command_line(&args);
        let output = self.output(&args, None)?;
        let document = self.document(&command, &output)?;
        let table = document
            .table("sessions")
            .ok_or_else(|| SessionError::MalformedOutput {
                command: command.clone(),
                detail: format!("no sessions table in {:?}", output.stdout_trimmed()),
            })?;
        let id = table
            .column("id")
            .map_err(|error| SessionError::MalformedOutput {
                command: command.clone(),
                detail: error.to_string(),
            })?;
        let state = table
            .column("state")
            .map_err(|error| SessionError::MalformedOutput {
                command: command.clone(),
                detail: error.to_string(),
            })?;
        let harness = table.column("harness").ok();
        let model = table.column("model").ok();

        table
            .rows
            .iter()
            .map(|row| {
                let id = row.get(id).map(String::as_str).unwrap_or_default();
                if id.is_empty() {
                    return Err(SessionError::MalformedOutput {
                        command: command.clone(),
                        detail: "a sessions row has an empty id".to_owned(),
                    });
                }
                let row_state = row.get(state).map(String::as_str).unwrap_or_default();
                Ok(SessionSummary {
                    session: SessionId::new(id),
                    state: session_state(&command, row_state)?,
                    harness: harness
                        .and_then(|i| row.get(i))
                        .cloned()
                        .unwrap_or_default(),
                    model: model.and_then(|i| row.get(i)).cloned().unwrap_or_default(),
                })
            })
            .collect()
    }
}

fn session_state(command: &str, state: &str) -> Result<SessionState, SessionError> {
    match state {
        "running" => Ok(SessionState::Running),
        "finished" => Ok(SessionState::Finished),
        "stopped" => Ok(SessionState::Stopped),
        "interrupted" => Ok(SessionState::Interrupted),
        "failed" => Ok(SessionState::Failed),
        other => Err(SessionError::UnknownState {
            command: command.to_owned(),
            state: other.to_owned(),
        }),
    }
}

fn bare_session_id(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    let mut lines = trimmed.lines();
    let (Some(line), None) = (lines.next(), lines.next()) else {
        return None;
    };
    let line = line.trim();
    if line.is_empty() || line.contains(':') || line.split_whitespace().count() != 1 {
        return None;
    }
    Some(line.to_owned())
}

fn mentions(help: &str, needle: &str) -> bool {
    help.lines()
        .flat_map(|line| line.split([',', ' ', '\t']))
        .any(|token| token.trim() == needle)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(text: &str) -> Option<Self> {
        text.split_whitespace().find_map(|token| {
            let token = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
            let mut parts = token.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            let patch = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some(Self {
                major,
                minor,
                patch,
            })
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug)]
pub enum SessionError {
    Command(ProcessError),
    BoxrTooOld {
        version: String,
        required: String,
    },
    MissingCapabilities {
        version: String,
        missing: Vec<String>,
    },
    MalformedOutput {
        command: String,
        detail: String,
    },
    MissingSessionId {
        command: String,
    },
    UnknownState {
        command: String,
        state: String,
    },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Command(error) => write!(f, "{error}"),
            SessionError::BoxrTooOld { version, required } => write!(
                f,
                "boxr {version} is older than the {required} depot requires, where detached sessions, resume and session listing exist"
            ),
            SessionError::MissingCapabilities { version, missing } => write!(
                f,
                "boxr {version} does not provide what depot requires: {}; depot needs boxr {MINIMUM_BOXR_VERSION} or newer with {} and never degrades silently without them",
                missing.join(", "),
                REQUIRED_BOXR_COMMANDS
                    .iter()
                    .chain(REQUIRED_BOXR_FLAGS.iter())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            SessionError::MalformedOutput { command, detail } => {
                write!(f, "{command} produced output depot cannot read: {detail}")
            }
            SessionError::MissingSessionId { command } => write!(
                f,
                "{command} printed no session id, and depot never guesses one"
            ),
            SessionError::UnknownState { command, state } => write!(
                f,
                "{command} reported the state {state:?}, which is not one depot knows"
            ),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Command(error) => Some(error),
            _ => None,
        }
    }
}
