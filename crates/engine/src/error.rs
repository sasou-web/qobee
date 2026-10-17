//! Error types used by the audio engine.

use thiserror::Error;

/// Result alias used by every fallible engine operation.
pub type EngineResult<T> = Result<T, EngineError>;

/// All failure modes the engine can produce.
#[derive(Debug, Error)]
pub enum EngineError {
    /// File could not be opened or read.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Decoding failure (rodio / Symphonia rejected the stream).
    #[error("decoder error: {0}")]
    Decode(String),

    /// No supported audio stream was found in the file.
    #[error("no playable audio stream found in file")]
    NoAudioStream,

    /// CPAL host or output stream creation failed.
    #[error("audio output error: {0}")]
    Output(String),

    /// The requested output device is not available.
    #[error("output device unavailable: {0}")]
    BackendUnavailable(String),

    /// Operation called in a state that does not allow it (e.g. seek
    /// with no track loaded, play without load, ...).
    #[error("invalid engine state: {0}")]
    InvalidState(&'static str),

    /// Catch-all for unexpected internal errors. Should be rare and
    /// always logged.
    #[error("internal engine error: {0}")]
    Internal(String),
}
