//! Player orchestration: ties the audio engine to the library and
//! exposes a unified surface to the application layer.
//!
//! The player owns a [`CpalSharedEngine`] (always) plus a lazily-built
//! [`WasapiExclusiveEngine`] on Windows. The active backend is held
//! behind a `Mutex<Arc<dyn AudioEngine>>` and swapped when the user
//! changes the output mode setting. It pumps engine events on a
//! worker thread, enriches them with the current track id, and
//! re-publishes them as [`crate::PlayerEvent`].
//!
//! ## Smoothness improvements
//!
//! - **Pre-fetch**: when a track gets close to the end the next track
//!   is opened in advance (Symphonia probe + decoder build) so the
//!   transition is immediate. Symphonia's open path is the slowest
//!   step on a slow disk; doing it during the previous track's tail
//!   keeps the gap below human perception.
//! - **Auto-advance**: the engine's `EndOfTrack` event triggers the
//!   next queued track without waiting for a user click.
//! - **Recently-played**: every new track start is recorded in the
//!   library DB so the home page reflects what the user is listening
//!   to right now.
//! - **ReplayGain**: per-track or per-album normalization applied
//!   pre-EQ at track start, mode persisted in the settings table.
//! - **Auto-fallback to Shared** if WASAPI Exclusive can't open the
//!   device (busy, unsupported format, etc.). The user gets a soft
//!   warning toast instead of a stuck red banner, and the persisted
//!   mode is rewritten so the next launch doesn't fail the same way.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use thiserror::Error;

use qobee_engine::{
    backend_cpal_shared::{list_output_devices, CpalSharedEngine},
    AudioEngine, EngineError, EngineEvent, OutputDevice, OutputMode, PlayerState,
};
#[cfg(target_os = "windows")]
use qobee_engine::backend_wasapi_exclusive::WasapiExclusiveEngine;
use qobee_library::{Library, LibraryError};

use crate::queue::{Queue, QueueSnapshot, RepeatMode, TrackId};
use crate::{PlayerEvent, ReplayGainMode};

#[derive(Debug, Error)]
pub enum PlayerError {
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    #[error("library error: {0}")]
    Library(#[from] LibraryError),
    #[error("track not found: {0}")]
    TrackNotFound(TrackId),
    #[error("queue is empty")]
    QueueEmpty,
}

pub type PlayerResult<T> = Result<T, PlayerError>;

#[derive(Clone)]
pub struct PlayerHandle {
    inner: Arc<PlayerInner>,
}

struct PlayerInner {
    /// Shared output (CPAL → WASAPI Shared on Windows). Always built;
    /// it's the default and the fallback when Exclusive can't open
    /// the device.
    shared_engine: Arc<CpalSharedEngine>,
    /// WASAPI Exclusive engine (Windows-only). Built lazily the
    /// first time the user enables Exclusive mode so app startup
    /// stays fast for the 99 % of users who never touch it.
    #[cfg(target_os = "windows")]
    exclusive_engine: Mutex<Option<Arc<WasapiExclusiveEngine>>>,
    /// The currently active backend, behind a lock so we can swap it
    /// from a setting change without racing with calls in flight.
    /// Reads use `engine()` to grab a clone of the Arc cheaply.
    active_engine: Mutex<Arc<dyn AudioEngine>>,
    /// Currently active backend: 0 = Shared, 1 = Exclusive. Mirrors
    /// `active_engine` but is cheap to read for snapshots.
    active_backend: AtomicU8,
    library: Library,
    queue: Queue,
    current_track_id: Mutex<Option<TrackId>>,
    /// Track id we have already "warmed" (probe + first-bytes touch).
    /// Reset on each new playback start.
    prefetched_track: Mutex<Option<TrackId>>,
    /// "Endless" mode: when reaching the end of the queue (either via
    /// auto-advance or a manual `next()`), the player picks a random
    /// album and starts it instead of stopping. Toggled from the UI's
    /// "Up next" panel.
    endless: AtomicBool,
    /// ReplayGain mode encoded as `u8` (0=Off, 1=Track, 2=Album) so
    /// every read is a relaxed atomic. Mirrors the Tauri setting.
    replaygain_mode: AtomicU8,
    event_tx: Sender<PlayerEvent>,
    event_rx: Receiver<PlayerEvent>,
}

impl PlayerInner {
    /// Returns the engine currently routing audio. Cheap: clones an
    /// `Arc<dyn AudioEngine>` (refcount bump only).
    fn engine(&self) -> Arc<dyn AudioEngine> {
        Arc::clone(&self.active_engine.lock())
    }

