//! Qobee orchestration layer.
//!
//! Glues the audio engine to the library: knows how to translate a
//! `track_id` (from [`qobee_library`]) into a file path, asks the engine
//! to play it, and merges engine events with playback control state
//! (queue, current track) before re-emitting them to consumers.
//!
//! The frontend never talks to the engine or the library directly: it
//! talks to a [`Player`] that owns both.

pub mod audio_settings;
pub mod diagnostic;
pub mod player;
pub mod queue;

pub use audio_settings::{AudioSettingsApply, AudioSettingsError, AudioSettingsStore};
pub use diagnostic::{
    cancel_null_test, run_null_test, NullTestConclusion, NullTestReport, NullTestSource,
};
pub use player::{Player, PlayerError, PlayerHandle};
pub use queue::Queue;

use serde::{Deserialize, Serialize};

/// ReplayGain mode used by the player when starting a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReplayGainMode {
    /// Don't apply any normalization.
    #[default]
    Off,
    /// Apply per-track gain. Most consistent perceived loudness across
    /// a shuffled queue.
    Track,
    /// Apply per-album gain. Preserves the relative dynamics within an
    /// album (the artist's intended balance between tracks).
    Album,
}

impl ReplayGainMode {
    pub fn from_setting(s: &str) -> Self {
        match s {
            "track" => ReplayGainMode::Track,
            "album" => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        }
    }
    pub fn as_setting(&self) -> &'static str {
        match self {
            ReplayGainMode::Off => "off",
            ReplayGainMode::Track => "track",
            ReplayGainMode::Album => "album",
        }
    }
}

/// Events emitted by [`Player`] to the application layer (which then
/// re-publishes them as Tauri events).
///
/// Tagged with `serde(tag = "type")` for a cleaner JSON shape on the
/// frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerEvent {
    /// State of the engine + currently selected track changed.
    StateChanged { state: qobee_engine::PlayerState },
    /// Position update emitted at decoder cadence.
    Position { position_seconds: f64 },
    /// Current track finished playing.
    EndOfTrack,
    /// Aggregated bit-perfect health snapshot republished from the
    /// engine's [`qobee_engine::EngineEvent::BitPerfectChanged`].
    /// Emitted as the dedicated `player:bit-perfect` Tauri topic so
    /// the UI panel can react without parsing the heavier
    /// `StateChanged` payload (R5.5).
    BitPerfectChanged {
        health: qobee_engine::BitPerfectHealth,
    },
    /// Recoverable engine error that the UI should surface.
    Error { message: String },
}
