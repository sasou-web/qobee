//! Shared engine types: PCM buffer enum, output mode, player state, events.

use serde::{Deserialize, Serialize};

/// User-facing output mode. Kept as an enum (not a single variant) so
/// the API can be extended without breaking the IPC layer (e.g. an ASIO
/// backend in the future).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Pick whatever is available (currently always Shared).
    Auto,
    /// OS-mixer path (CPAL / WASAPI Shared on Windows). Supports
    /// concurrent playback with other applications.
    Shared,
    /// WASAPI Exclusive on Windows. Bit-perfect when the device
    /// natively supports the source sample rate and bit depth.
    /// Other applications cannot play to the same device while
    /// Qobee is active.
    Exclusive,
}

impl Default for OutputMode {
    fn default() -> Self {
        OutputMode::Auto
    }
}

/// Effective output mode reported back to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveOutputMode {
    /// Running in Shared (the OS mixer is in the chain).
    Shared,
    /// Running in WASAPI Exclusive (the OS mixer is bypassed).
    Exclusive,
}

/// Description of an output endpoint the user can pick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputDevice {
    /// Stable identifier (CPAL name on Windows). Matches what the UI
    /// passes back when calling `set_output_device`.
    pub id: String,
    /// Human-readable label. May be the same as `id` on most hosts.
    pub name: String,
    /// True if this is the system default at the time of the call.
    pub is_default: bool,
    /// Native sample rate the device's default config exposes (Hz).
    pub default_sample_rate: u32,
    /// Channel count of the device's default config.
    pub channels: u16,
}

/// High-level playback status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Idle,
    Loading,
    Playing,
    Paused,
    Stopped,
    Errored,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        PlaybackStatus::Idle
    }
}

/// Format descriptor for the currently playing track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackFormat {
    pub sample_rate: u32,
    pub channels: u16,
    /// Bit depth as reported by the decoder, when known.
    pub bit_depth: Option<u8>,
}

/// PCM buffer carried through the decode → output pipeline.
///
/// Only [`PcmBuffer::F32Interleaved`] is produced by the MVP, but the
/// integer variants are part of the public API so a future bit-perfect
/// path (WASAPI Exclusive) can plug in without breaking downstream code.
#[derive(Debug, Clone)]
pub enum PcmBuffer {
    /// 32-bit float, interleaved. Used by SharedCpal.
    F32Interleaved(Vec<f32>),
    /// 16-bit signed, interleaved.
    I16Interleaved(Vec<i16>),
    /// 24-bit signed packed in i32 (low 24 bits significant), interleaved.
    /// Reserved for the WASAPI Exclusive integer path.
    I24In32Interleaved(Vec<i32>),
    /// 32-bit signed, interleaved.
    I32Interleaved(Vec<i32>),
}

impl PcmBuffer {
    /// Number of interleaved samples (frames * channels) currently held.
    pub fn len(&self) -> usize {
        match self {
            PcmBuffer::F32Interleaved(v) => v.len(),
            PcmBuffer::I16Interleaved(v) => v.len(),
            PcmBuffer::I24In32Interleaved(v) => v.len(),
            PcmBuffer::I32Interleaved(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
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
    /// Mode the engine is *actually* running in.
    pub output_mode: EffectiveOutputMode,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u16>,
    /// True only on a strictly bit-perfect chain. Always `false` in
    /// Shared mode (the OS mixer is in the path); kept on the state
    /// for forward-compatibility and so the UI can render the truth.
    pub is_bit_perfect: bool,
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
            output_mode: EffectiveOutputMode::Shared,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            is_bit_perfect: false,
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
    /// Emitted when the decoder thread swaps a prepared next track in
    /// place mid-stream (gapless transition). The audio callback never
    /// stopped: the user heard one continuous output. The orchestrator
    /// advances the queue without issuing a fresh `Load`.
    GaplessTransition,
    Error { message: String },
}