    fn resolve_path(&self, track_id: TrackId) -> PlayerResult<PathBuf> {
        let track = self
            .library
            .get_track(track_id)?
            .ok_or(PlayerError::TrackNotFound(track_id))?;
        Ok(PathBuf::from(track.path))
    }

    fn start_track(&self, track_id: TrackId, path: PathBuf) -> PlayerResult<()> {
        // Apply ReplayGain before loading the track so the new pre-gain
        // is in effect from the very first sample. Falls back to 0 dB
        // when the file has no RG tag or the mode is "off".
        let track = self
            .library
            .get_track(track_id)?
            .ok_or(PlayerError::TrackNotFound(track_id))?;
        let mode = match self.replaygain_mode.load(Ordering::Relaxed) {
            1 => ReplayGainMode::Track,
            2 => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        };
        let rg_db = match mode {
            ReplayGainMode::Off => None,
            ReplayGainMode::Track => track
                .replaygain_track_db
                .or(track.replaygain_album_db),
            ReplayGainMode::Album => track
                .replaygain_album_db
                .or(track.replaygain_track_db),
        };
        let pre_gain_linear = match rg_db {
            Some(db) => 10f32.powf(db.clamp(-24.0, 12.0) / 20.0),
            None => 1.0,
        };
        self.engine().set_pre_gain(pre_gain_linear);
        if let Some(db) = rg_db {
            tracing::debug!(
                target: "qobee::core",
                track_id,
                rg_db = db,
                pre_gain = pre_gain_linear,
                "ReplayGain pre-gain applied"
            );
        }

        *self.current_track_id.lock() = Some(track_id);
        *self.prefetched_track.lock() = None;
        // The engine may still be holding a prepared next track from
        // before this manual jump (e.g. user pressed Next during the
        // last 3 seconds of a song). Drop it so it doesn't get
        // gapless-swapped after the explicit Load below.
        let _ = self.engine().clear_pending_next();
        self.engine().set_current_track_id(Some(track_id.to_string()));
        self.engine().load(&path)?;
        self.engine().play()?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Err(e) = self.library.record_play(track_id, now) {
            tracing::warn!(target: "qobee::core", error = %e, "record_play failed");
        }
        Ok(())
    }

    /// Try to advance to the next track in the queue.
    fn advance(&self) -> PlayerResult<bool> {
        match self.queue.advance() {
            Some(id) => {
                let path = self.resolve_path(id)?;
                self.start_track(id, path)?;
                Ok(true)
            }
            None => {
                self.engine().stop()?;
                *self.current_track_id.lock() = None;
                Ok(false)
            }
        }
    }

