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
pub mod sync;

pub use audio_settings::{AudioSettingsApply, AudioSettingsError, AudioSettingsStore};
pub use diagnostic::{
    cancel_null_test, run_null_test, NullTestConclusion, NullTestReport, NullTestSource,
};
pub use player::{Player, PlayerError, PlayerHandle};
pub use queue::Queue;
pub use sync::MediaBridge;

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
///
/// ## Two flavours
///
/// The new `Started` / `Paused` / `Resumed` / `Stopped` /
/// `TrackChanged` / `PositionTick` / `Errored` variants form the
/// **R8 transport bus** (cf. spec `native-media-integration-and-ux`).
/// They are broadcast on the `tokio::sync::broadcast` channel
/// exposed by [`PlayerHandle::subscribe_broadcast`] and are the
/// single source of truth for the UI store, the Windows SMTC
/// bridge and the macOS MPNowPlayingInfoCenter bridge.
///
/// The legacy variants (`StateChanged`, `Position`, `EndOfTrack`,
/// `BitPerfectChanged`, `Error`) are kept on the same enum so the
/// existing crossbeam consumers (`commands_drive`, `null_test`,
/// the bit-perfect topic) keep working without churn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerEvent {
    // --- R8 transport bus (broadcast) ---------------------------------
    /// Playback just started for `track`. Broadcast once per track,
    /// when the engine transitions from `Idle | Stopped | Loading`
    /// straight to `Playing`.
    Started {
        track: TrackMeta,
        position_ms: u64,
        duration_ms: u64,
    },
    /// Playback paused. `position_ms` is the position at which the
    /// pause occurred so the OS bridges can freeze their timeline.
    Paused { position_ms: u64 },
    /// Playback resumed from a previous `Paused` state.
    Resumed { position_ms: u64 },
    /// Playback stopped (queue ended, engine stopped, hard error).
    Stopped,
    /// The active track changed (gapless transition, manual `next`,
    /// `previous`, queue jump). Broadcast separately from `Started`
    /// because pre-existing state (volume / EQ / device) is
    /// preserved across the transition.
    TrackChanged { track: TrackMeta },
    /// Position tick, throttled to one event every 250 ms in the
    /// player pump. The unique source of progress information for
    /// the UI and the OS bridges.
    PositionTick { position_ms: u64 },
    /// Recoverable engine / orchestration error scoped to a
    /// specific track when known. The message is FR-localised when
    /// it originates from the orchestrator.
    Errored {
        kind: PlayerErrorKind,
        message: String,
    },

    // --- Legacy variants ------------------------------------------------
    /// State of the engine + currently selected track changed.
    StateChanged { state: qobee_engine::PlayerState },
    /// Position update emitted at decoder cadence (legacy: not
    /// throttled).
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
    /// Recoverable engine error that the UI should surface (legacy).
    Error { message: String },
}

/// Track metadata projected onto the R8 transport bus.
///
/// Carries everything the UI store and the OS bridges need to
/// render a "Now Playing" surface without going back to the library
/// layer. `cover_bytes` is annotated `#[serde(skip)]` so it never
/// reaches the frontend through Tauri (the cover host already
/// streams cached covers via HTTP); only the in-process bridges
/// (SMTC / MPNowPlayingInfoCenter) consume the bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackMeta {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    /// Raw bytes of the embedded cover, when extracted by the
    /// scanner. Skipped on the wire (Tauri / IPC) so the frontend
    /// keeps using the cover host; consumed in-process by the OS
    /// bridges for `Thumbnail` / `MPMediaItemArtwork`.
    #[serde(skip)]
    pub cover_bytes: Option<Vec<u8>>,
}

/// Coarse classification of recoverable player errors broadcast on
/// the R8 transport bus. The UI uses the `kind` discriminator to
/// pick a specific FR-localised toast / aria-label without parsing
/// the free-form message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerErrorKind {
    /// The audio file could not be opened (missing on disk, bad
    /// permissions, dangling library row).
    FileNotFound,
    /// Symphonia / decoder backend rejected the file.
    DecodeFailed,
    /// The audio device became unavailable mid-playback (busy,
    /// disconnected, refused the requested format).
    DeviceUnavailable,
    /// Catch-all for anything else surfaced as `EngineError` /
    /// `LibraryError`.
    Other,
}
