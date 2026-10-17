//! Qobee audio engine.
//!
//! A deliberately small playback core built on
//! [`rodio`](https://github.com/RustAudio/rodio) (RustAudio), which
//! itself wraps Symphonia (decoding) and CPAL (output). The engine
//! exposes the [`AudioEngine`] trait and a single implementation,
//! [`rodio_engine::RodioEngine`], that handles loading, transport
//! (play / pause / resume / stop / seek), volume, and an output-device
//! picker.
//!
//! The previous audiophile stack — multiple output backends (CPAL
//! Shared, WASAPI Exclusive, ASIO), a 10-band EQ, ReplayGain, crossfeed,
//! an FFT convolver, dither, a limiter, balance/trim, a DSD pipeline,
//! and the bit-perfect health/null-test diagnostics — was removed in
//! favour of this minimal, maintainable core. Gapless / crossfade is
//! out of scope; a small inter-track gap is acceptable.

pub mod device;
pub mod error;
pub mod rodio_engine;
pub mod types;

pub use device::{list_output_devices, resolve_output_device};
pub use error::{EngineError, EngineResult};
pub use rodio_engine::{map_volume, new_shared, RodioEngine};
pub use types::{EngineEvent, OutputDevice, PlaybackStatus, PlayerState, TrackFormat};

use crossbeam_channel::Receiver;
use std::path::Path;

/// Common interface the audio backend satisfies. Kept intentionally
/// small: load, transport, volume, state/events, and the output-device
/// picker.
pub trait AudioEngine: Send + Sync {
    /// Open a local file path or a `drv://…` Drive URI and prepare it
    /// for playback. Returns `Err` (without panicking) on a file that
    /// cannot be opened or decoded.
    fn load(&self, path: &Path) -> EngineResult<()>;

    /// Begin (or resume from paused) playback of the loaded track.
    fn play(&self) -> EngineResult<()>;

    /// Suspend output, keeping the loaded position.
    fn pause(&self) -> EngineResult<()>;

    /// Resume output from the paused position.
    fn resume(&self) -> EngineResult<()>;

    /// Stop output and drop the loaded source.
    fn stop(&self) -> EngineResult<()>;

    /// Seek to `position_seconds` within the current track.
    fn seek(&self, position_seconds: f64) -> EngineResult<()>;

    /// Set the output volume from a slider position in `[0.0, 1.0]`.
    fn set_volume(&self, volume: f32) -> EngineResult<()>;

    /// Snapshot the current player state.
    fn state(&self) -> PlayerState;

    /// Subscribe to engine events (state changes, position ticks,
    /// end-of-track, recoverable errors).
    fn subscribe_events(&self) -> Receiver<EngineEvent>;

    // ---- Output device picker ----

    /// List the available output devices.
    fn list_output_devices(&self) -> EngineResult<Vec<OutputDevice>>;

    /// Select an output device by id (`None` = system default).
    fn set_output_device(&self, device_id: Option<String>) -> EngineResult<()>;

    /// The currently selected output device id (`None` = default).
    fn selected_output_device(&self) -> Option<String>;
}