    /// Prepare the next queued track for a gapless transition.
    /// Calls the engine's `prepare_next` so the file is opened and
    /// (when the format matches) the active decoder thread can swap
    /// in place at end-of-stream — no audible gap. Idempotent: only
    /// prepares once per "current" track. Errors are best-effort.
    fn prefetch_next_if_any(&self) {
        let next_id = self.queue.peek_next();
        let already = *self.prefetched_track.lock();
        match (next_id, already) {
            (Some(id), Some(prev)) if prev == id => return,
            (Some(id), _) => {
                if let Ok(path) = self.resolve_path(id) {
                    if let Err(e) = self.engine().prepare_next(&path, Some(id.to_string())) {
                        tracing::debug!(
                            target: "qobee::core",
                            error = %e,
                            "prepare_next failed; will fall back to non-gapless transition"
                        );
                    }
                    *self.prefetched_track.lock() = Some(id);
                }
            }
            (None, _) => {}
        }
    }
}

pub struct Player {
    inner: Arc<PlayerInner>,
    _pump: JoinHandle<()>,
}

impl Player {
    pub fn new(library: Library) -> PlayerResult<Self> {
        let shared_engine = Arc::new(CpalSharedEngine::new()?);
        let (event_tx, event_rx) = bounded::<PlayerEvent>(256);

        let active_engine: Arc<dyn AudioEngine> =
            Arc::clone(&shared_engine) as Arc<dyn AudioEngine>;

        let inner = Arc::new(PlayerInner {
            shared_engine: Arc::clone(&shared_engine),
            #[cfg(target_os = "windows")]
            exclusive_engine: Mutex::new(None),
            active_engine: Mutex::new(active_engine),
            active_backend: AtomicU8::new(0),
            library,
            queue: Queue::new(),
            current_track_id: Mutex::new(None),
            prefetched_track: Mutex::new(None),
            endless: AtomicBool::new(true),
            replaygain_mode: AtomicU8::new(0),
            event_tx,
            event_rx,
        });

        // Pump events from the Shared engine. When the user toggles
        // Exclusive, we spawn a second pump thread for that engine —
        // both write to the same player event channel so the UI sees
        // a unified stream of state updates.
        let pump_inner = Arc::clone(&inner);
        let engine_events = shared_engine.subscribe_events();
        let pump = thread::Builder::new()
            .name("qobee-core-pump-shared".into())
            .spawn(move || pump_engine_events(pump_inner, engine_events))
            .map_err(|e| PlayerError::Engine(EngineError::Internal(e.to_string())))?;

        Ok(Player { inner, _pump: pump })
    }

