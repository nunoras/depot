use std::path::{Path, PathBuf};
use std::process::Command;

const UNKNOWN: &str = "unknown";

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    if let Some(git_dir) = git_path(&manifest, "--absolute-git-dir") {
        let common_dir = git_path(&manifest, "--git-common-dir")
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| git_dir.clone());
        for path in tracked_paths(&git_dir, &common_dir) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    println!("cargo:rustc-env=DEPOT_BUILD_ID={}", build_id(&manifest));
}

fn build_id(directory: &Path) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(["rev-parse", "--short=12", "HEAD"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if id.is_empty() {
                UNKNOWN.to_string()
            } else {
                id
            }
        }
        _ => UNKNOWN.to_string(),
    }
}

fn git_path(directory: &Path, flag: &str) -> Option<PathBuf> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(["rev-parse", flag])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let path = if raw.is_absolute() {
        raw
    } else {
        directory.join(raw)
    };
    Some(path)
}

fn tracked_paths(git_dir: &Path, common_dir: &Path) -> Vec<PathBuf> {
    let head = git_dir.join("HEAD");
    let mut paths = vec![head.clone()];
    let Ok(contents) = std::fs::read_to_string(&head) else {
        return paths;
    };
    let Some(reference) = contents.trim().strip_prefix("ref: ") else {
        return paths;
    };
    let mut candidates = vec![git_dir.join(reference), common_dir.join(reference)];
    candidates.push(common_dir.join("packed-refs"));
    paths.extend(candidates.into_iter().filter(|path| path.is_file()));
    paths
}
