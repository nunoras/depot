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
    pub title: String,
    pub body: Option<String>,
    pub state: PrState,
    pub checks: Checks,
    pub head: CommitId,
    pub head_ref: String,
    pub base: CommitId,
    pub mergeable: Option<bool>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAssetUpload {
    pub path: PathBuf,
    pub content_type: String,
}

pub trait Forge {
    fn pull_request(&self, repo: &RepoSlug, number: u64) -> Result<PullRequest, ForgeError>;
    fn pull_request_text(
        &self,
        repo: &RepoSlug,
        number: u64,
    ) -> Result<(String, Option<String>), ForgeError>;
    fn find_open_pull_request(
        &self,
        repo: &RepoSlug,
        head: &str,
    ) -> Result<Option<OpenedPullRequest>, ForgeError>;
    fn open_pull_request(&self, request: &NewPullRequest) -> Result<OpenedPullRequest, ForgeError>;
    fn update_pull_request(
        &self,
        repo: &RepoSlug,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<(), ForgeError>;
    fn upload_user_asset(&self, upload: &UserAssetUpload) -> Result<String, ForgeError>;
    fn merge_pull_request(
        &self,
        repo: &RepoSlug,
        number: u64,
        head: &CommitId,
    ) -> Result<(), ForgeError>;
    fn delete_branch(&self, repo: &RepoSlug, branch: &str) -> Result<(), ForgeError>;
}

pub fn content_type_for(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_owned()
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

    fn web_url(&self, path: &str) -> String {
        format!("{}{path}", self.web_base())
    }

    fn web_base(&self) -> String {
        let base = self.api.trim_end_matches('/');
        if let Some(enterprise) = base.strip_suffix("/api/v3") {
            return enterprise.to_owned();
        }
        base.replacen("//api.github.com", "//github.com", 1)
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
            ("PUT", Some(body)) => self.with_headers(self.agent.put(url)).send(body),
            ("PATCH", Some(body)) => self.with_headers(self.agent.patch(url)).send(body),
            ("DELETE", None) => self.with_headers(self.agent.delete(url)).call(),
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

    fn check_run_page(
        &self,
        repo: &RepoSlug,
        head: &CommitId,
        page: u32,
    ) -> Result<(String, String, u64, Vec<Value>), ForgeError> {
        let url = self.url(&format!(
            "/repos/{}/commits/{}/check-runs?per_page=100&page={page}",
            repo.path(),
            head.as_str()
        ));
        let body = self.read(&url)?;
        let value: Value = serde_json::from_str(&body).map_err(|error| ForgeError::Malformed {
            url: url.clone(),
            detail: error.to_string(),
        })?;
        let total_count = value
            .get("total_count")
            .and_then(Value::as_u64)
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no total_count in {}", truncated(&body)),
            })?;
        let runs = value
            .get("check_runs")
            .and_then(Value::as_array)
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no check_runs list in {}", truncated(&body)),
            })?
            .clone();
        Ok((url, body, total_count, runs))
    }

    fn checks(&self, repo: &RepoSlug, head: &CommitId) -> Result<Checks, ForgeError> {
        let mut runs = Vec::new();
        let (mut last_url, mut last_body, mut total, page_runs) =
            self.check_run_page(repo, head, 1)?;
        let mut page_len = page_runs.len();
        runs.extend(page_runs);
        let mut page = 1_u32;

        while (runs.len() as u64) < total && page_len == 100 {
            page = page.checked_add(1).ok_or_else(|| ForgeError::Malformed {
                url: last_url.clone(),
                detail: "check-runs pagination overflowed".to_owned(),
            })?;
            if page > 100 {
                return Err(ForgeError::Malformed {
                    url: last_url,
                    detail: format!(
                        "check-runs listed {total} runs but pagination stopped after {}",
                        runs.len()
                    ),
                });
            }

            let (url, body, page_total, page_runs) = self.check_run_page(repo, head, page)?;
            if page_total != total {
                return Err(ForgeError::Malformed {
                    url: url.clone(),
                    detail: format!(
                        "check-runs total_count changed from {total} to {page_total} across pages"
                    ),
                });
            }
            page_len = page_runs.len();
            runs.extend(page_runs);
            last_url = url;
            last_body = body;
            total = page_total;
        }

        if (runs.len() as u64) < total {
            return Err(ForgeError::Malformed {
                url: last_url,
                detail: format!(
                    "check-runs listed {total} runs but only returned {}; a truncated list is never treated as passing",
                    runs.len()
                ),
            });
        }

        if runs.is_empty() {
            return Ok(Checks::None);
        }

        let mut failing = false;
        for run in &runs {
            let status =
                run.get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ForgeError::Malformed {
                        url: last_url.clone(),
                        detail: format!("a check run has no status in {}", truncated(&last_body)),
                    })?;
            if status != "completed" {
                return Ok(Checks::Pending);
            }
            let conclusion = run
                .get("conclusion")
                .and_then(Value::as_str)
                .ok_or_else(|| ForgeError::Malformed {
                    url: last_url.clone(),
                    detail: format!(
                        "a completed check run has no conclusion in {}",
                        truncated(&last_body)
                    ),
                })?;
            match conclusion {
                "success" | "neutral" | "skipped" => {}
                "failure" | "timed_out" | "cancelled" | "action_required" | "stale"
                | "startup_failure" => failing = true,
                other => {
                    return Err(ForgeError::Malformed {
                        url: last_url.clone(),
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
        let title = string(&url, &value, "title", &body)?;
        let body_text = value.get("body").and_then(Value::as_str).map(str::to_owned);
        let head = value
            .get("head")
            .and_then(|head| head.get("sha"))
            .and_then(Value::as_str)
            .filter(|sha| !sha.is_empty())
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no head commit in {}", truncated(&body)),
            })?;
        let base = value
            .get("base")
            .and_then(|base| base.get("sha"))
            .and_then(Value::as_str)
            .filter(|sha| !sha.is_empty())
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no base commit in {}", truncated(&body)),
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

        let head_ref = value
            .get("head")
            .and_then(|head| head.get("ref"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mergeable = value.get("mergeable").and_then(Value::as_bool);
        let checks = self.checks(repo, &CommitId::new(head))?;
        Ok(PullRequest {
            number,
            url: html_url,
            title,
            body: body_text,
            state,
            checks,
            head: CommitId::new(head),
            head_ref,
            base: CommitId::new(base),
            mergeable,
        })
    }

    fn pull_request_text(
        &self,
        repo: &RepoSlug,
        number: u64,
    ) -> Result<(String, Option<String>), ForgeError> {
        let url = self.url(&format!("/repos/{}/pulls/{number}", repo.path()));
        let body = self.read(&url)?;
        let value: Value = serde_json::from_str(&body).map_err(|error| ForgeError::Malformed {
            url: url.clone(),
            detail: error.to_string(),
        })?;
        let title = string(&url, &value, "title", &body)?;
        let text = value.get("body").and_then(Value::as_str).map(str::to_owned);
        Ok((title, text))
    }

    fn find_open_pull_request(
        &self,
        repo: &RepoSlug,
        head: &str,
    ) -> Result<Option<OpenedPullRequest>, ForgeError> {
        let url = self.url(&format!(
            "/repos/{}/pulls?state=open&head={}%3A{}",
            repo.path(),
            repo.owner,
            head
        ));
        let body = self.read(&url)?;
        let value: Value = serde_json::from_str(&body).map_err(|error| ForgeError::Malformed {
            url: url.clone(),
            detail: error.to_string(),
        })?;
        let Some(pull_request) = value.as_array().and_then(|items| items.first()) else {
            return Ok(None);
        };
        let number = pull_request
            .get("number")
            .and_then(Value::as_u64)
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no pull request number in {}", truncated(&body)),
            })?;
        Ok(Some(OpenedPullRequest {
            number,
            url: string(&url, pull_request, "html_url", &body)?,
        }))
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

    fn update_pull_request(
        &self,
        repo: &RepoSlug,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        let url = self.url(&format!("/repos/{}/pulls/{number}", repo.path()));
        let payload = json!({ "title": title, "body": body }).to_string();
        let (status, response) = self.call("PATCH", &url, Some(payload))?;
        match status {
            200 => Ok(()),
            401 | 403 => Err(ForgeError::Unauthorized { url }),
            404 => Err(ForgeError::NotFound { url }),
            status => Err(ForgeError::Status {
                url,
                status,
                body: response.trim().to_owned(),
            }),
        }
    }

    fn upload_user_asset(&self, upload: &UserAssetUpload) -> Result<String, ForgeError> {
        let bytes = fs::read(&upload.path).map_err(|error| ForgeError::Request {
            url: upload.path.display().to_string(),
            detail: error.to_string(),
        })?;
        let name = upload
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ForgeError::Request {
                url: upload.path.display().to_string(),
                detail: "the evidence file has no usable name".to_owned(),
            })?
            .to_owned();
        let policy = self.user_asset_policy(&name, bytes.len(), &upload.content_type)?;
        self.post_user_asset(&policy, &name, &upload.content_type, &bytes)?;
        Ok(policy.asset_url)
    }

    fn merge_pull_request(
        &self,
        repo: &RepoSlug,
        number: u64,
        head: &CommitId,
    ) -> Result<(), ForgeError> {
        let url = self.url(&format!("/repos/{}/pulls/{number}/merge", repo.path()));
        let body = json!({
            "sha": head.as_str(),
            "merge_method": "squash",
        })
        .to_string();
        let (status, response) = self.call("PUT", &url, Some(body))?;
        match status {
            200 => Ok(()),
            401 | 403 => Err(ForgeError::Unauthorized { url }),
            404 => Err(ForgeError::NotFound { url }),
            status => Err(ForgeError::Refused {
                url,
                status,
                body: response.trim().to_owned(),
            }),
        }
    }

    fn delete_branch(&self, repo: &RepoSlug, branch: &str) -> Result<(), ForgeError> {
        let url = self.url(&format!("/repos/{}/git/refs/heads/{branch}", repo.path()));
        let (status, response) = self.call("DELETE", &url, None)?;
        match status {
            204 => Ok(()),
            401 | 403 => Err(ForgeError::Unauthorized { url }),
            404 => Err(ForgeError::NotFound { url }),
            status => Err(ForgeError::Refused {
                url,
                status,
                body: response.trim().to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
struct UserAssetPolicy {
    upload_url: String,
    asset_url: String,
    form: Vec<(String, String)>,
    header: Vec<(String, String)>,
}

impl GitHub {
    fn user_asset_policy(
        &self,
        name: &str,
        size: usize,
        content_type: &str,
    ) -> Result<UserAssetPolicy, ForgeError> {
        let url = self.web_url("/upload/policies/assets");
        let payload =
            json!({ "name": name, "size": size, "content_type": content_type }).to_string();
        let (status, response) = self.call("POST", &url, Some(payload))?;
        if status != 200 {
            return Err(ForgeError::UploadUnavailable {
                url,
                status,
                body: truncated(&response),
            });
        }
        let value: Value =
            serde_json::from_str(&response).map_err(|_| ForgeError::UploadUnavailable {
                url: url.clone(),
                status,
                body: truncated(&response),
            })?;
        let upload_url = string(&url, &value, "upload_url", &response)?;
        let asset = value
            .get("asset")
            .and_then(Value::as_object)
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no asset in {}", truncated(&response)),
            })?;
        let asset_url = asset
            .get("href")
            .or_else(|| asset.get("url"))
            .and_then(Value::as_str)
            .filter(|href| !href.is_empty())
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no asset url in {}", truncated(&response)),
            })?
            .to_owned();
        let asset_content_type = asset
            .get("content_type")
            .and_then(Value::as_str)
            .ok_or_else(|| ForgeError::Malformed {
                url: url.clone(),
                detail: format!("no asset content type in {}", truncated(&response)),
            })?;
        if asset_content_type != content_type {
            return Err(ForgeError::Malformed {
                url,
                detail: format!(
                    "the upload policy returned content type {asset_content_type:?} for a {content_type:?} upload"
                ),
            });
        }
        if !asset_url.starts_with("http") {
            return Err(ForgeError::Malformed {
                url,
                detail: format!("the upload policy returned a relative asset url {asset_url:?}"),
            });
        }
        Ok(UserAssetPolicy {
            upload_url,
            asset_url,
            form: string_map(&value, "form", &url)?,
            header: string_map(&value, "header", &url).unwrap_or_default(),
        })
    }

    fn post_user_asset(
        &self,
        policy: &UserAssetPolicy,
        name: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<(), ForgeError> {
        let (boundary, body) = multipart_body(&policy.form, name, content_type, bytes);
        let mut request = self.agent.post(&policy.upload_url);
        for (header, value) in &policy.header {
            request = request.header(header, value);
        }
        let response = request
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send(body)
            .map_err(|error| ForgeError::Request {
                url: policy.upload_url.clone(),
                detail: error.to_string(),
            })?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(());
        }
        Err(ForgeError::Status {
            url: policy.upload_url.clone(),
            status,
            body: String::new(),
        })
    }
}

fn string_map(value: &Value, name: &str, url: &str) -> Result<Vec<(String, String)>, ForgeError> {
    let Some(map) = value.get(name).and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    map.iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.to_owned()))
                .ok_or_else(|| ForgeError::Malformed {
                    url: url.to_owned(),
                    detail: format!("the {name} field {key:?} is not a string"),
                })
        })
        .collect()
}

