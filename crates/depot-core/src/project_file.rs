pub const PROJECT_DIRECTORY: &str = ".agni";

pub fn project_files_touched(paths: &[String]) -> Vec<String> {
    let prefix = format!("{PROJECT_DIRECTORY}/");
    paths
        .iter()
        .filter(|path| path.as_str() == PROJECT_DIRECTORY || path.starts_with(&prefix))
        .cloned()
        .collect()
}

pub fn project_file_hold_reason(files: &[String]) -> String {
    format!(
        "the change edits project files a person must review and merge: {}",
        files.join(", ")
    )
}
