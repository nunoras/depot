use std::path::Path;
use std::process::Command;

pub fn read_origin(directory: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let origin = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if origin.is_empty() {
        None
    } else {
        Some(origin)
    }
}

pub fn identity_for(directory: &Path, origin: Option<&str>) -> String {
    match origin {
        Some(origin) => identity_for_origin(origin).unwrap_or_else(|| {
            canonical(&directory.to_string_lossy())
                .unwrap_or_else(|| directory.to_string_lossy().to_string())
        }),
        None => canonical(&directory.to_string_lossy())
            .unwrap_or_else(|| directory.to_string_lossy().to_string()),
    }
}

pub fn identity_for_origin(origin: &str) -> Option<String> {
    depot_core::remote_identity(origin).or_else(|| canonical(origin))
}

fn canonical(value: &str) -> Option<String> {
    let value = value.strip_prefix("file://").unwrap_or(value);
    let path = Path::new(value);
    let canonical = std::fs::canonicalize(path).ok()?;
    Some(canonical.to_string_lossy().to_string())
}
