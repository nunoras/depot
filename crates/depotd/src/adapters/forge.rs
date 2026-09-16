use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use depot_core::{Checks, CommitId};
use serde_json::{Value, json};

use crate::adapters::process::Program;

pub const DEFAULT_API_BASE: &str = "https://api.github.com";
pub const TOKEN_FILE: &str = "github-token";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSlug {
    pub owner: String,
    pub name: String,
}

impl RepoSlug {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }

    pub fn path(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for RepoSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: PrState,
    pub checks: Checks,
    pub head: CommitId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPullRequest {
    pub repo: RepoSlug,
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedPullRequest {
    pub number: u64,
    pub url: String,
}

pub trait Forge {
    fn pull_request(&self, repo: &RepoSlug, number: u64) -> Result<PullRequest, ForgeError>;
    fn open_pull_request(&self, request: &NewPullRequest) -> Result<OpenedPullRequest, ForgeError>;
}

#[derive(Debug, Clone)]
pub struct GitHub {
    api: String,
    token: String,
    agent: ureq::Agent,
}

impl GitHub {
    pub fn new(api: impl Into<String>, token: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self {
            api: api.into().trim_end_matches('/').to_owned(),
            token: token.into(),
            agent,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }

    fn with_headers<T>(&self, request: ureq::RequestBuilder<T>) -> ureq::RequestBuilder<T> {
        request
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "depot")
            .header("Content-Type", "application/json")
    }

    fn call(
        &self,
        method: &str,
        url: &str,
        body: Option<String>,
    ) -> Result<(u16, String), ForgeError> {
        let invalid_method = || ForgeError::Malformed {
            url: url.to_owned(),
            detail: format!("{method} is not a method depot sends"),
        };

        let mut response = match (method, body) {
            ("GET", None) => self.with_headers(self.agent.get(url)).call(),
            ("POST", Some(body)) => self.with_headers(self.agent.post(url)).send(body),
            _ => return Err(invalid_method()),
        }
        .map_err(|error| ForgeError::Request {
            url: url.to_owned(),
            detail: error.to_string(),
        })?;

        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| ForgeError::Request {
                url: url.to_owned(),
                detail: error.to_string(),
            })?;
        Ok((status, body))
    }

    fn read(&self, url: &str) -> Result<String, ForgeError> {
        let (status, body) = self.call("GET", url, None)?;
        match status {
            200 => Ok(body),
            401 | 403 => Err(ForgeError::Unauthorized {
                url: url.to_owned(),
            }),
            404 => Err(ForgeError::NotFound {
                url: url.to_owned(),
            }),
            status => Err(ForgeError::Status {
                url: url.to_owned(),
                status,
                body: body.trim().to_owned(),
            }),
        }
    }

    fn checks(&self, repo: &RepoSlug, head: &CommitId) -> Result<Checks, ForgeError> {
        let url = self.url(&format!(
            "/repos/{}/commits/{}/check-runs?per_page=100",
            repo.path(),
            head.as_str()
        ));
        let body = self.read(&url)?;
        let value: Value = serde_json::from_str(&body).map_err(|error| ForgeError::Malformed {
            url: url.clone(),
            detail: error.to_string(),
        })?;
        let runs = value
            .get("check_runs")
            .and_then(Value::as_array)
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no check_runs list in {}", truncated(&body)),
            })?;

        if runs.is_empty() {
            return Ok(Checks::Unknown);
        }

        let mut failing = false;
        for run in runs {
            let status =
                run.get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ForgeError::Malformed {
                        url: url.clone(),
                        detail: format!("a check run has no status in {}", truncated(&body)),
                    })?;
            if status != "completed" {
                return Ok(Checks::Pending);
            }
            let conclusion = run
                .get("conclusion")
                .and_then(Value::as_str)
                .ok_or_else(|| ForgeError::Malformed {
                    url: url.clone(),
                    detail: format!(
                        "a completed check run has no conclusion in {}",
                        truncated(&body)
                    ),
                })?;
            match conclusion {
                "success" | "neutral" | "skipped" => {}
                "failure" | "timed_out" | "cancelled" | "action_required" | "stale"
                | "startup_failure" => failing = true,
                other => {
                    return Err(ForgeError::Malformed {
                        url: url.clone(),
                        detail: format!("{other:?} is not a check conclusion depot knows"),
                    });
                }
            }
        }
        Ok(if failing {
            Checks::Failing
        } else {
            Checks::Passing
        })
    }
}

