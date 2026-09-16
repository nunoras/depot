use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::home::ProjectHome;

pub fn document_path(home: &ProjectHome, name: &str) -> Result<PathBuf> {
    let relative = Path::new(name.trim());
    if relative.as_os_str().is_empty() {
        return Err(Error::Project("a document needs a name".to_string()));
    }
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(Error::Project(format!(
            "`{name}` is not a document name inside the store: name a file under `{}`",
            home.documents_dir().display()
        )));
    }
    Ok(home.documents_dir().join(relative))
}

pub fn write_document(home: &ProjectHome, name: &str, content: &str) -> Result<PathBuf> {
    let path = document_path(home, name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    Ok(path)
}
