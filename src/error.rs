use thiserror::Error;

#[derive(Error, Debug)]
pub enum LyricError {
    #[error("MPRIS error: {0}")]
    MprisError(#[from] mpris::DBusError),

    #[error("MPRIS Find error: {0}")]
    MprisFindError(#[from] mpris::FindingError),

    #[error("HTTP error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("base64 error: {0}")]
    DecodeError(#[from] base64::DecodeError),

    #[error("No active media player found")]
    NoPlayerFound,

    #[error("Failed to get cache path")]
    CachePathError,

    #[error("No lyrics found")]
    NoLyricFound,

    #[error("JSON parse error")]
    JsonError,

    #[error("Lyric validation failed")]
    LyricValidationFailed,

    #[error("Lyric decode failed")]
    LyricDecodeError,

    #[error("Invalid time format")]
    InvalidTimeFormat,

    #[error("Empty lyric content")]
    EmptyLyric,
}
