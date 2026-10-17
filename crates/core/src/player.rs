//! Player orchestration: ties the audio engine to the library and
//! exposes a unified surface to the application layer.
//!
//! The player owns a single [`RodioEngine`] behind an
//! `Arc<dyn AudioEngine>`. It pumps engine events on a worker thread,
//! enriches them with the current track id, and re-publishes them as
//! [`crate::PlayerEvent`] (both on the legacy crossbeam channel and on
//! the R8 `tokio::sync::broadcast` transport bus).
//!
//! Compared with the previous audiophile orchestrator, this version
//! drops the multi-backend swap (Shared / Exclusive / ASIO), ReplayGain,
//! the DSD gate, the EQ / pre-gain caches, the convolver IR plumbing,
//! gapless pre-fetch, and crossfade. What remains is the essential
//! music-player loop: resolve a track to a path, load it, play it,
//! record the play, and auto-advance the queue on end-of-track.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::broadcast;

use qobee_engine::{
    list_output_devices, new_shared, AudioEngine, EngineError, EngineEvent, OutputDevice,
    PlaybackStatus, PlayerState, RodioEngine,
};
use qobee_library::{Library, LibraryError};

use crate::queue::{Queue, QueueSnapshot, RepeatMode, TrackId};
use crate::{PlayerErrorKind, PlayerEvent, TrackMeta};

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

/// Capacity of the [`broadcast::Sender`] on the R8 transport bus.
const BROADCAST_CAPACITY: usize = 256;

/// `PositionTick` throttle window for the transport bus.
const POSITION_TICK_THROTTLE: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct PlayerHandle {
    inner: Arc<PlayerInner>,
}

struct PlayerInner {
    /// The single rodio-backed engine.
    engine: Arc<RodioEngine>,
    library: Library,
    queue: Queue,
    current_track_id: Mutex<Option<TrackId>>,
    /// "Endless" mode: when the queue runs dry, pick a random album.
    endless: AtomicBool,
    event_tx: Sender<PlayerEvent>,
    event_rx: Receiver<PlayerEvent>,
    /// R8 transport bus.
    broadcast_tx: broadcast::Sender<PlayerEvent>,
    /// Sleep timer wall-clock deadline.
    sleep_deadline: Mutex<Option<Instant>>,
    /// "Stop at end of current track" latch.
    sleep_stop_after_track: AtomicBool,
}

impl PlayerInner {
    /// The engine routing audio, as a trait object.
    fn engine(&self) -> Arc<dyn AudioEngine> {
        Arc::clone(&self.engine) as Arc<dyn AudioEngine>
    }

    fn broadcast(&self, ev: PlayerEvent) {
        let _ = self.broadcast_tx.send(ev);
    }

    /// Build a [`TrackMeta`] from a library row, reading the cover blob
    /// from the cover cache when present (for the OS media bridges).
    fn track_meta_for(&self, track_id: TrackId) -> Option<TrackMeta> {
        let track = self.library.get_track(track_id).ok().flatten()?;
        let duration_ms = (track.duration_seconds * 1000.0).max(0.0) as u64;
        let cover_bytes = track.cover_key.as_deref().and_then(|key| {
            let normalized = key.replace('/', std::path::MAIN_SEPARATOR_STR);
            let path = self.library.cover_cache_dir().join(normalized);
            std::fs::read(&path).ok()
        });
        Some(TrackMeta {
            id: track.id,
            title: track.title,
            artist: track.artist,
            album: track.album,
            duration_ms,
            cover_bytes,
        })
    }

    fn resolve_path(&self, track_id: TrackId) -> PlayerResult<PathBuf> {
        let track = self
            .library
            .get_track(track_id)?
            .ok_or(PlayerError::TrackNotFound(track_id))?;
        Ok(PathBuf::from(track.path))
    }

    /// Load and play a track: resolve → load → play → record → broadcast.
    fn start_track(&self, track_id: TrackId, path: PathBuf) -> PlayerResult<()> {
        *self.current_track_id.lock() = Some(track_id);
        self.engine().load(&path)?;
        self.engine().play()?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Err(e) = self.library.record_play(track_id, now) {
            tracing::warn!(target: "qobee::core", error = %e, "record_play failed");
        }

        if let Some(meta) = self.track_meta_for(track_id) {
            let duration_ms = meta.duration_ms;
            self.broadcast(PlayerEvent::Started {
                track: meta,
                position_ms: 0,
                duration_ms,
            });
        }
        Ok(())
    }

    /// Advance to the next track in the queue, or stop if none.
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
}

pub struct Player {
    inner: Arc<PlayerInner>,
    _pump: JoinHandle<()>,
}

