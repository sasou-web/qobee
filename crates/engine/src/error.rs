//! Error types used by the audio engine.

use thiserror::Error;

/// Result alias used by every fallible engine operation.
pub type EngineResult<T> = Result<T, EngineError>;

/// All failure modes the engine can produce. Variants are intentionally
/// explicit so the UI can render accurate, non-misleading messages
/// (especially around bit-perfect playback claims).
#[derive(Debug, Error)]
pub enum EngineError {
    /// File could not be opened or read.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Symphonia (decoding) failure.
    #[error("decoder error: {0}")]
    Decode(String),

    /// No supported audio stream was found in the file.
    #[error("no playable audio stream found in file")]
    NoAudioStream,

    /// CPAL host or stream creation failed.
    #[error("audio output error: {0}")]
    Output(String),

    /// The requested output mode or device is not available.
    #[error("output mode unavailable: {0}")]
    BackendUnavailable(String),

    /// Operation called in a state that does not allow it (e.g. seek with
    /// no track loaded, play without load, ...).
    #[error("invalid engine state: {0}")]
    InvalidState(&'static str),

    /// Catch-all for unexpected internal errors. Should be rare and
    /// always logged.
    #[error("internal engine error: {0}")]
    Internal(String),

    // ---- Convolver IR loading (R9.2 / R9.3 / R9.8) ----
    /// IR exceeds the design's `100_000`-tap budget after resampling.
    /// Carries the actual length so the UI can report it.
    #[error("convolver IR too long after resample (max 100000 taps)")]
    ConvolverIrTooLong,

    /// IR contains NaN / Inf samples or an empty channel — anything
    /// the convolver cannot run on.
    #[error("convolver IR invalid: {0}")]
    ConvolverIrInvalid(String),

    /// IR file could not be opened, decoded, or resampled.
    #[error("convolver IR load failed: {0}")]
    ConvolverIrLoadFailed(String),

    // ---- DSD parsers (R7.2) ----
    /// DSF or DFF parser rejected the supplied file. The string
    /// carries the offset / chunk identifier so the issue can be
    /// triaged from logs alone.
    #[error("DSD invalid file: {0}")]
    DsdInvalidFile(String),

    // ---- Null-test diagnostic (R11.8) ----
    /// WASAPI loopback capture refused to start (driver lacks
    /// loopback support, the device is held by another exclusive
    /// client, or `IAudioClient::Initialize` returned a hard error).
    /// The diagnostic falls back to an `Inconclusive` report and
    /// surfaces this string in the UI so the user knows why.
    #[error("null-test loopback failed: {0}")]
    NullTestLoopbackFailed(String),
}
