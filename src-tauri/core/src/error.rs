use serde::Serialize;

/// Errors surfaced to the frontend. Messages must never contain clipboard content.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("not found")]
    NotFound,
    #[error("clipboard unavailable: {0}")]
    Clipboard(String),
    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Other(format!("json error: {e}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;
