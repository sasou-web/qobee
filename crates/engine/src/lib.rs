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

pub mod audio_settings;
pub mod backend_cpal_shared;
pub mod backend_symphonia;
#[cfg(target_os = "windows")]
pub mod backend_wasapi_exclusive;
pub mod diagnostic;
pub mod dsd;
pub mod dsp;
pub mod eq;
pub mod error;
pub mod types;
pub mod volume;

pub use audio_settings::*;
pub use dsd::{DopPacker, DsdGroup16, DsdRate, DsdStream};
pub use dsp::{PreGainContext, StagesBypass};
pub use error::{EngineError, EngineResult};
pub use types::{
    BitPerfectHealth, BitPerfectStatus, EffectiveOutputMode, EngineEvent, OutputDevice, OutputMode,
    PcmBuffer, PlaybackStatus, PlayerState, TrackFormat,
};

use crossbeam_channel::Receiver;
use std::path::Path;

/// Common interface every audio backend must satisfy.
pub trait AudioEngine: Send + Sync {
    fn load(&self, path: &Path) -> EngineResult<()>;
    /// Open a DSD track on the engine. Implementations that do not
    /// support DSD (CPAL Shared) MUST return
    /// `EngineError::BackendUnavailable`. The WASAPI Exclusive
    /// backend (Windows-only) implements the full DSF/DFF + DoP
    /// pipeline (R7.3 / R7.5 / R7.7). Default implementation rejects
    /// the call so non-DSD backends don't have to override anything.
    fn load_dsd(&self, _path: &Path, _rate: DsdRate) -> EngineResult<()> {
        Err(EngineError::BackendUnavailable(
            "DSD playback requires WASAPI Exclusive on Windows".to_string(),
        ))
    }
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

    /// Replace the engine's published [`AudioSettings`] snapshot. The
    /// decoder thread observes the new values lazily on the next
    /// chunk boundary (compare-and-swap on the `version` counter).
    fn set_audio_settings(&self, settings: AudioSettings);

    /// Push a new per-load context (RG dB/peak from the loaded track
    /// and the current volume slider) into the `Pre_Gain_Stage`. The
    /// decoder thread picks it up on the next chunk boundary.
    ///
    /// Called by the orchestration layer (`Player::start_track`)
    /// after a track load and on volume changes so the stage's
    /// combined `g_linear` stays in sync with both the track's tags
    /// and the user's slider position.
    fn set_pre_gain_context(&self, ctx: PreGainContext);

    /// Forward a freshly-loaded convolver impulse response (R9.2)
    /// to the engine. The decoder thread picks it up at the next
    /// chunk boundary and forwards it to `PcmChain::set_convolver_ir`.
    /// Pass two empty vectors to drop the IR (the convolver stage
    /// falls back to bypass on the next chunk).
    fn set_convolver_ir(&self, ir_left: Vec<f32>, ir_right: Vec<f32>);

    /// Length of the currently loaded convolver IR in taps (per
    /// channel); zero when none loaded. Read by
    /// `get_convolver_status` to compute the reported latency.
    fn convolver_ir_len(&self) -> usize;

    /// Whether the engine is currently rendering a DSD stream
    /// (R7.8). Used by the orchestrator to gate volume / EQ /
    /// pre-gain mutations: requests received while this returns
    /// `true` are cached for the next PCM track but not forwarded
    /// to the engine. Default implementation reports `false` for
    /// backends that do not support DSD.
    fn is_dsd_active(&self) -> bool {
        false
    }

    // ---- Gapless ----

    /// Open `path` ahead of time so the next end-of-stream can swap
    /// to it without an audio gap. Implementations that don't
    /// support gapless can return `Ok(())` and silently ignore;
    /// the orchestrator falls back to the standard EOT → Load path.
    fn prepare_next(&self, path: &Path, track_id: Option<String>) -> EngineResult<()>;

    /// Drop any previously prepared next track.
    fn clear_pending_next(&self) -> EngineResult<()>;
}
