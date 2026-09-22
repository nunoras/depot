use depot_core::{ProjectId, Timestamp};

use std::path::PathBuf;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationKind {
    Path,
    Url,
}

impl LocationKind {
    pub fn name(self) -> &'static str {
        match self {
            LocationKind::Path => "path",
            LocationKind::Url => "url",
        }
    }

    pub fn from_name(name: &str) -> Result<Self> {
        match name {
            "path" => Ok(LocationKind::Path),
            "url" => Ok(LocationKind::Url),
            other => Err(Error::Schema(format!(
                "unknown project location kind `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub kind: LocationKind,
    pub slug: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clone {
    pub project: ProjectId,
    pub path: PathBuf,
    pub origin: Option<String>,
}