    pub fn handle(&self) -> PlayerHandle {
        PlayerHandle {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl PlayerHandle {
    pub fn subscribe(&self) -> Receiver<PlayerEvent> {
        self.inner.event_rx.clone()
    }

    pub fn state(&self) -> PlayerState {
        let mut state = self.inner.engine().state();
        state.current_track_id = self
            .inner
            .current_track_id
            .lock()
            .map(|id| id.to_string());
        state
    }

    pub fn output_mode(&self) -> qobee_engine::types::EffectiveOutputMode {
        self.state().output_mode
    }

    /// User-facing output mode (Auto / Shared / Exclusive). Mirrors
    /// the persisted setting; only the *effective* mode is reported by
    /// `state().output_mode`.
    pub fn current_output_mode(&self) -> OutputMode {
        match self.inner.active_backend.load(Ordering::Acquire) {
            1 => OutputMode::Exclusive,
            _ => OutputMode::Shared,
        }
    }

    pub fn play_track(&self, track_id: TrackId) -> PlayerResult<()> {
        // Click-on-row plays the surrounding album so the listener
        // gets album-flow by default. We fall back to single-track
        // playback if the album lookup fails.
        if let Ok(Some(detail)) = self.inner.library.get_album_for_track(track_id) {
            if !detail.tracks.is_empty() {
                let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
                let start = ids.iter().position(|id| *id == track_id);
                self.inner.queue.replace(ids, start);
                let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
                let path = self.inner.resolve_path(cur)?;
                return self.inner.start_track(cur, path);
            }
        }
        let path = self.inner.resolve_path(track_id)?;
        self.inner.queue.replace_with_track(track_id);
        self.inner.start_track(track_id, path)
    }

    pub fn play_album_from_track(
        &self,
        album_id: i64,
        start_track_id: TrackId,
    ) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_album(album_id)?
            .ok_or(PlayerError::TrackNotFound(start_track_id))?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        let start = ids.iter().position(|id| *id == start_track_id);
        self.inner.queue.replace(ids, start);
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    pub fn play_playlist_from_track(
        &self,
        playlist_id: i64,
        start_track_id: TrackId,
    ) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_playlist(playlist_id)?
            .ok_or(PlayerError::TrackNotFound(start_track_id))?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        let start = ids.iter().position(|id| *id == start_track_id);
        self.inner.queue.replace(ids, start);
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    pub fn play_tracks(&self, track_ids: Vec<TrackId>, start: Option<usize>) -> PlayerResult<()> {
        if track_ids.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        self.inner.queue.replace(track_ids, start);
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    /// Insert `track_ids` right after the currently-playing track. If
    /// nothing is currently playing, fall back to starting playback.
    pub fn play_next(&self, track_ids: Vec<TrackId>) -> PlayerResult<()> {
        if track_ids.is_empty() {
            return Ok(());
        }
        if self.inner.current_track_id.lock().is_none() || self.inner.queue.is_empty() {
            // Nothing is currently playing; behave like a normal play.
            return self.play_tracks(track_ids, Some(0));
        }
        self.inner.queue.play_next(track_ids);
        Ok(())
    }

    /// Append `track_ids` at the end of the current queue.
    pub fn add_to_queue(&self, track_ids: Vec<TrackId>) -> PlayerResult<()> {
        if track_ids.is_empty() {
            return Ok(());
        }
        if self.inner.current_track_id.lock().is_none() || self.inner.queue.is_empty() {
            return self.play_tracks(track_ids, Some(0));
        }
        self.inner.queue.append(track_ids);
        Ok(())
    }

    /// Snapshot of the queue (live order + cursor). Used by the UI to
    /// render the "Now Playing / Queue" screen.
    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.inner.queue.snapshot()
    }

    /// Remove the queue entry at `idx`. If the entry is the currently
    /// playing track, playback advances to the next item; if the
    /// queue empties out, the engine is stopped.
    pub fn queue_remove_at(&self, idx: usize) -> PlayerResult<()> {
        let snap = self.inner.queue.snapshot();
        let was_current = snap.cursor == Some(idx);
        let removed = self.inner.queue.remove_at(idx);
        if !removed {
            return Ok(());
        }
        if was_current {
            // The cursor now points to whatever sat *after* the
            // removed track (or stayed at the same numeric index,
            // which becomes the next track). Restart playback there.
            match self.inner.queue.current() {
                Some(id) => {
                    let path = self.inner.resolve_path(id)?;
                    self.inner.start_track(id, path)?;
                }
                None => {
                    self.inner.engine().stop()?;
                    *self.inner.current_track_id.lock() = None;
                }
            }
        }
        Ok(())
    }

    /// Reorder the queue by moving the entry at `from` to `to`. The
    /// currently playing track is preserved (its index is updated to
    /// follow the move).
    pub fn queue_move(&self, from: usize, to: usize) -> PlayerResult<()> {
        self.inner.queue.move_item(from, to);
        Ok(())
    }

    /// Jump straight to the queue entry at `idx`, restarting playback
    /// there. Equivalent to clicking a row in the "up next" panel.
    pub fn queue_jump_to(&self, idx: usize) -> PlayerResult<()> {
        let snap = self.inner.queue.snapshot();
        if idx >= snap.items.len() {
            return Err(PlayerError::QueueEmpty);
        }
        let id = snap.items[idx];
        // `replace` rebuilds the queue identically and moves the cursor
        // — cheap and avoids exposing internal cursor mutation on
        // `Queue`. The order is preserved because we feed back the
        // same list.
        self.inner.queue.replace(snap.items, Some(idx));
        let path = self.inner.resolve_path(id)?;
        self.inner.start_track(id, path)
    }

    pub fn pause(&self) -> PlayerResult<()> {
        self.inner.engine().pause()?;
        Ok(())
    }

    pub fn resume(&self) -> PlayerResult<()> {
        self.inner.engine().resume()?;
        Ok(())
    }

    pub fn stop(&self) -> PlayerResult<()> {
        self.inner.engine().stop()?;
        *self.inner.current_track_id.lock() = None;
        Ok(())
    }

    pub fn seek(&self, position_seconds: f64) -> PlayerResult<()> {
        self.inner.engine().seek(position_seconds)?;
        Ok(())
    }

    pub fn set_volume(&self, volume: f32) -> PlayerResult<()> {
        self.inner.engine().set_volume(volume)?;
        Ok(())
    }

    pub fn set_output_mode(&self, mode: OutputMode) -> PlayerResult<()> {
        // Map user-facing OutputMode to active backend index.
        // Auto + Shared -> backend 0 (Shared). Exclusive -> backend 1.
        let want_exclusive = matches!(mode, OutputMode::Exclusive);

        #[cfg(target_os = "windows")]
        {
            if want_exclusive {
                self.activate_exclusive_backend()?;
            } else {
                self.activate_shared_backend();
            }
            // Inform the active engine of the mode for any
            // backend-internal book-keeping.
            self.inner.engine().set_output_mode(mode)?;
            return Ok(());
        }

        // Non-Windows: only Shared is meaningful. We accept the call
        // silently so the UI doesn't have to special-case the platform.
        #[cfg(not(target_os = "windows"))]
        {
            let _ = want_exclusive;
            self.inner.engine().set_output_mode(mode)?;
            Ok(())
        }
    }

    #[cfg(target_os = "windows")]
    fn activate_shared_backend(&self) {
        if self.inner.active_backend.load(Ordering::Acquire) == 0 {
            return;
        }
        // Stop whatever is currently playing on the active engine.
        let _ = self.inner.engine().stop();
        // Swap: Shared becomes active.
        let shared: Arc<dyn AudioEngine> =
            Arc::clone(&self.inner.shared_engine) as Arc<dyn AudioEngine>;
        *self.inner.active_engine.lock() = shared;
        self.inner.active_backend.store(0, Ordering::Release);
        *self.inner.current_track_id.lock() = None;
        tracing::info!(target: "qobee::core", "switched to Shared backend");
    }

    #[cfg(target_os = "windows")]
    fn activate_exclusive_backend(&self) -> PlayerResult<()> {
        if self.inner.active_backend.load(Ordering::Acquire) == 1 {
            return Ok(());
        }
        // Lazy-init the Exclusive engine.
        let mut slot = self.inner.exclusive_engine.lock();
        if slot.is_none() {
            let eng = Arc::new(WasapiExclusiveEngine::new()?);
            // Pump its events into the same player channel.
            let pump_inner = Arc::clone(&self.inner);
            let events = eng.subscribe_events();
            std::thread::Builder::new()
                .name("qobee-core-pump-exclusive".into())
                .spawn(move || pump_engine_events(pump_inner, events))
                .map_err(|e| PlayerError::Engine(EngineError::Internal(e.to_string())))?;
            // Mirror persistent device + EQ + pre-gain settings.
            let dev = self.inner.shared_engine.selected_device();
            eng.set_output_device(dev);
            let gains = self.inner.shared_engine.eq_gains_db();
            eng.set_eq_gains_db(&gains);
            *slot = Some(eng);
        }
        let eng = Arc::clone(slot.as_ref().unwrap());
        drop(slot);

        // Stop what's playing on Shared.
        let _ = self.inner.shared_engine.stop();

        let dyn_eng: Arc<dyn AudioEngine> = eng as Arc<dyn AudioEngine>;
        *self.inner.active_engine.lock() = dyn_eng;
        self.inner.active_backend.store(1, Ordering::Release);
        *self.inner.current_track_id.lock() = None;
        tracing::info!(target: "qobee::core", "switched to WASAPI Exclusive backend");
        Ok(())
    }

    pub fn next(&self) -> PlayerResult<()> {
        // Same auto-fill semantics as the engine's EndOfTrack handler:
        // when there's nothing left in the queue we pick a random
        // album and start it, instead of stopping. Keeps the listening
        // session going whether the user skips or the track ends on
        // its own. Honors the "endless" toggle.
        if self.inner.advance()? {
            return Ok(());
        }
        if !self.inner.endless.load(Ordering::Relaxed) {
            return Ok(());
        }
        match self.play_random_album() {
            Ok(()) => Ok(()),
            Err(PlayerError::QueueEmpty) => {
                // Nothing in the library to fall back to; just stop.
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Whether endless mode is currently on. UI reads this to mirror
    /// the toggle state.
    pub fn endless(&self) -> bool {
        self.inner.endless.load(Ordering::Relaxed)
    }

    pub fn set_endless(&self, on: bool) {
        self.inner.endless.store(on, Ordering::Relaxed);
    }

    pub fn previous(&self) -> PlayerResult<()> {
        match self.inner.queue.previous() {
            Some(id) => {
                let path = self.inner.resolve_path(id)?;
                self.inner.start_track(id, path)
            }
            None => Ok(()),
        }
    }

    // ---- Repeat / shuffle ----

    pub fn repeat_mode(&self) -> RepeatMode {
        self.inner.queue.repeat()
    }

    pub fn set_repeat_mode(&self, mode: RepeatMode) {
        self.inner.queue.set_repeat(mode);
    }

    pub fn shuffle(&self) -> bool {
        self.inner.queue.shuffle()
    }

    pub fn set_shuffle(&self, on: bool) {
        self.inner.queue.set_shuffle(on);
    }

    /// Pick a random album from the library and start playing it from
    /// track 1. Used by the home page "shuffle library" CTA and by the
    /// auto-fill behavior at end of queue.
    pub fn play_random_album(&self) -> PlayerResult<()> {
        let albums = self.inner.library.list_albums()?;
        if albums.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        // Lehmer-style RNG (no extra crate) seeded on time.
        let now_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15);
        let idx = (now_nanos as usize) % albums.len();
        let album = &albums[idx];
        let detail = self
            .inner
            .library
            .get_album(album.id)?
            .ok_or(PlayerError::QueueEmpty)?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        let first = ids[0];
        self.inner.queue.replace(ids, Some(0));
        let path = self.inner.resolve_path(first)?;
        self.inner.start_track(first, path)
    }

    // ---- EQ ----

    pub fn set_eq_gains_db(&self, gains: Vec<f32>) {
        self.inner.engine().set_eq_gains_db(&gains);
    }

    pub fn eq_gains_db(&self) -> Vec<f32> {
        self.inner.engine().eq_gains_db()
    }

    // ---- Output device picking ----

    pub fn list_output_devices(&self) -> PlayerResult<Vec<OutputDevice>> {
        Ok(list_output_devices()?)
    }

    pub fn set_output_device(&self, device_id: Option<String>) -> PlayerResult<()> {
        self.inner.engine().set_output_device(device_id);
        Ok(())
    }

    pub fn selected_output_device(&self) -> Option<String> {
        self.inner.engine().selected_device()
    }

    // ---- ReplayGain ----

    pub fn replaygain_mode(&self) -> ReplayGainMode {
        match self.inner.replaygain_mode.load(Ordering::Relaxed) {
            1 => ReplayGainMode::Track,
            2 => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        }
    }

    pub fn set_replaygain_mode(&self, mode: ReplayGainMode) {
        let v = match mode {
            ReplayGainMode::Off => 0,
            ReplayGainMode::Track => 1,
            ReplayGainMode::Album => 2,
        };
        self.inner.replaygain_mode.store(v, Ordering::Relaxed);

        // Re-apply gain to the currently playing track immediately, so
        // toggling the setting takes effect without waiting for the
        // next track.
        if let Some(track_id) = *self.inner.current_track_id.lock() {
            if let Ok(Some(track)) = self.inner.library.get_track(track_id) {
                let rg_db = match mode {
                    ReplayGainMode::Off => None,
                    ReplayGainMode::Track => track
                        .replaygain_track_db
                        .or(track.replaygain_album_db),
                    ReplayGainMode::Album => track
                        .replaygain_album_db
                        .or(track.replaygain_track_db),
                };
                let pre_gain = match rg_db {
                    Some(db) => 10f32.powf(db.clamp(-24.0, 12.0) / 20.0),
                    None => 1.0,
                };
                self.inner.engine().set_pre_gain(pre_gain);
            }
        }
    }
}

/// Shorten a raw engine error message into something nicer for a
/// toast: drop the parenthesised HRESULT if any, keep the human part.
#[cfg(target_os = "windows")]
fn shorten_error(msg: &str) -> String {
    // Strip an optional " HRESULT 0x..." or "(0x...)" tail.
    let cut = msg
        .find(" HRESULT")
        .or_else(|| msg.find(" (0x"))
        .unwrap_or(msg.len());
    msg[..cut].trim_end_matches([':', ' ']).to_string()
}

/// Pump engine events into the player event channel.
fn pump_engine_events(inner: Arc<PlayerInner>, engine_events: Receiver<EngineEvent>) {
    while let Ok(ev) = engine_events.recv() {
        let player_ev = match ev {
            EngineEvent::StateChanged { mut state } => {
                state.current_track_id = inner.current_track_id.lock().map(|id| id.to_string());
                PlayerEvent::StateChanged { state }
            }
            EngineEvent::Position { position_seconds } => {
                // Pre-fetch the next track when we're within 5s of the
                // end of the current one, so the gap on EndOfTrack is
                // dominated by the engine's load() and not by disk I/O.
                let dur = inner.engine().state().duration_seconds;
                if dur > 0.0 && position_seconds >= dur - 5.0 {
                    inner.prefetch_next_if_any();
                }
                PlayerEvent::Position { position_seconds }
            }
            EngineEvent::EndOfTrack => {
                match inner.advance() {
                    Ok(true) => {}
                    Ok(false) => {
                        if inner.endless.load(Ordering::Relaxed) {
                            let me = PlayerHandle {
                                inner: Arc::clone(&inner),
                            };
                            if let Err(e) = me.play_random_album() {
                                tracing::info!(
                                    target: "qobee::core",
                                    error = %e,
                                    "no random album available; staying idle"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        tracing::warn!(
                            target: "qobee::core",
                            error = %msg,
                            "auto-advance failed"
                        );
                        let _ = inner.event_tx.try_send(PlayerEvent::Error { message: msg });
                    }
                }
                PlayerEvent::EndOfTrack
            }
            EngineEvent::GaplessTransition => {
                // The engine already swapped the active decoder. We
                // only update bookkeeping: advance the queue cursor
                // (without calling start_track), record the play in
                // history, and clear the prefetch flag so we can
                // prepare the *next* track.
                if let Some(new_id) = inner.queue.advance() {
                    *inner.current_track_id.lock() = Some(new_id);
                    inner
                        .engine()
                        .set_current_track_id(Some(new_id.to_string()));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if let Err(e) = inner.library.record_play(new_id, now) {
                        tracing::warn!(
                            target: "qobee::core",
                            error = %e,
                            "record_play failed (gapless)"
                        );
                    }
                }
                *inner.prefetched_track.lock() = None;

                // Tell the UI we transitioned. Re-fetch the engine
                // state so the new track's metadata reaches the UI
                // even if it raced with the StateChanged emitted
                // by the engine.
                let mut state = inner.engine().state();
                state.current_track_id = inner.current_track_id.lock().map(|id| id.to_string());
                let _ = inner.event_tx.try_send(PlayerEvent::StateChanged { state });

                PlayerEvent::EndOfTrack
            }
            EngineEvent::Error { message } => {
                // If WASAPI Exclusive can't open the device, drop back
                // to Shared automatically and retry the current track.
                // This keeps the user's session going even when their
                // device refuses every Exclusive format we try.
                #[cfg(target_os = "windows")]
                {
                    let active = inner.active_backend.load(Ordering::Acquire);
                    let looks_like_exclusive_fail = active == 1
                        && (message.contains("Exclusive")
                            || message.contains("WASAPI"));
                    if looks_like_exclusive_fail {
                        tracing::warn!(
                            target: "qobee::core",
                            error = %message,
                            "WASAPI Exclusive failed; falling back to Shared automatically"
                        );
                        // Swap to Shared on the active engine slot.
                        let shared: Arc<dyn AudioEngine> =
                            Arc::clone(&inner.shared_engine) as Arc<dyn AudioEngine>;
                        *inner.active_engine.lock() = shared;
                        inner.active_backend.store(0, Ordering::Release);

                        // Persist the fallback so the next launch
                        // doesn't try Exclusive again and fail in the
                        // same way. The user can re-enable it from
                        // Settings if they fix their device config.
                        if let Err(e) = inner
                            .library
                            .set_setting("audio.output_mode", "shared")
                        {
                            tracing::warn!(
                                target: "qobee::core",
                                error = %e,
                                "failed to persist fallback output_mode"
                            );
                        }

                        // Restart the same track on Shared.
                        let cur_id = *inner.current_track_id.lock();
                        if let Some(track_id) = cur_id {
                            if let Ok(path) = inner.resolve_path(track_id) {
                                let _ = inner.start_track(track_id, path);
                            }
                        }

                        // Tell the UI we fell back so the dropdown can
                        // sync. We surface a soft warning rather than
                        // the raw HRESULT.
                        let _ = inner.event_tx.try_send(PlayerEvent::Error {
                            message: format!(
                                "Exclusive output unavailable: {}. Switched to Shared.",
                                shorten_error(&message)
                            ),
                        });
                        continue;
                    }
                }

                PlayerEvent::Error { message }
            }
        };
        if inner.event_tx.try_send(player_ev).is_err() {
            tracing::warn!(
                target: "qobee::core",
                "player event channel saturated; dropping engine event"
            );
        }
    }
}