impl Player {
    pub fn new(library: Library) -> PlayerResult<Self> {
        let engine = new_shared()?;
        let (event_tx, event_rx) = bounded::<PlayerEvent>(256);
        let (broadcast_tx, _) = broadcast::channel::<PlayerEvent>(BROADCAST_CAPACITY);

        let inner = Arc::new(PlayerInner {
            engine: Arc::clone(&engine),
            library,
            queue: Queue::new(),
            current_track_id: Mutex::new(None),
            endless: AtomicBool::new(true),
            event_tx,
            event_rx,
            broadcast_tx,
            sleep_deadline: Mutex::new(None),
            sleep_stop_after_track: AtomicBool::new(false),
        });

        // Pump engine events into the player event channel + bus.
        let pump_inner = Arc::clone(&inner);
        let engine_events = engine.subscribe_events();
        let pump = thread::Builder::new()
            .name("qobee-core-pump".into())
            .spawn(move || pump_engine_events(pump_inner, engine_events))
            .map_err(|e| PlayerError::Engine(EngineError::Internal(e.to_string())))?;

        // Sleep-timer monitor: a 1 s poll that pauses the engine once
        // the armed wall-clock deadline passes.
        let sleep_inner = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("qobee-core-sleep-timer".into())
            .spawn(move || loop {
                thread::sleep(Duration::from_millis(1000));
                let Some(inner) = sleep_inner.upgrade() else {
                    break;
                };
                let fire = {
                    let mut slot = inner.sleep_deadline.lock();
                    match *slot {
                        Some(deadline) if Instant::now() >= deadline => {
                            *slot = None;
                            true
                        }
                        _ => false,
                    }
                };
                if fire {
                    tracing::info!(target: "qobee::core", "sleep timer elapsed; pausing playback");
                    if let Err(e) = inner.engine().pause() {
                        tracing::warn!(target: "qobee::core", error = %e, "sleep-timer pause failed");
                    }
                }
            })
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

    /// Subscribe to the R8 transport bus.
    pub fn subscribe_broadcast(&self) -> broadcast::Receiver<PlayerEvent> {
        self.inner.broadcast_tx.subscribe()
    }

    pub fn state(&self) -> PlayerState {
        let mut state = self.inner.engine().state();
        state.current_track_id = self.inner.current_track_id.lock().map(|id| id.to_string());
        state
    }

    /// [`TrackMeta`] for the currently active track, when any.
    pub fn current_track_meta(&self) -> Option<TrackMeta> {
        let track_id = (*self.inner.current_track_id.lock())?;
        self.inner.track_meta_for(track_id)
    }

    pub fn play_track(&self, track_id: TrackId) -> PlayerResult<()> {
        // Click-on-row plays the surrounding album for album-flow by
        // default, falling back to single-track playback.
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

    /// Start playing an album from its first track.
    pub fn play_album(&self, album_id: i64) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_album(album_id)?
            .ok_or(PlayerError::QueueEmpty)?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        self.inner.queue.replace(ids, Some(0));
        let cur = self.inner.queue.current().ok_or(PlayerError::QueueEmpty)?;
        let path = self.inner.resolve_path(cur)?;
        self.inner.start_track(cur, path)
    }

    /// Start playing a playlist from its first track.
    pub fn play_playlist(&self, playlist_id: i64) -> PlayerResult<()> {
        let detail = self
            .inner
            .library
            .get_playlist(playlist_id)?
            .ok_or(PlayerError::QueueEmpty)?;
        if detail.tracks.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
        let ids: Vec<TrackId> = detail.tracks.iter().map(|t| t.id).collect();
        self.inner.queue.replace(ids, Some(0));
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

    /// Insert `track_ids` right after the currently-playing track.
    pub fn play_next(&self, track_ids: Vec<TrackId>) -> PlayerResult<()> {
        if track_ids.is_empty() {
            return Ok(());
        }
        if self.inner.current_track_id.lock().is_none() || self.inner.queue.is_empty() {
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

    /// Snapshot of the queue (live order + cursor).
    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.inner.queue.snapshot()
    }

    /// Restore a previously persisted session, leaving playback paused.
    pub fn restore_session(
        &self,
        track_ids: Vec<TrackId>,
        cursor: usize,
        position_seconds: f64,
    ) -> PlayerResult<bool> {
        let valid: Vec<TrackId> = track_ids
            .into_iter()
            .filter(|id| matches!(self.inner.library.get_track(*id), Ok(Some(_))))
            .collect();
        if valid.is_empty() {
            return Ok(false);
        }
        let start = cursor.min(valid.len() - 1);
        self.inner.queue.replace(valid, Some(start));
        let cur = match self.inner.queue.current() {
            Some(id) => id,
            None => return Ok(false),
        };
        let path = self.inner.resolve_path(cur)?;
        *self.inner.current_track_id.lock() = Some(cur);
        // Load but do not play: the engine lands in Paused after a load.
        self.inner.engine().load(&path)?;
        if position_seconds > 1.0 {
            let _ = self.inner.engine().seek(position_seconds);
        }
        Ok(true)
    }

    /// Remove the queue entry at `idx`.
    pub fn queue_remove_at(&self, idx: usize) -> PlayerResult<()> {
        let snap = self.inner.queue.snapshot();
        let was_current = snap.cursor == Some(idx);
        let removed = self.inner.queue.remove_at(idx);
        if !removed {
            return Ok(());
        }
        if was_current {
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

    /// Reorder the queue by moving the entry at `from` to `to`.
    pub fn queue_move(&self, from: usize, to: usize) -> PlayerResult<()> {
        self.inner.queue.move_item(from, to);
        Ok(())
    }

    /// Empty the "up next" queue. The current track keeps playing.
    pub fn clear_queue(&self) -> PlayerResult<()> {
        self.inner.queue.clear();
        Ok(())
    }

    /// Jump straight to the queue entry at `idx`, restarting playback.
    pub fn queue_jump_to(&self, idx: usize) -> PlayerResult<()> {
        let snap = self.inner.queue.snapshot();
        if idx >= snap.items.len() {
            return Err(PlayerError::QueueEmpty);
        }
        let id = snap.items[idx];
        self.inner.queue.replace(snap.items, Some(idx));
        let path = self.inner.resolve_path(id)?;
        self.inner.start_track(id, path)
    }

    /// Resume / start playback idempotently.
    pub fn play(&self) -> PlayerResult<()> {
        let status = self.inner.engine().state().status;
        match status {
            PlaybackStatus::Playing => Ok(()),
            PlaybackStatus::Paused => self.resume(),
            _ => Ok(()),
        }
    }

    pub fn pause(&self) -> PlayerResult<()> {
        let status = self.inner.engine().state().status;
        if !matches!(status, PlaybackStatus::Playing) {
            return Ok(());
        }
        self.inner.engine().pause()?;
        let position_ms = (self.inner.engine().state().position_seconds * 1000.0).max(0.0) as u64;
        self.inner.broadcast(PlayerEvent::Paused { position_ms });
        Ok(())
    }

    pub fn resume(&self) -> PlayerResult<()> {
        let status = self.inner.engine().state().status;
        if !matches!(status, PlaybackStatus::Paused) {
            return Ok(());
        }
        self.inner.engine().resume()?;
        let position_ms = (self.inner.engine().state().position_seconds * 1000.0).max(0.0) as u64;
        self.inner.broadcast(PlayerEvent::Resumed { position_ms });
        Ok(())
    }

    pub fn stop(&self) -> PlayerResult<()> {
        self.inner.engine().stop()?;
        *self.inner.current_track_id.lock() = None;
        self.inner.broadcast(PlayerEvent::Stopped);
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

    pub fn next(&self) -> PlayerResult<()> {
        if self.inner.advance()? {
            return Ok(());
        }
        if !self.inner.endless.load(Ordering::Relaxed) {
            return Ok(());
        }
        match self.play_random_album() {
            Ok(()) => Ok(()),
            Err(PlayerError::QueueEmpty) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn endless(&self) -> bool {
        self.inner.endless.load(Ordering::Relaxed)
    }

    pub fn set_endless(&self, on: bool) {
        self.inner.endless.store(on, Ordering::Relaxed);
    }

    // ---- Sleep timer ----

    pub fn set_sleep_timer(&self, minutes: u32) {
        self.inner
            .sleep_stop_after_track
            .store(false, Ordering::Relaxed);
        let mut slot = self.inner.sleep_deadline.lock();
        if minutes == 0 {
            *slot = None;
        } else {
            *slot = Some(Instant::now() + Duration::from_secs(minutes as u64 * 60));
        }
    }

    pub fn set_sleep_after_track(&self, on: bool) {
        if on {
            *self.inner.sleep_deadline.lock() = None;
        }
        self.inner
            .sleep_stop_after_track
            .store(on, Ordering::Relaxed);
    }

    pub fn sleep_timer_remaining_secs(&self) -> Option<u64> {
        let slot = self.inner.sleep_deadline.lock();
        slot.map(|deadline| deadline.saturating_duration_since(Instant::now()).as_secs())
    }

    pub fn sleep_after_track(&self) -> bool {
        self.inner.sleep_stop_after_track.load(Ordering::Relaxed)
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

    /// Pick a random album from the library and start playing it.
    pub fn play_random_album(&self) -> PlayerResult<()> {
        let albums = self.inner.library.list_albums()?;
        if albums.is_empty() {
            return Err(PlayerError::QueueEmpty);
        }
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

    // ---- Output device picking ----

    pub fn list_output_devices(&self) -> PlayerResult<Vec<OutputDevice>> {
        Ok(list_output_devices()?)
    }

    pub fn set_output_device(&self, device_id: Option<String>) -> PlayerResult<()> {
        self.inner.engine().set_output_device(device_id)?;
        Ok(())
    }

    pub fn selected_output_device(&self) -> Option<String> {
        self.inner.engine().selected_output_device()
    }
}

/// Pump engine events into the player event channel + transport bus.
fn pump_engine_events(inner: Arc<PlayerInner>, engine_events: Receiver<EngineEvent>) {
    let mut last_tick_at: Option<Instant> = None;
    let mut last_tick_position_ms: u64 = u64::MAX;

    while let Ok(ev) = engine_events.recv() {
        let player_ev = match ev {
            EngineEvent::StateChanged { mut state } => {
                state.current_track_id = inner.current_track_id.lock().map(|id| id.to_string());
                PlayerEvent::StateChanged { state }
            }
            EngineEvent::Position { position_seconds } => {
                let position_ms = (position_seconds * 1000.0).max(0.0) as u64;
                let now = Instant::now();
                let should_emit = match last_tick_at {
                    None => true,
                    Some(prev) if now.duration_since(prev) >= POSITION_TICK_THROTTLE => true,
                    _ if position_ms < last_tick_position_ms => true,
                    _ => false,
                };
                if should_emit {
                    last_tick_at = Some(now);
                    last_tick_position_ms = position_ms;
                    inner.broadcast(PlayerEvent::PositionTick { position_ms });
                }
                PlayerEvent::Position { position_seconds }
            }
            EngineEvent::EndOfTrack => {
                // Sleep timer "stop at end of track" mode.
                if inner.sleep_stop_after_track.swap(false, Ordering::Relaxed) {
                    tracing::info!(
                        target: "qobee::core",
                        "sleep timer (end of track) elapsed; stopping playback"
                    );
                    let _ = inner.engine().stop();
                    *inner.current_track_id.lock() = None;
                    last_tick_at = None;
                    last_tick_position_ms = u64::MAX;
                    inner.broadcast(PlayerEvent::Stopped);
                    let _ = inner.event_tx.try_send(PlayerEvent::EndOfTrack);
                    continue;
                }
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
                        } else {
                            inner.broadcast(PlayerEvent::Stopped);
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        tracing::warn!(target: "qobee::core", error = %msg, "auto-advance failed");
                        let _ = inner.event_tx.try_send(PlayerEvent::Error {
                            message: msg.clone(),
                        });
                        inner.broadcast(PlayerEvent::Errored {
                            kind: PlayerErrorKind::Other,
                            message: msg,
                        });
                    }
                }
                last_tick_at = None;
                last_tick_position_ms = u64::MAX;
                PlayerEvent::EndOfTrack
            }
            EngineEvent::Error { message } => {
                inner.broadcast(PlayerEvent::Errored {
                    kind: classify_engine_error(&message),
                    message: message.clone(),
                });
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

/// Coarse classifier mapping a free-form engine error message onto a
/// [`PlayerErrorKind`] for the R8 transport bus.
fn classify_engine_error(message: &str) -> PlayerErrorKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("not found")
        || lower.contains("no such file")
        || lower.contains("introuvable")
    {
        PlayerErrorKind::FileNotFound
    } else if lower.contains("decode") || lower.contains("symphonia") || lower.contains("codec") {
        PlayerErrorKind::DecodeFailed
    } else if lower.contains("device")
        || lower.contains("output")
        || lower.contains("cpal")
        || lower.contains("backend")
    {
        PlayerErrorKind::DeviceUnavailable
    } else {
        PlayerErrorKind::Other
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qobee_library::Library;

    fn fresh_library() -> Library {
        let mut base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        base.push(format!("qobee-player-test-{nanos}-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("create temp dir");
        let db = base.join("library.db");
        let cover = base.join("covers");
        Library::open(&db, &cover).expect("open library")
    }

    /// A freshly built player starts idle with no current track.
    #[test]
    fn new_player_starts_idle() {
        let library = fresh_library();
        let player = Player::new(library).expect("Player::new");
        let handle = player.handle();
        let state = handle.state();
        assert!(matches!(state.status, PlaybackStatus::Idle));
        assert!(state.current_track_id.is_none());
    }

    /// Playing an empty library's random album surfaces `QueueEmpty`
    /// rather than panicking.
    #[test]
    fn random_album_on_empty_library_is_queue_empty() {
        let library = fresh_library();
        let player = Player::new(library).expect("Player::new");
        let handle = player.handle();
        assert!(matches!(
            handle.play_random_album(),
            Err(PlayerError::QueueEmpty)
        ));
    }
}
