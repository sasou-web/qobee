//! Error types for the Drive backend.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DriveError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("OS keychain error: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("OAuth flow failed: {0}")]
    OAuth(String),

    /// The stored refresh token was rejected by Google. The caller
    /// should present the wizard again to acquire a fresh one.
    #[error("authentication required: {0}")]
    NeedsReauth(String),

    #[error("Drive API rejected the request: {status} {body}")]
    Api { status: u16, body: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("operation cancelled")]
    Cancelled,

    #[error("internal error: {0}")]
    Internal(String),
}

pub type DriveResult<T> = Result<T, DriveError>;
