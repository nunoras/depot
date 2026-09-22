use std::io;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{EvidenceConfig, ValidationConfig};
use crate::error::{Error, Result};

pub const PROJECT_FILE_PATH: &str = ".agni/project.toml";
pub const PROJECT_FILE_KEYS: &str = "`base_branch`, `[validation] command`, `[evidence]`, `[pull_request] describe_style`, `[dispatch]` and `[delivery]`";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectFile {
    pub base_branch: String,
    pub validation: ValidationConfig,
    pub evidence: EvidenceConfig,
    pub pull_request: ProjectFilePullRequest,
    pub dispatch: Option<toml::Value>,
    pub delivery: Option<DeliveryConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectFilePullRequest {
    pub describe_style: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliveryConfig {
    pub mode: Option<String>,
}

impl Default for ProjectFile {
    fn default() -> Self {
        Self {
            base_branch: "main".to_string(),
            validation: ValidationConfig::default(),
            evidence: EvidenceConfig::default(),
            pull_request: ProjectFilePullRequest::default(),
            dispatch: None,
            delivery: None,
        }
    }
}

impl ProjectFile {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn load_optional(directory: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(directory.join(PROJECT_FILE_PATH)) {
            Ok(text) => Ok(Some(Self::parse(&text)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn missing_message() -> String {
        format!("the base branch has no `{PROJECT_FILE_PATH}`; commit it with {PROJECT_FILE_KEYS}")
    }

    pub fn read_for_base(repo: &Path) -> Result<Option<Self>> {
        let default = default_branch(repo)?;
        fetch_branch(repo, &default)?;
        let Some(at_default) = Self::at_ref(repo, &format!("refs/remotes/origin/{default}"))?
        else {
            return Ok(None);
        };
        let base = at_default.base_branch.trim().to_owned();
        if base.is_empty() {
            return Err(Error::Config(format!(
                "`{PROJECT_FILE_PATH}` at `origin/{default}` names no base_branch"
            )));
        }
        if base == default {
            return Ok(Some(at_default));
        }
        fetch_branch(repo, &base)?;
        let at_base = Self::at_ref(repo, &format!("refs/remotes/origin/{base}"))?
            .ok_or_else(|| Error::Config(Self::missing_message()))?;
        let named = at_base.base_branch.trim().to_owned();
        if named != base {
            return Err(Error::Config(format!(
                "`{PROJECT_FILE_PATH}` names base_branch `{named}` on `origin/{base}` but `{base}` on `origin/{default}`; refusing to follow the disagreement"
            )));
        }
        Ok(Some(at_base))
    }

    fn at_ref(repo: &Path, reference: &str) -> Result<Option<Self>> {
        let spec = format!("{reference}:{PROJECT_FILE_PATH}");
        if git_exit(repo, &["cat-file", "-e", &spec])? != 0 {
            return Ok(None);
        }
        let text = git(repo, &["show", &spec])?;
        Ok(Some(Self::parse(&text)?))
    }
}

fn default_branch(repo: &Path) -> Result<String> {
    if let Ok(branch) = symbolic_default(repo) {
        return Ok(branch);
    }
    git(repo, &["remote", "set-head", "origin", "-a"]).map_err(|error| {
        Error::Project(format!(
            "could not resolve the default branch of the origin remote in {}: {error}",
            repo.display()
        ))
    })?;
    symbolic_default(repo).map_err(|error| {
        Error::Project(format!(
            "could not resolve the default branch of the origin remote in {}: {error}",
            repo.display()
        ))
    })
}

fn symbolic_default(repo: &Path) -> Result<String> {
    let full = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )?;
    full.trim()
        .strip_prefix("origin/")
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::Project("origin/HEAD points nowhere".to_string()))
}

fn fetch_branch(repo: &Path, branch: &str) -> Result<()> {
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    git(repo, &["fetch", "origin", &refspec])
        .map_err(|error| Error::Project(format!("could not fetch base `{branch}`: {error}")))?;
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(Error::Io)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(Error::Project(
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    ))
}

fn git_exit(repo: &Path, args: &[&str]) -> Result<i32> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(Error::Io)?;
    Ok(output.status.code().unwrap_or(-1))
}
