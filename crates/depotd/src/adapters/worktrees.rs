use std::fmt;
use std::path::{Path, PathBuf};

use depot_core::{Baseline, WorktreeLease};
use serde_json::Value;

use crate::adapters::process::{Output, ProcessError, Program};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub lease: WorktreeLease,
    pub path: PathBuf,
    pub holder: String,
    pub acquired_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolEntry {
    pub name: String,
    pub path: PathBuf,
    pub state: String,
    pub lease: Option<WorktreeLease>,
    pub holder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireRequest {
    pub repo: PathBuf,
    pub holder: String,
    pub baseline: Baseline,
}

pub trait Worktrees {
    fn acquire(&self, request: &AcquireRequest) -> Result<Lease, WorktreeError>;
    fn release(&self, lease: &Lease) -> Result<(), WorktreeError>;
    fn pool(&self, repo: &Path) -> Result<Vec<PoolEntry>, WorktreeError>;
}

#[derive(Debug, Clone)]
pub struct Treehouse {
    program: Program,
    git: Program,
}

impl Treehouse {
    pub fn new(program: Program) -> Self {
        Self {
            program,
            git: Program::new("git"),
        }
    }

    fn git_args(&self, path: &Path, rest: &[&str]) -> Vec<String> {
        let mut args = vec!["-C".to_owned(), path.display().to_string()];
        args.extend(rest.iter().map(|arg| (*arg).to_owned()));
        args
    }

    fn git_output(&self, path: &Path, rest: &[&str]) -> Result<String, WorktreeError> {
        Ok(self.git.run_ok(&self.git_args(path, rest), None)?.stdout)
    }

    fn refs_containing_head(
        &self,
        path: &Path,
        only: Option<&str>,
    ) -> Result<Vec<String>, WorktreeError> {
        let mut args = vec!["for-each-ref", "--format=%(refname)", "--contains", "HEAD"];
        if let Some(only) = only {
            args.push(only);
        }
        let output = self.git_output(path, &args)?;
        Ok(non_empty_lines(&output))
    }

    fn return_lease(&self, lease: &Lease) -> Result<Output, ProcessError> {
        let args = vec![
            "return".to_owned(),
            lease.path.display().to_string(),
            "--if-lease-id".to_owned(),
            lease.lease.as_str().to_owned(),
        ];
        self.program.run_ok(&args, None)
    }

    fn unlanded_work(&self, path: &Path) -> Result<Option<String>, WorktreeError> {
        let status = self.git_output(path, &["status", "--porcelain"])?;
        let changed = non_empty_lines(&status).len();
        if changed > 0 {
            let what = if changed == 1 {
                "uncommitted path"
            } else {
                "uncommitted paths"
            };
            return Ok(Some(format!("{} holds {changed} {what}", path.display())));
        }

        if !self
            .refs_containing_head(path, Some("refs/remotes"))?
            .is_empty()
        {
            return Ok(None);
        }

        let remotes = non_empty_lines(&self.git_output(
            path,
            &["for-each-ref", "--format=%(refname)", "refs/remotes"],
        )?);
        if !remotes.is_empty() {
            let count =
                self.git_output(path, &["rev-list", "--count", "HEAD", "--not", "--remotes"])?;
            return Ok(Some(unreached_from(
                count.trim(),
                "no remote branch",
                "git -C <worktree> for-each-ref --contains HEAD refs/remotes",
            )));
        }

        if !self.refs_containing_head(path, None)?.is_empty() {
            return Ok(None);
        }
        let count = self.git_output(path, &["rev-list", "--count", "HEAD"])?;
        Ok(Some(unreached_from(
            count.trim(),
            "no ref at all",
            "git -C <worktree> for-each-ref --contains HEAD",
        )))
    }
}

impl Worktrees for Treehouse {
    fn acquire(&self, request: &AcquireRequest) -> Result<Lease, WorktreeError> {
        let args = vec![
            "get".to_owned(),
            "--lease".to_owned(),
            "--json".to_owned(),
            "--lease-holder".to_owned(),
            request.holder.clone(),
        ];
        let command = self.program.command_line(&args);
        let output = self
            .program
            .run_ok(&args, Some(&request.repo))
            .map_err(WorktreeError::Command)?;
        let value: Value = serde_json::from_str(output.stdout_trimmed()).map_err(|error| {
            WorktreeError::MalformedOutput {
                command: command.clone(),
                detail: error.to_string(),
            }
        })?;

        let lease = Lease {
            lease: WorktreeLease::new(field(&command, &value, "lease_id")?),
            path: PathBuf::from(field(&command, &value, "path")?),
            holder: value
                .get("lease_holder")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            acquired_at: field(&command, &value, "leased_at")?,
        };

        if let Baseline::PinnedCommit(commit) = &request.baseline {
            let branch = request.holder.replace(':', "-");
            let args = self.git_args(&lease.path, &["checkout", "-B", &branch, commit.as_str()]);
            if let Err(error) = self.git.run_ok(&args, None) {
                let _ = self.return_lease(&lease);
                return Err(error.into());
            }
        }

        Ok(lease)
    }

    fn release(&self, lease: &Lease) -> Result<(), WorktreeError> {
        if let Some(reason) = self.unlanded_work(&lease.path)? {
            return Err(WorktreeError::UnlandedWork {
                lease: lease.lease.as_str().to_owned(),
                path: lease.path.clone(),
                reason,
            });
        }

        self.return_lease(lease).map_err(WorktreeError::Command)?;
        Ok(())
    }

    fn pool(&self, repo: &Path) -> Result<Vec<PoolEntry>, WorktreeError> {
        let args = vec!["status".to_owned(), "--json".to_owned()];
        let command = self.program.command_line(&args);
        let output = self
            .program
            .run_ok(&args, Some(repo))
            .map_err(WorktreeError::Command)?;
        let value: Value = serde_json::from_str(output.stdout_trimmed()).map_err(|error| {
            WorktreeError::MalformedOutput {
                command: command.clone(),
                detail: error.to_string(),
            }
        })?;
        let entries = value
            .as_array()
            .ok_or_else(|| WorktreeError::MalformedOutput {
                command: command.clone(),
                detail: "the pool status is not a list of worktrees".to_owned(),
            })?;

        entries
            .iter()
            .map(|entry| {
                Ok(PoolEntry {
                    name: field(&command, entry, "name")?,
                    path: PathBuf::from(field(&command, entry, "path")?),
                    state: field(&command, entry, "status")?,
                    lease: entry
                        .get("lease_id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(WorktreeLease::new),
                    holder: entry
                        .get("lease_holder")
                        .and_then(Value::as_str)
                        .filter(|holder| !holder.is_empty())
                        .map(str::to_owned),
                })
            })
            .collect()
    }
}

fn non_empty_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn unreached_from(count: &str, target: &str, hint: &str) -> String {
    let (what, verb) = if count == "1" {
        ("commit", "is")
    } else {
        ("commits", "are")
    };
    format!("{count} {what} reachable from HEAD {verb} on {target} ({hint})")
}

fn field(command: &str, value: &Value, name: &str) -> Result<String, WorktreeError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| WorktreeError::MissingField {
            command: command.to_owned(),
            field: name.to_owned(),
        })
}

#[derive(Debug)]
pub enum WorktreeError {
    Command(ProcessError),
    MalformedOutput {
        command: String,
        detail: String,
    },
    MissingField {
        command: String,
        field: String,
    },
    UnlandedWork {
        lease: String,
        path: PathBuf,
        reason: String,
    },
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorktreeError::Command(error) => write!(f, "{error}"),
            WorktreeError::MalformedOutput { command, detail } => {
                write!(f, "{command} produced output depot cannot read: {detail}")
            }
            WorktreeError::MissingField { command, field } => {
                write!(f, "{command} reported no {field} field")
            }
            WorktreeError::UnlandedWork {
                lease,
                path,
                reason,
            } => write!(
                f,
                "refusing to release lease {lease} at {}: {reason}; a worktree holding unlanded work is never reset or removed",
                path.display()
            ),
        }
    }
}

impl std::error::Error for WorktreeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WorktreeError::Command(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProcessError> for WorktreeError {
    fn from(error: ProcessError) -> Self {
        WorktreeError::Command(error)
    }
}
