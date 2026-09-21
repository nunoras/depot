use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::adapters::process::Program;
use crate::error::{Error, Result};
use crate::home::DepotHome;

const DEPOT_ARTIFACT_PATH: &str = "DEPOT_ARTIFACT_PATH";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub path: PathBuf,
    pub url: String,
}

pub fn add_artifact(home: &DepotHome, file: &Path) -> Result<Staged> {
    let settings = home.load_settings()?;
    let command = settings.artifacts.publish_command.trim();
    if command.is_empty() {
        return Err(Error::Config(format!(
            "no artifact publish command is configured: add publish_command under [artifacts] in {}, for example\n\
             \n\
             [artifacts]\n\
             publish_command = \"my-uploader\"\n\
             \n\
             The command runs with DEPOT_ARTIFACT_PATH set to the staged file and must print exactly one http(s) URL.",
            home.config_path().display()
        )));
    }
    let staged = stage(home, file)?;
    let output = run_publisher(command, &staged)?;
    match exactly_one_url(&output.stdout) {
        Ok(url) => Ok(Staged { path: staged, url }),
        Err(reason) => Err(Error::Project(format!(
            "{reason}; the file is kept at {}",
            staged.display()
        ))),
    }
}

pub fn exactly_one_url(stdout: &str) -> std::result::Result<String, String> {
    let mut urls: Vec<String> = Vec::new();
    for token in stdout.split_whitespace() {
        if token.starts_with("http://") || token.starts_with("https://") {
            let token = token.to_string();
            if !urls.contains(&token) {
                urls.push(token);
            }
        }
    }
    match urls.len() {
        1 => Ok(urls.remove(0)),
        0 => Err("the publish command printed no http(s) URL".to_string()),
        count => Err(format!(
            "the publish command printed {count} different http(s) URLs: {}",
            urls.join(", ")
        )),
    }
}

fn stage(home: &DepotHome, file: &Path) -> Result<PathBuf> {
    let metadata = std::fs::metadata(file)
        .map_err(|_| Error::Project(format!("artifact `{}` does not exist", file.display())))?;
    if !metadata.is_file() {
        return Err(Error::Project(format!(
            "artifact `{}` is not a regular file",
            file.display()
        )));
    }
    let directory = home.artifacts_dir();
    std::fs::create_dir_all(&directory)?;
    let name = file
        .file_name()
        .ok_or_else(|| Error::Project(format!("artifact `{}` has no file name", file.display())))?;
    let target = collision_free(&directory, name);
    std::fs::copy(file, &target)?;
    Ok(target)
}

fn collision_free(directory: &Path, name: &OsStr) -> PathBuf {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "artifact".to_string());
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let mut suffix = 1;
    loop {
        let candidate = directory.join(format!("{stem}-{suffix}{extension}"));
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
}

fn run_publisher(command: &str, staged: &Path) -> Result<crate::adapters::process::Output> {
    let program = Program::new(shell()).with_env(DEPOT_ARTIFACT_PATH, staged.as_os_str());
    let output = program
        .run(&shell_arguments(command), None)
        .map_err(|error| {
            Error::Project(format!(
                "the artifact publish command could not run: {error}; the file is kept at {}",
                staged.display()
            ))
        })?;
    if output.succeeded() {
        return Ok(output);
    }
    Err(Error::Project(format!(
        "the artifact publish command exited {}: {}; the file is kept at {}",
        output
            .status
            .map(|code| code.to_string())
            .unwrap_or_else(|| "on a signal".to_string()),
        output_tail(&output),
        staged.display()
    )))
}

fn shell() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}

fn shell_arguments(command: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["/C".to_string(), command.to_string()]
    } else {
        vec!["-c".to_string(), command.to_string()]
    }
}

fn output_tail(output: &crate::adapters::process::Output) -> String {
    let mut combined = output.stderr.clone();
    if !output.stdout.trim().is_empty() {
        combined.push_str(output.stdout.trim());
    }
    let tail = combined.chars().rev().take(512).collect::<String>();
    tail.chars().rev().collect::<String>().trim().to_string()
}
