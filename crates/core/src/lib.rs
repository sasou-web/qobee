//! Qobee orchestration layer.
//!
//! Glues the audio engine to the library: knows how to translate a
//! `track_id` (from [`qobee_library`]) into a file path, asks the engine
//! to play it, and merges engine events with playback control state
//! (queue, current track) before re-emitting them to consumers.
//!
//! The frontend never talks to the engine or the library directly: it
//! talks to a [`Player`] that owns both.

pub mod player;
pub mod queue;

pub use player::{Player, PlayerError, PlayerHandle};
pub use queue::Queue;

use serde::{Deserialize, Serialize};

/// Events emitted by [`Player`] to the application layer (which then
/// re-publishes them as Tauri events).
///
/// Tagged with `serde(tag = "type")` for a cleaner JSON shape on the
/// frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerEvent {
    /// State of the engine + currently selected track changed.
    StateChanged {
        state: qobee_engine::PlayerState,
    },
    /// Position update emitted at decoder cadence.
    Position { position_seconds: f64 },
    /// Current track finished playing.
    EndOfTrack,
    /// Recoverable engine error that the UI should surface.
    Error { message: String },
}
