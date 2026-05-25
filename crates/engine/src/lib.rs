//! Qobee audio engine.
//!
//! This crate exposes the [`AudioEngine`] trait and the default
//! implementation built around Symphonia (decoding) and CPAL
//! (Shared output, with in-app FFT resampling when the device rate
//! differs from the file rate).
//!
//! ## Honest reporting
//!
//! Shared mode runs through the OS mixer (WASAPI Shared on Windows).
//! It is excellent quality — within the audible range, indistinguishable
//! from bit-perfect for nearly every consumer file — but it is *not*
//! bit-perfect by definition: the OS may dither, mix with other apps,
//! and apply system-wide effects. The UI describes it as "Shared
//! (lossless to OS mixer)" rather than making bit-perfect claims it
//! cannot uphold.

pub mod backend_cpal_shared;
pub mod backend_symphonia;
pub mod eq;
pub mod error;
pub mod types;

pub use error::{EngineError, EngineResult};
pub use types::{
    EffectiveOutputMode, EngineEvent, OutputDevice, OutputMode, PcmBuffer, PlaybackStatus,
    PlayerState, TrackFormat,
};

use crossbeam_channel::Receiver;
use std::path::Path;

/// Common interface every audio backend must satisfy.
pub trait AudioEngine: Send + Sync {
    fn load(&self, path: &Path) -> EngineResult<()>;
    fn play(&self) -> EngineResult<()>;
    fn pause(&self) -> EngineResult<()>;
    fn resume(&self) -> EngineResult<()>;
    fn stop(&self) -> EngineResult<()>;
    fn seek(&self, position_seconds: f64) -> EngineResult<()>;
    fn set_volume(&self, volume: f32) -> EngineResult<()>;
    fn set_output_mode(&self, mode: OutputMode) -> EngineResult<()>;
    fn state(&self) -> PlayerState;
    fn subscribe_events(&self) -> Receiver<EngineEvent>;
}
