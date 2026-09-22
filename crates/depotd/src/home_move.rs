use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::adapters::forge::TOKEN_FILE;
use crate::adapters::typesafe::KEY_FILE;
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

pub struct LegacyLock {
    file: Option<File>,
}

impl LegacyLock {
    fn release(&mut self) {
        self.file = None;
    }
}

pub fn hold_legacy_lock(home: &DepotHome) -> Result<LegacyLock> {
    let Some(legacy) = home.legacy_sibling() else {
        return Ok(LegacyLock { file: None });
    };
    let root = legacy.root();
    if !root.is_dir() {
        return Ok(LegacyLock { file: None });
    }
    let path = root.join(DAEMON_LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(false)
        .create(true)
        .open(&path)?;
    if file.try_lock_exclusive().is_err() {
        return Err(Error::Home(legacy_lock_refusal(root)));
    }
    Ok(LegacyLock { file: Some(file) })
}

pub fn move_legacy_home(home: &DepotHome, mut lock: LegacyLock) -> Result<Option<PathBuf>> {
    let Some(legacy) = home.legacy_sibling() else {
        return Ok(None);
    };
    let legacy_root = legacy.root().to_path_buf();
    if !legacy_root.is_dir() {
        return Ok(None);
    }
    checkpoint_legacy_database(&legacy_root)?;
    home.ensure()?;
    move_settings(&legacy_root, home.root())?;
    move_database(&legacy_root, home.root())?;
    for entry in fs::read_dir(&legacy_root)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_database_file(&name) || name == OsStr::new(DAEMON_LOCK_FILE_NAME) {
            continue;
        }
        move_entry(&entry.path(), &home.root().join(destination_for(&name)))?;
    }
    lock.release();
    let lock_path = legacy_root.join(DAEMON_LOCK_FILE_NAME);
    if lock_path.exists() {
        fs::remove_file(&lock_path)?;
    }
    drop(lock);
    remove_if_empty(&legacy_root);
    Ok(Some(legacy_root))
}

fn is_database_file(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name == LEGACY_DATABASE_FILE_NAME || name == LEGACY_DATABASE_WAL || name == LEGACY_DATABASE_SHM
}

fn move_database(legacy_root: &Path, root: &Path) -> Result<()> {
    let source = legacy_root.join(LEGACY_DATABASE_FILE_NAME);
    if !source.is_file() {
        return Ok(());
    }
    let destination = root.join(DATABASE_FILE_NAME);
    if destination.exists() {
        let source_projects = projects_in(&source)?;
        let destination_projects = projects_in(&destination)?;
        if source_projects > 0 && destination_projects > 0 {
            return Err(Error::Home(format!(
                "cannot move {}: {} already holds records; merge them by hand, then run `depot store migrate` again",
                source.display(),
                destination.display()
            )));
        }
        if source_projects == 0 {
            remove_database(&source);
            return Ok(());
        }
        remove_database(&destination);
    }
    fs::rename(&source, &destination)?;
    Ok(())
}

fn checkpoint_legacy_database(legacy_root: &Path) -> Result<()> {
    let source = legacy_root.join(LEGACY_DATABASE_FILE_NAME);
    if source.is_file() {
        checkpoint_database(&source)?;
    }
    Ok(())
}

fn checkpoint_database(database: &Path) -> Result<()> {
    let connection = rusqlite::Connection::open(database)
        .map_err(|error| Error::Home(format!("cannot read {}: {error}", database.display())))?;
    let busy: i64 = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(|error| {
            Error::Home(format!("cannot checkpoint {}: {error}", database.display()))
        })?;
    if busy != 0 {
        return Err(Error::Home(format!(
            "cannot checkpoint {}: it is in use; stop whatever holds it, then run `depot store migrate` again",
            database.display()
        )));
    }
    drop(connection);
    Ok(())
}

fn remove_database(database: &Path) {
    if let Some(name) = database.file_name().and_then(|name| name.to_str()) {
        let _ = fs::remove_file(database.with_file_name(format!("{name}-wal")));
        let _ = fs::remove_file(database.with_file_name(format!("{name}-shm")));
    }
    let _ = fs::remove_file(database);
}

fn projects_in(database: &Path) -> Result<i64> {
    let connection = rusqlite::Connection::open(database)
        .map_err(|error| Error::Home(format!("cannot read {}: {error}", database.display())))?;
    connection
        .query_row("select count(*) from projects", [], |row| row.get(0))
        .map_err(|error| {
            Error::Home(format!(
                "cannot count the projects in {}: {error}",
                database.display()
            ))
        })
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
        TOKEN_FILE | KEY_FILE => Path::new(SECRETS_DIR_NAME).join(name.as_ref()),
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

fn legacy_lock_refusal(legacy_root: &Path) -> String {
    let path = legacy_root.join(DAEMON_LOCK_FILE_NAME);
    let held = format!("another depot daemon already holds {}", path.display());
    match legacy_scope_pid(legacy_root) {
        Some(pid) => format!(
            "{held}: pid {pid}; stop it, then run `depot store migrate` to move the old depot home into agni"
        ),
        None => format!(
            "{held}: the lock record names no daemon; stop it by hand, then run `depot store migrate`"
        ),
    }
}

fn legacy_scope_pid(legacy_root: &Path) -> Option<u32> {
    let bytes = fs::read(legacy_root.join(DAEMON_SCOPE_FILE_NAME)).ok()?;
    let scope: DaemonScope = serde_json::from_slice(&bytes).ok()?;
    (scope.pid != 0).then_some(scope.pid)
}
