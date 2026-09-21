use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

pub const PROOF_START: &str = "<!-- depot-proof:start -->";
pub const PROOF_END: &str = "<!-- depot-proof:end -->";
pub const MANAGED_MARKER: &str = "<!-- depot-managed -->";

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceArtifact {
    Url { url: String, caption: String },
    File { path: PathBuf, caption: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedArtifact {
    Url { url: String, caption: String },
    LocalFile { path: PathBuf, caption: String },
}

impl ResolvedArtifact {
    pub fn caption(&self) -> &str {
        match self {
            ResolvedArtifact::Url { caption, .. } | ResolvedArtifact::LocalFile { caption, .. } => {
                caption
            }
        }
    }

    pub fn is_url(&self) -> bool {
        matches!(self, ResolvedArtifact::Url { .. })
    }
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
            let (value, caption) = line.trim().split_once('\t')?;
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            let caption = caption.trim().to_owned();
            Some(if value.contains("://") {
                EvidenceArtifact::Url {
                    url: value.to_owned(),
                    caption,
                }
            } else {
                EvidenceArtifact::File {
                    path: PathBuf::from(value),
                    caption,
                }
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

pub fn resolve_artifacts(worktree: &Path, artifacts: &[EvidenceArtifact]) -> Vec<ResolvedArtifact> {
    artifacts
        .iter()
        .map(|artifact| match artifact {
            EvidenceArtifact::Url { url, caption } => ResolvedArtifact::Url {
                url: url.clone(),
                caption: caption.clone(),
            },
            EvidenceArtifact::File { path, caption } => {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    worktree.join(path)
                };
                ResolvedArtifact::LocalFile {
                    path,
                    caption: caption.clone(),
                }
            }
        })
        .collect()
}

pub fn render_proof(artifacts: &[ResolvedArtifact]) -> String {
    let mut body = String::from(PROOF_START);
    body.push_str("\n## Proof\n\n");
    if artifacts.is_empty() {
        body.push_str("The evidence command ran but captured nothing for this commit.\n");
    } else {
        for artifact in artifacts {
            let caption = if artifact.caption().is_empty() {
                "evidence"
            } else {
                artifact.caption()
            };
            match artifact {
                ResolvedArtifact::Url { url, .. } if is_image(url) => {
                    body.push_str(&format!("![{caption}]({url})\n\n"));
                }
                ResolvedArtifact::Url { url, caption } if caption.is_empty() => {
                    body.push_str(&format!("{url}\n\n"));
                }
                ResolvedArtifact::Url { url, .. } => {
                    body.push_str(&format!("{caption}\n\n{url}\n\n"));
                }
                ResolvedArtifact::LocalFile { path, .. } => {
                    body.push_str(&format!(
                        "{caption}\n\n`{}` is a local file, so it cannot be attached to a pull request. Publish it at a URL and add that URL to the evidence manifest.\n\n",
                        path.display()
                    ));
                }
            }
        }
    }
    body.push_str(PROOF_END);
    body
}

pub fn mark_managed(body: &str) -> String {
    if is_managed(body) {
        return body.to_owned();
    }
    format!("{MANAGED_MARKER}\n\n{}", body.trim_start())
}

pub fn is_managed(body: &str) -> bool {
    body.contains(MANAGED_MARKER)
}

pub fn upsert_proof(body: &str, proof: &str) -> String {
    match proof_section(body) {
        Some(section) => {
            let start = body
                .find(PROOF_START)
                .expect("the section starts where found");
            let end = start + section.len();
            format!("{}{}{}", &body[..start], proof, &body[end..])
        }
        None => {
            let trimmed = body.trim_end();
            if trimmed.is_empty() {
                format!("{proof}\n")
            } else {
                format!("{trimmed}\n\n{proof}\n")
            }
        }
    }
}

pub fn preserve_proof(existing: &str, fresh: &str) -> String {
    match proof_section(existing) {
        Some(section) => upsert_proof(fresh, section),
        None => fresh.to_owned(),
    }
}

pub fn proof_section(body: &str) -> Option<&str> {
    let start = body.find(PROOF_START)?;
    let end = body[start..].find(PROOF_END)? + start + PROOF_END.len();
    Some(&body[start..end])
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
    use std::path::{Path, PathBuf};

    use super::{
        EvidenceArtifact, MANAGED_MARKER, PROOF_END, PROOF_START, ResolvedArtifact, is_image,
        is_managed, mark_managed, parse_manifest, preserve_proof, proof_section, render_proof,
        resolve_artifacts, upsert_proof,
    };

    fn resolved(url: &str, caption: &str) -> ResolvedArtifact {
        ResolvedArtifact::Url {
            url: url.to_owned(),
            caption: caption.to_owned(),
        }
    }

    #[test]
    fn the_manifest_reads_url_and_file_records_and_ignores_everything_else() {
        let stdout = "noise without a tab\n\nhttps://img.example/shot.png\tlogin screen\n  https://vid.example/clip.mp4\tthe flow  \n/tmp/clip.mp4\tthe upload  \n\tdropemptyurl\n";
        assert_eq!(
            parse_manifest(stdout),
            vec![
                EvidenceArtifact::Url {
                    url: "https://img.example/shot.png".to_owned(),
                    caption: "login screen".to_owned(),
                },
                EvidenceArtifact::Url {
                    url: "https://vid.example/clip.mp4".to_owned(),
                    caption: "the flow".to_owned(),
                },
                EvidenceArtifact::File {
                    path: PathBuf::from("/tmp/clip.mp4"),
                    caption: "the upload".to_owned(),
                },
            ]
        );
        assert!(parse_manifest("").is_empty());
    }

    #[test]
    fn a_local_file_is_noted_not_attached_and_a_relative_path_resolves_against_the_worktree() {
        let worktree = Path::new("/work");
        let artifacts = parse_manifest("out/clip.mp4\tthe flow\n");
        let resolved = resolve_artifacts(worktree, &artifacts);
        assert_eq!(
            resolved,
            vec![ResolvedArtifact::LocalFile {
                path: worktree.join("out/clip.mp4"),
                caption: "the flow".to_owned(),
            }]
        );
        assert!(!resolved[0].is_url(), "a local file is never a usable url");
        let body = render_proof(&resolved);
        assert!(body.contains("the flow"), "{body}");
        assert!(
            body.contains("cannot be attached to a pull request"),
            "{body}"
        );
        assert!(
            body.contains("Publish it at a URL and add that URL to the evidence manifest."),
            "{body}"
        );
        assert!(!body.contains("![the flow]"), "{body}");

        let absolute = parse_manifest("/tmp/clip.mp4\tthe upload\n");
        let resolved = resolve_artifacts(worktree, &absolute);
        assert_eq!(
            resolved[0],
            ResolvedArtifact::LocalFile {
                path: PathBuf::from("/tmp/clip.mp4"),
                caption: "the upload".to_owned(),
            }
        );
    }

    #[test]
    fn a_body_depot_owns_carries_the_managed_marker_once() {
        let marked = mark_managed("## Why\n\nsomething\n");
        assert!(marked.starts_with(MANAGED_MARKER), "{marked}");
        assert!(is_managed(&marked));
        assert!(!is_managed("## Why\n\nsomeone else's body\n"));
        assert!(!is_managed(""));
    }

    #[test]
    fn images_embed_and_other_media_are_a_bare_url() {
        let artifacts = vec![
            resolved("https://img.example/shot.png", "login screen"),
            resolved("https://vid.example/clip.mp4", "the flow"),
        ];
        let body = render_proof(&artifacts);
        assert!(body.starts_with(PROOF_START), "{body}");
        assert!(body.ends_with(PROOF_END), "{body}");
        assert!(
            body.contains("![login screen](https://img.example/shot.png)"),
            "{body}"
        );
        assert!(
            body.contains("the flow\n\nhttps://vid.example/clip.mp4"),
            "{body}"
        );
        assert!(
            !body.contains("[the flow](https://vid.example/clip.mp4)"),
            "a video is not a markdown link: {body}"
        );
        let empty = render_proof(&[]);
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
    fn the_proof_section_is_replaced_in_place() {
        let body = "## Why\n\nsome reasoning\n\n## Validation\n\ncargo test\n";
        let once = upsert_proof(body, &render_proof(&[resolved("https://a.test/x.mp4", "")]));
        assert!(once.contains("## Why"));
        assert!(once.contains("## Validation"));
        assert_eq!(once.matches("## Proof").count(), 1);

        let twice = upsert_proof(
            &once,
            &render_proof(&[resolved("https://a.test/y.mp4", "")]),
        );
        assert_eq!(twice.matches("## Proof").count(), 1);
        assert!(twice.contains("https://a.test/y.mp4"), "{twice}");
        assert!(!twice.contains("https://a.test/x.mp4"), "{twice}");
        assert!(twice.contains("## Validation"));
    }

    #[test]
    fn a_fresh_body_keeps_the_owned_proof_section_across_a_describe_refresh() {
        let published = upsert_proof(
            "## Why\n\nold\n",
            &render_proof(&[resolved("https://a.test/x.mp4", "clip")]),
        );
        let refreshed = "## Why\n\nnew\n\n## Validation\n\ncargo test\n";
        let merged = preserve_proof(&published, refreshed);
        assert!(merged.contains("## Why\n\nnew"), "{merged}");
        assert!(merged.contains("https://a.test/x.mp4"), "{merged}");
        assert_eq!(merged.matches("## Proof").count(), 1);
        assert_eq!(preserve_proof("no proof here", refreshed), refreshed);
    }

    #[test]
    fn the_section_reads_between_its_markers() {
        let body =
            "before\n<!-- depot-proof:start -->\n## Proof\nx\n<!-- depot-proof:end -->\nafter";
        assert_eq!(
            proof_section(body),
            Some("<!-- depot-proof:start -->\n## Proof\nx\n<!-- depot-proof:end -->")
        );
        assert_eq!(proof_section("nothing"), None);
        assert_eq!(
            proof_section("<!-- depot-proof:start -->unterminated"),
            None
        );
    }
}
