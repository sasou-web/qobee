//! Qobee audio engine.
//!
//! This crate exposes the [`AudioEngine`] trait and two implementations:
//!
//!   * [`backend_cpal_shared::CpalSharedEngine`] — default. Routes
//!     audio through the OS mixer (WASAPI Shared on Windows). Uses
//!     a sinc resampler when the device's rate differs from the file
//!     rate, applies a 10-band peaking EQ, ReplayGain pre-gain, soft
//!     clipping, and TPDF dither when the device asks for ≤ 16-bit.
//!
//!   * [`backend_wasapi_exclusive::WasapiExclusiveEngine`] (Windows
//!     only) — opt-in. Bypasses the OS mixer and reaches a strictly
//!     bit-perfect chain when the device natively accepts the source's
//!     sample rate, channel count, and bit depth. Other applications
//!     cannot play to the same device while this backend is active.
//!
//! ## Honest reporting
//!
//! Shared mode runs through the OS mixer (WASAPI Shared on Windows).
//! It is excellent quality — within the audible range, indistinguishable
//! from bit-perfect for nearly every consumer file — but it is *not*
//! bit-perfect by definition: the OS may dither, mix with other apps,
//! and apply system-wide effects. The UI describes it as "Shared
//! (lossless to OS mixer)" rather than making bit-perfect claims it
//! cannot uphold. Exclusive mode reports `is_bit_perfect = true` only
//! when *every* element of the chain is at unity (volume, EQ, RG) and
//! no resampling or upmix happened.

pub mod backend_cpal_shared;
pub mod backend_symphonia;
#[cfg(target_os = "windows")]
pub mod backend_wasapi_exclusive;
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

    // ---- Common knobs every backend must support ----
    fn set_current_track_id(&self, id: Option<String>);
    fn set_eq_gains_db(&self, gains: &[f32]);
    fn eq_gains_db(&self) -> Vec<f32>;
    fn set_pre_gain(&self, linear: f32);
    fn set_output_device(&self, device_id: Option<String>);
    fn selected_device(&self) -> Option<String>;

    // ---- Gapless ----

    /// Open `path` ahead of time so the next end-of-stream can swap
    /// to it without an audio gap. Implementations that don't
    /// support gapless can return `Ok(())` and silently ignore;
    /// the orchestrator falls back to the standard EOT → Load path.
    fn prepare_next(&self, path: &Path, track_id: Option<String>) -> EngineResult<()>;

    /// Drop any previously prepared next track.
    fn clear_pending_next(&self) -> EngineResult<()>;
}
