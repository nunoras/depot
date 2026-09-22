use std::error::Error as StdError;
use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Database(rusqlite::Error),
    ConfigDecode(toml::de::Error),
    ConfigEncode(toml::ser::Error),
    Home(String),
    Project(String),
    Config(String),
    Template(String),
    Schema(String),
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn is_lock_contention(&self) -> bool {
        matches!(
            self,
            Error::Database(rusqlite::Error::SqliteFailure(ffi, _))
                if ffi.code == rusqlite::ErrorCode::DatabaseBusy
                    || ffi.code == rusqlite::ErrorCode::DatabaseLocked
        )
    }

    pub fn is_project(&self) -> bool {
        matches!(self, Error::Project(_))
    }

    pub fn is_task_fatal(&self) -> bool {
        matches!(self, Error::Config(_) | Error::Template(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "{error}"),
            Error::Database(error) => write!(f, "database error: {error}"),
            Error::ConfigDecode(error) => write!(f, "invalid configuration: {error}"),
            Error::ConfigEncode(error) => {
                write!(f, "could not encode configuration: {error}")
            }
            Error::Home(message) | Error::Project(message) | Error::Config(message) => {
                f.write_str(message)
            }
            Error::Template(message) => write!(f, "template error: {message}"),
            Error::Schema(message) => write!(f, "store schema error: {message}"),
            Error::NotFound(message) => f.write_str(message),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            Error::Database(error) => Some(error),
            Error::ConfigDecode(error) => Some(error),
            Error::ConfigEncode(error) => Some(error),
            Error::Home(_)
            | Error::Project(_)
            | Error::Config(_)
            | Error::Template(_)
            | Error::Schema(_)
            | Error::NotFound(_) => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Error::Database(error)
    }
}

impl From<toml::de::Error> for Error {
    fn from(error: toml::de::Error) -> Self {
        Error::ConfigDecode(error)
    }
}

impl From<toml::ser::Error> for Error {
    fn from(error: toml::ser::Error) -> Self {
        Error::ConfigEncode(error)
    }
}
