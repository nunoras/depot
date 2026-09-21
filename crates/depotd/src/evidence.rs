use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::{Error, Result};

pub const EVIDENCE_MARKER: &str = "<!-- depot-evidence -->";

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceArtifact {
    pub url: String,
    pub caption: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceOutput {
    pub exit_code: i32,
    pub stdout: String,
}

pub fn parse_manifest(stdout: &str) -> Vec<EvidenceArtifact> {
    stdout
        .lines()
        .filter_map(|line| {
            let (url, caption) = line.trim().split_once('\t')?;
            let url = url.trim();
            if url.is_empty() {
                return None;
            }
            Some(EvidenceArtifact {
                url: url.to_owned(),
                caption: caption.trim().to_owned(),
            })
        })
        .collect()
}

pub fn is_image(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('.').next().is_some_and(|extension| {
        IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

pub fn comment_body(artifacts: &[EvidenceArtifact]) -> String {
    let mut body = String::from(EVIDENCE_MARKER);
    body.push_str("\n### Task evidence\n\n");
    if artifacts.is_empty() {
        body.push_str("The evidence command ran but captured nothing for this commit.\n");
        return body;
    }
    for artifact in artifacts {
        if is_image(&artifact.url) {
            let caption = if artifact.caption.is_empty() {
                "evidence"
            } else {
                &artifact.caption
            };
            body.push_str(&format!("![{caption}]({0})\n\n", artifact.url));
        } else {
            let caption = if artifact.caption.is_empty() {
                "evidence"
            } else {
                &artifact.caption
            };
            body.push_str(&format!("[{caption}]({0})\n\n", artifact.url));
        }
    }
    body
}

pub fn comment_id_with_marker(list_json: &str, marker: &str) -> Option<u64> {
    let comments: Vec<Value> = serde_json::from_str(list_json).ok()?;
    comments
        .iter()
        .rev()
        .filter_map(|comment| {
            let body = comment.get("body")?.as_str()?;
            let id = comment.get("id")?.as_u64()?;
            body.contains(marker).then_some(id)
        })
        .next()
}

pub trait EvidenceRunner {
    fn run(
        &self,
        command: &str,
        worktree: &Path,
        base: &str,
        timeout: Duration,
    ) -> Result<EvidenceOutput>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellEvidence;

impl EvidenceRunner for ShellEvidence {
    fn run(
        &self,
        command: &str,
        worktree: &Path,
        base: &str,
        timeout: Duration,
    ) -> Result<EvidenceOutput> {
        let mut process = crate::daemon::shell_command(command);
        process
            .current_dir(worktree)
            .env("DEPOT_EVIDENCE_BASE", base)
            .env("DEPOT_EVIDENCE_RANGE", format!("{base}...HEAD"))
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = process.spawn().map_err(Error::Io)?;
        let mut piped = child
            .stdout
            .take()
            .ok_or_else(|| Error::Project("the evidence command closed its stdout".to_string()))?;
        let reader = thread::spawn(move || {
            let mut stdout = String::new();
            let _ = piped.read_to_string(&mut stdout);
            stdout
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait().map_err(Error::Io)? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::Project(format!(
                        "the evidence command timed out after {} seconds",
                        timeout.as_secs()
                    )));
                }
                None => thread::sleep(Duration::from_millis(100)),
            }
        };
        let stdout = reader.join().unwrap_or_default();
        Ok(EvidenceOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EVIDENCE_MARKER, EvidenceArtifact, comment_body, comment_id_with_marker, is_image,
        parse_manifest,
    };

    #[test]
    fn the_manifest_reads_url_caption_lines_and_ignores_everything_else() {
        let stdout = "noise without a tab\n\nhttps://img.example/shot.png\tlogin screen\n  https://vid.example/clip.mp4\tthe flow  \n\tdropemptyurl\n";
        assert_eq!(
            parse_manifest(stdout),
            vec![
                EvidenceArtifact {
                    url: "https://img.example/shot.png".to_owned(),
                    caption: "login screen".to_owned(),
                },
                EvidenceArtifact {
                    url: "https://vid.example/clip.mp4".to_owned(),
                    caption: "the flow".to_owned(),
                },
            ]
        );
        assert!(parse_manifest("").is_empty());
    }

    #[test]
    fn images_embed_and_other_media_stay_captioned_links() {
        let artifacts = vec![
            EvidenceArtifact {
                url: "https://img.example/shot.png".to_owned(),
                caption: "login screen".to_owned(),
            },
            EvidenceArtifact {
                url: "https://vid.example/clip.mp4".to_owned(),
                caption: "the flow".to_owned(),
            },
        ];
        let body = comment_body(&artifacts);
        assert!(body.starts_with(EVIDENCE_MARKER), "{body}");
        assert!(
            body.contains("![login screen](https://img.example/shot.png)"),
            "{body}"
        );
        assert!(
            body.contains("[the flow](https://vid.example/clip.mp4)"),
            "{body}"
        );
        let empty = comment_body(&[]);
        assert!(empty.starts_with(EVIDENCE_MARKER));
        assert!(empty.contains("captured nothing"), "{empty}");
    }

    #[test]
    fn image_detection_follows_the_url_extension_only() {
        assert!(is_image("https://example.test/a/b.PNG"));
        assert!(is_image("https://example.test/a.webp?token=1"));
        assert!(!is_image("https://example.test/a.mp4"));
        assert!(!is_image("https://example.test/noext"));
    }

    #[test]
    fn the_marker_finds_the_latest_marked_comment_id() {
        let list = r#"[
            {"id": 7, "body": "a plain review comment"},
            {"id": 9, "body": "older\n<!-- depot-evidence -->\n### Task evidence"},
            {"id": 11, "body": "newest\n<!-- depot-evidence -->\n### Task evidence"}
        ]"#;
        assert_eq!(comment_id_with_marker(list, EVIDENCE_MARKER), Some(11));
        assert_eq!(
            comment_id_with_marker(r#"[{"id": 7, "body": "plain"}]"#, EVIDENCE_MARKER),
            None
        );
        assert_eq!(comment_id_with_marker("not json", EVIDENCE_MARKER), None);
    }
}
