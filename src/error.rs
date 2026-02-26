use std::fmt;

pub type Result<T> = std::result::Result<T, HeftError>;

#[derive(Debug)]
pub enum HeftError {
    Io(std::io::Error),
    Db(heed::Error),
    Encoding(String),
    Json(serde_json::Error),
    Other(String),
}

impl fmt::Display for HeftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeftError::Io(e) => write!(f, "IO error: {e}"),
            HeftError::Db(e) => write!(f, "Database error: {e}"),
            HeftError::Encoding(msg) => write!(f, "Encoding error: {msg}"),
            HeftError::Json(e) => write!(f, "JSON error: {e}"),
            HeftError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for HeftError {}

impl From<std::io::Error> for HeftError {
    fn from(e: std::io::Error) -> Self {
        HeftError::Io(e)
    }
}

impl From<heed::Error> for HeftError {
    fn from(e: heed::Error) -> Self {
        HeftError::Db(e)
    }
}

impl From<serde_json::Error> for HeftError {
    fn from(e: serde_json::Error) -> Self {
        HeftError::Json(e)
    }
}
