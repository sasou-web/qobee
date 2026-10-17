//! Shared engine types: output device descriptor, playback status,
//! player state snapshot, and engine events.
//!
//! The audiophile types (output modes, bit-perfect health, DSD, upmix,
//! PCM buffer variants) were removed when the engine was rebuilt on
//! `rodio`. What remains is the minimal surface the simplified player
//! needs.

use serde::{Deserialize, Serialize};

/// Description of an output endpoint the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputDevice {
    /// Stable identifier (CPAL device name). Matches what the UI passes
    /// back when calling `set_output_device`.
    pub id: String,
    /// Human-readable label. Usually identical to `id`.
    pub name: String,
    /// True if this is the system default at the time of the call.
    pub is_default: bool,
    /// Native sample rate the device's default config exposes (Hz).
    pub default_sample_rate: u32,
    /// Channel count of the device's default config.
    pub channels: u16,
}

/// High-level playback status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Stopped,
    Errored,
}

/// Format descriptor for the currently playing track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackFormat {
    pub sample_rate: u32,
    pub channels: u16,
    /// Bit depth as reported by the decoder, when known.
    pub bit_depth: Option<u8>,
}

/// Snapshot of player state, suitable for serialization to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    /// Identifier of the currently loaded track, if any. Opaque to the
    /// engine; assigned by [`qobee-core`](../qobee_core/index.html).
    pub current_track_id: Option<String>,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    /// Current software volume in `[0.0, 1.0]`.
    pub volume: f32,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u16>,
    /// Last error message, when [`PlayerState::status`] is
    /// [`PlaybackStatus::Errored`].
    pub error: Option<String>,
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            status: PlaybackStatus::Idle,
            current_track_id: None,
            position_seconds: 0.0,
            duration_seconds: 0.0,
            volume: 1.0,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            error: None,
        }
    }
}

/// Events emitted by the engine to subscribers (the orchestration layer
/// re-publishes them to the UI as Tauri events).
///
/// Tagged with `serde(tag = "type")` so the UI receives a discriminated
/// union it can pattern-match on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    StateChanged { state: PlayerState },
    Position { position_seconds: f64 },
    EndOfTrack,
    Error { message: String },
}