fn multipart_body(
    fields: &[(String, String)],
    file_name: &str,
    content_type: &str,
    bytes: &[u8],
) -> (String, Vec<u8>) {
    let boundary = format!("depot-boundary-{}", std::process::id());
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
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
pub(crate) fn check_owner_only(path: &Path) -> Result<(), ForgeError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path).map_err(|error| ForgeError::Request {
        url: path.display().to_string(),
        detail: error.to_string(),
    })?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(ForgeError::InsecureTokenFile {
            path: path.to_owned(),
            detail: format!("mode {mode:o}"),
        });
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn check_owner_only(path: &Path) -> Result<(), ForgeError> {
    match windows_token_acl::allow_identities(path) {
        Ok(identities) => {
            if token_acl_is_restricted(&identities) {
                Ok(())
            } else {
                Err(ForgeError::InsecureTokenFile {
                    path: path.to_owned(),
                    detail: "DACL grants access outside the current user, the owner, Administrators and SYSTEM".to_owned(),
                })
            }
        }
        Err(detail) => Err(ForgeError::InsecureTokenFile {
            path: path.to_owned(),
            detail,
        }),
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn check_owner_only(path: &Path) -> Result<(), ForgeError> {
    Err(ForgeError::InsecureTokenFile {
        path: path.to_owned(),
        detail: "this platform cannot prove the token file ACL is restricted".to_owned(),
    })
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenAclIdentity {
    CurrentUser,
    Owner,
    Administrators,
    System,
    Other,
}

#[cfg(any(windows, test))]
fn token_acl_is_restricted(allow_identities: &[TokenAclIdentity]) -> bool {
    allow_identities
        .iter()
        .all(|identity| *identity != TokenAclIdentity::Other)
}

#[cfg(windows)]
mod windows_token_acl {
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree, PSID};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CopySid, CreateWellKnownSid,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid, GetTokenInformation, IsValidSid,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER, TokenUser,
        WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::TokenAclIdentity;

    const SECURITY_MAX_SID_SIZE: usize = 68;

    pub(super) fn allow_identities(path: &Path) -> Result<Vec<TokenAclIdentity>, String> {
        let wide = path_to_wide(path)?;
        let mut owner: PSID = ptr::null_mut();
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut security: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut security,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "GetNamedSecurityInfo failed with status {status}; the file DACL could not be verified"
            ));
        }
        struct FreeDesc(PSECURITY_DESCRIPTOR);
        impl Drop for FreeDesc {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe {
                        LocalFree(self.0 as _);
                    }
                }
            }
        }
        let _guard = FreeDesc(security);

        if dacl.is_null() {
            return Err("the file has a NULL DACL, which grants access to everyone".to_owned());
        }

        let current_user = current_user_sid()?;
        let administrators = well_known_sid(WinBuiltinAdministratorsSid)?;
        let system = well_known_sid(WinLocalSystemSid)?;

        let ace_count = unsafe { (*dacl).AceCount } as u32;
        let mut identities = Vec::new();
        for index in 0..ace_count {
            let mut ace: *mut core::ffi::c_void = ptr::null_mut();
            let ok = unsafe { GetAce(dacl, index, &mut ace) };
            if ok == 0 || ace.is_null() {
                return Err(format!(
                    "GetAce failed for index {index}; the file DACL could not be verified"
                ));
            }
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8 {
                continue;
            }
            let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
            if allowed.Mask == 0 {
                continue;
            }
            let sid = unsafe { ptr::addr_of!((*allowed).SidStart) as PSID };
            if unsafe { IsValidSid(sid) } == 0 {
                return Err("an allow ACE carried an invalid SID".to_owned());
            }
            identities.push(classify_sid(
                sid,
                owner,
                current_user.as_ptr(),
                administrators.as_ptr(),
                system.as_ptr(),
            ));
        }
        Ok(identities)
    }

    fn classify_sid(
        sid: PSID,
        owner: PSID,
        current_user: PSID,
        administrators: PSID,
        system: PSID,
    ) -> TokenAclIdentity {
        unsafe {
            if !owner.is_null() && EqualSid(sid, owner) != 0 {
                return TokenAclIdentity::Owner;
            }
            if EqualSid(sid, current_user) != 0 {
                return TokenAclIdentity::CurrentUser;
            }
            if EqualSid(sid, administrators) != 0 {
                return TokenAclIdentity::Administrators;
            }
            if EqualSid(sid, system) != 0 {
                return TokenAclIdentity::System;
            }
        }
        TokenAclIdentity::Other
    }

    struct SidBuffer {
        bytes: Vec<u8>,
    }

    impl SidBuffer {
        fn as_ptr(&self) -> PSID {
            self.bytes.as_ptr() as PSID
        }
    }

    fn well_known_sid(kind: i32) -> Result<SidBuffer, String> {
        let mut size = SECURITY_MAX_SID_SIZE as u32;
        let mut bytes = vec![0_u8; SECURITY_MAX_SID_SIZE];
        let ok = unsafe {
            CreateWellKnownSid(kind, ptr::null_mut(), bytes.as_mut_ptr() as PSID, &mut size)
        };
        if ok == 0 {
            return Err(format!(
                "CreateWellKnownSid({kind}) failed; the file DACL could not be verified"
            ));
        }
        bytes.truncate(size as usize);
        Ok(SidBuffer { bytes })
    }

    fn current_user_sid() -> Result<SidBuffer, String> {
        let mut token: HANDLE = ptr::null_mut();
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 {
            return Err("OpenProcessToken failed; the file DACL could not be verified".to_owned());
        }
        struct Close(HANDLE);
        impl Drop for Close {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe {
                        CloseHandle(self.0);
                    }
                }
            }
        }
        let _token = Close(token);

        let mut needed = 0_u32;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(
                "GetTokenInformation could not size the current user SID; the file DACL could not be verified"
                    .to_owned(),
            );
        }
        let mut buffer = vec![0_u8; needed as usize];
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr() as *mut _,
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err(
                "GetTokenInformation failed; the file DACL could not be verified".to_owned(),
            );
        }
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let sid = user.User.Sid;
        if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
            return Err("the current user SID is invalid".to_owned());
        }
        let length = unsafe { GetLengthSid(sid) } as usize;
        let mut bytes = vec![0_u8; length];
        let ok = unsafe { CopySid(length as u32, bytes.as_mut_ptr() as PSID, sid) };
        if ok == 0 {
            return Err("CopySid failed; the file DACL could not be verified".to_owned());
        }
        Ok(SidBuffer { bytes })
    }

    fn path_to_wide(path: &Path) -> Result<Vec<u16>, String> {
        use std::os::windows::ffi::OsStrExt;

        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err("the token path contains an interior NUL".to_owned());
        }
        wide.push(0);
        Ok(wide)
    }
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
        detail: String,
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
    Refused {
        url: String,
        status: u16,
        body: String,
    },
    Malformed {
        url: String,
        detail: String,
    },
    UploadUnavailable {
        url: String,
        status: u16,
        body: String,
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
            ForgeError::InsecureTokenFile { path, detail } => write!(
                f,
                "{} is readable beyond its owner ({detail}); authenticate `gh` or provide the token through the environment when the ACL cannot be proven to restrict access, and only then use an owner-only file in the depot home",
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
            ForgeError::Refused { url, status, body } => write!(
                f,
                "GitHub refused to merge {url} and answered {status}: {body}; the pull request is not in a state depot could merge"
            ),
            ForgeError::Malformed { url, detail } => {
                write!(f, "the answer from {url} is not what depot reads: {detail}")
            }
            ForgeError::UploadUnavailable { url, status, body } => write!(
                f,
                "GitHub answered {status} for the user-attachments upload endpoint {url}: {body}; that endpoint belongs to the github.com web application and needs a signed-in browser session, and the API token depot reads from the gh CLI or the depot home is not accepted there, so a local evidence file cannot be uploaded and must be published as a URL instead"
            ),
        }
    }
}

impl std::error::Error for ForgeError {}

#[cfg(test)]
mod tests {
    use super::{TokenAclIdentity, token_acl_is_restricted};

    #[test]
    fn accepts_only_the_owner_set() {
        assert!(token_acl_is_restricted(&[
            TokenAclIdentity::CurrentUser,
            TokenAclIdentity::Owner,
            TokenAclIdentity::Administrators,
            TokenAclIdentity::System,
        ]));
        assert!(token_acl_is_restricted(&[]));
    }

    #[test]
    fn refuses_any_other_allow_ace() {
        assert!(!token_acl_is_restricted(&[
            TokenAclIdentity::CurrentUser,
            TokenAclIdentity::Other,
        ]));
    }
}
