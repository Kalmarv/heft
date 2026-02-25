use std::fmt;

pub type Result<T> = std::result::Result<T, DuxError>;

#[derive(Debug)]
pub enum DuxError {
    Io(std::io::Error),
    Db(heed::Error),
    Encoding(String),
    Json(serde_json::Error),
    Other(String),
}

impl fmt::Display for DuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DuxError::Io(e) => write!(f, "IO error: {e}"),
            DuxError::Db(e) => write!(f, "Database error: {e}"),
            DuxError::Encoding(msg) => write!(f, "Encoding error: {msg}"),
            DuxError::Json(e) => write!(f, "JSON error: {e}"),
            DuxError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for DuxError {}

impl From<std::io::Error> for DuxError {
    fn from(e: std::io::Error) -> Self {
        DuxError::Io(e)
    }
}

impl From<heed::Error> for DuxError {
    fn from(e: heed::Error) -> Self {
        DuxError::Db(e)
    }
}

impl From<serde_json::Error> for DuxError {
    fn from(e: serde_json::Error) -> Self {
        DuxError::Json(e)
    }
}
