use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::daemon::{DAEMON_LOCK_FILE_NAME, DAEMON_SCOPE_FILE_NAME, DaemonScope};
use crate::error::{Error, Result};
use crate::home::{
    DATABASE_FILE_NAME, DepotHome, LEGACY_DATABASE_FILE_NAME, RUN_DIR_NAME, SECRETS_DIR_NAME,
    SETTINGS_FILE_NAME,
};
use crate::restart::{DAEMON_LOG_FILE_NAME, DAEMON_STOP_FILE_NAME};
use crate::settings::Settings;

const LEGACY_DATABASE_WAL: &str = "depot.db-wal";
const LEGACY_DATABASE_SHM: &str = "depot.db-shm";
const LEGACY_TOKEN_FILE: &str = "github-token";
const LEGACY_KEY_FILE: &str = "typesafe-key";

pub fn move_legacy_home(home: &DepotHome) -> Result<Option<PathBuf>> {
    let Some(legacy) = home.legacy_sibling() else {
        return Ok(None);
    };
    let legacy_root = legacy.root().to_path_buf();
    if !legacy_root.is_dir() {
        return Ok(None);
    }
    refuse_if_a_daemon_holds_the_legacy_lock(&legacy_root)?;
    refuse_a_second_database(&legacy_root, home.root())?;
    home.ensure()?;
    move_settings(&legacy_root, home.root())?;
    for entry in fs::read_dir(&legacy_root)? {
        let entry = entry?;
        if entry.file_name() == OsStr::new(DAEMON_LOCK_FILE_NAME) {
            fs::remove_file(entry.path())?;
            continue;
        }
        move_entry(
            &entry.path(),
            &home.root().join(destination_for(&entry.file_name())),
        )?;
    }
    remove_if_empty(&legacy_root);
    Ok(Some(legacy_root))
}

fn refuse_a_second_database(legacy_root: &Path, root: &Path) -> Result<()> {
    let source = legacy_root.join(LEGACY_DATABASE_FILE_NAME);
    let destination = root.join(DATABASE_FILE_NAME);
    if source.is_file() && destination.exists() {
        return Err(Error::Home(format!(
            "cannot move {}: {} already holds records; merge them by hand, then run `depot store migrate` again",
            source.display(),
            destination.display()
        )));
    }
    Ok(())
}

fn move_settings(legacy_root: &Path, root: &Path) -> Result<()> {
    let source = legacy_root.join(SETTINGS_FILE_NAME);
    if !source.is_file() {
        return Ok(());
    }
    let destination = root.join(SETTINGS_FILE_NAME);
    if destination.is_file() {
        let existing = fs::read_to_string(&destination).unwrap_or_default();
        if existing != Settings::default().to_toml()? {
            return Err(Error::Home(format!(
                "cannot move {}: {} already holds settings; move them by hand, then run `depot store migrate` again",
                source.display(),
                destination.display()
            )));
        }
        fs::remove_file(&destination)?;
    }
    fs::rename(&source, &destination)?;
    Ok(())
}

fn destination_for(name: &OsStr) -> PathBuf {
    let name = name.to_string_lossy();
    match name.as_ref() {
        LEGACY_DATABASE_FILE_NAME => PathBuf::from(DATABASE_FILE_NAME),
        LEGACY_DATABASE_WAL => PathBuf::from(format!("{DATABASE_FILE_NAME}-wal")),
        LEGACY_DATABASE_SHM => PathBuf::from(format!("{DATABASE_FILE_NAME}-shm")),
        LEGACY_TOKEN_FILE | LEGACY_KEY_FILE => Path::new(SECRETS_DIR_NAME).join(name.as_ref()),
        DAEMON_LOCK_FILE_NAME | DAEMON_SCOPE_FILE_NAME | DAEMON_STOP_FILE_NAME => {
            Path::new(RUN_DIR_NAME).join(name.as_ref())
        }
        _ if name.starts_with(DAEMON_LOG_FILE_NAME) => Path::new(RUN_DIR_NAME).join(name.as_ref()),
        _ => PathBuf::from(name.as_ref()),
    }
}

fn move_entry(source: &Path, destination: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(source) else {
        return Ok(());
    };
    if fs::symlink_metadata(destination).is_err() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(source, destination)?;
        return Ok(());
    }
    if metadata.is_dir() && destination.is_dir() {
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            move_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        remove_if_empty(source);
        return Ok(());
    }
    Err(Error::Home(format!(
        "cannot move {}: {} already exists",
        source.display(),
        destination.display()
    )))
}

fn remove_if_empty(directory: &Path) {
    if fs::read_dir(directory).is_ok_and(|mut entries| entries.next().is_none()) {
        let _ = fs::remove_dir(directory);
    }
}

fn refuse_if_a_daemon_holds_the_legacy_lock(legacy_root: &Path) -> Result<()> {
    let path = legacy_root.join(DAEMON_LOCK_FILE_NAME);
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
    else {
        return Ok(());
    };
    if file.try_lock_exclusive().is_ok() {
        let _ = file.unlock();
        return Ok(());
    }
    let held = format!("another depot daemon already holds {}", path.display());
    Err(Error::Home(match legacy_scope_pid(legacy_root) {
        Some(pid) => format!(
            "{held}: pid {pid}; stop it, then run `depot store migrate` to move the old depot home into agni"
        ),
        None => format!(
            "{held}: the lock record names no daemon; stop it by hand, then run `depot store migrate`"
        ),
    }))
}

fn legacy_scope_pid(legacy_root: &Path) -> Option<u32> {
    let bytes = fs::read(legacy_root.join(DAEMON_SCOPE_FILE_NAME)).ok()?;
    let scope: DaemonScope = serde_json::from_slice(&bytes).ok()?;
    (scope.pid != 0).then_some(scope.pid)
}