impl Forge for GitHub {
    fn pull_request(&self, repo: &RepoSlug, number: u64) -> Result<PullRequest, ForgeError> {
        let url = self.url(&format!("/repos/{}/pulls/{number}", repo.path()));
        let body = self.read(&url)?;
        let value: Value = serde_json::from_str(&body).map_err(|error| ForgeError::Malformed {
            url: url.clone(),
            detail: error.to_string(),
        })?;

        let number =
            value
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| ForgeError::Malformed {
                    url: url.clone(),
                    detail: format!("no pull request number in {}", truncated(&body)),
                })?;
        let html_url = string(&url, &value, "html_url", &body)?;
        let state = string(&url, &value, "state", &body)?;
        let head = value
            .get("head")
            .and_then(|head| head.get("sha"))
            .and_then(Value::as_str)
            .filter(|sha| !sha.is_empty())
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no head commit in {}", truncated(&body)),
            })?;

        let state = match state.as_str() {
            "open" => PrState::Open,
            "closed" => {
                let merged = value
                    .get("merged")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| ForgeError::Malformed {
                        url: url.clone(),
                        detail: format!(
                            "the pull request is closed but carries no merged flag in {}",
                            truncated(&body)
                        ),
                    })?;
                if merged {
                    PrState::Merged
                } else {
                    PrState::Closed
                }
            }
            other => {
                return Err(ForgeError::Malformed {
                    url: url.clone(),
                    detail: format!("{other:?} is not a pull request state depot knows"),
                });
            }
        };

        let checks = self.checks(repo, &CommitId::new(head))?;
        Ok(PullRequest {
            number,
            url: html_url,
            state,
            checks,
            head: CommitId::new(head),
        })
    }

    fn open_pull_request(&self, request: &NewPullRequest) -> Result<OpenedPullRequest, ForgeError> {
        let url = self.url(&format!("/repos/{}/pulls", request.repo.path()));
        let body = json!({
            "title": &request.title,
            "body": &request.body,
            "head": &request.head,
            "base": &request.base,
        })
        .to_string();
        let (status, response) = self.call("POST", &url, Some(body))?;
        match status {
            201 => {}
            401 | 403 => return Err(ForgeError::Unauthorized { url }),
            404 => return Err(ForgeError::NotFound { url }),
            status => {
                return Err(ForgeError::Status {
                    url,
                    status,
                    body: response.trim().to_owned(),
                });
            }
        }

        let value: Value =
            serde_json::from_str(&response).map_err(|error| ForgeError::Malformed {
                url: url.clone(),
                detail: error.to_string(),
            })?;
        let number =
            value
                .get("number")
                .and_then(Value::as_u64)
                .ok_or_else(|| ForgeError::Malformed {
                    url: url.clone(),
                    detail: format!("no pull request number in {}", truncated(&response)),
                })?;
        Ok(OpenedPullRequest {
            number,
            url: string(&url, &value, "html_url", &response)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    GhCli,
    TokenFile(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub token: String,
    pub source: CredentialSource,
}

pub fn resolve_credentials(gh: &Program, depot_home: &Path) -> Result<Credentials, ForgeError> {
    let args = vec!["auth".to_owned(), "token".to_owned()];
    if let Ok(output) = gh.run(&args, None)
        && output.succeeded()
    {
        let token = output.stdout_trimmed();
        if !token.is_empty() {
            return Ok(Credentials {
                token: token.to_owned(),
                source: CredentialSource::GhCli,
            });
        }
    }

    let path = depot_home.join(TOKEN_FILE);
    if !path.exists() {
        return Err(ForgeError::NoCredential { path });
    }
    check_owner_only(&path)?;
    let token = fs::read_to_string(&path).map_err(|error| ForgeError::Request {
        url: path.display().to_string(),
        detail: error.to_string(),
    })?;
    if token.trim().is_empty() {
        return Err(ForgeError::EmptyToken { path });
    }
    Ok(Credentials {
        token: token.trim().to_owned(),
        source: CredentialSource::TokenFile(path),
    })
}

#[cfg(unix)]
fn check_owner_only(path: &Path) -> Result<(), ForgeError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path).map_err(|error| ForgeError::Request {
        url: path.display().to_string(),
        detail: error.to_string(),
    })?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(ForgeError::InsecureTokenFile {
            path: path.to_owned(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_owner_only(_path: &Path) -> Result<(), ForgeError> {
    Ok(())
}

fn string(url: &str, value: &Value, name: &str, body: &str) -> Result<String, ForgeError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ForgeError::Malformed {
            url: url.to_owned(),
            detail: format!("no {name} in {}", truncated(body)),
        })
}

fn truncated(body: &str) -> String {
    let body = body.trim();
    if body.chars().count() <= 200 {
        return body.to_owned();
    }
    format!("{}...", body.chars().take(200).collect::<String>())
}

#[derive(Debug)]
pub enum ForgeError {
    NoCredential {
        path: PathBuf,
    },
    InsecureTokenFile {
        path: PathBuf,
        mode: u32,
    },
    EmptyToken {
        path: PathBuf,
    },
    Request {
        url: String,
        detail: String,
    },
    Unauthorized {
        url: String,
    },
    NotFound {
        url: String,
    },
    Status {
        url: String,
        status: u16,
        body: String,
    },
    Malformed {
        url: String,
        detail: String,
    },
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeError::NoCredential { path } => write!(
                f,
                "no GitHub credential: `gh auth token` is not authenticated and {} does not exist; depot reads credentials from the authenticated gh CLI or from that owner-only file in the depot home, never from a project",
                path.display()
            ),
            ForgeError::InsecureTokenFile { path, mode } => write!(
                f,
                "{} is readable beyond its owner (mode {mode:o}); make it owner-only before depot reads a token from it",
                path.display()
            ),
            ForgeError::EmptyToken { path } => {
                write!(f, "{} holds no token", path.display())
            }
            ForgeError::Request { url, detail } => {
                write!(f, "the request to {url} failed: {detail}")
            }
            ForgeError::Unauthorized { url } => write!(
                f,
                "GitHub refused the credentials depot sent to {url}; the token is missing, expired or lacking the scope this call needs"
            ),
            ForgeError::NotFound { url } => write!(f, "GitHub has nothing at {url}"),
            ForgeError::Status { url, status, body } => {
                write!(f, "GitHub answered {status} for {url}: {body}")
            }
            ForgeError::Malformed { url, detail } => {
                write!(f, "the answer from {url} is not what depot reads: {detail}")
            }
        }
    }
}

impl std::error::Error for ForgeError {}
