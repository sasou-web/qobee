//! Tauri command surface exposed to the frontend.
//!
//! Each function maps directly to a `tauri::command` and returns a
//! `Result<T, String>` so errors render nicely in JS.
//!
//! **Cover art rule:** these commands only ever return *paths* or
//! *URLs*. Image bytes never cross the IPC boundary. The frontend loads
//! covers via the `qobee-cover://` custom protocol registered in
//! the crate root.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Emitter, State};

use qobee_core::queue::RepeatMode;
use qobee_core::{audio_settings::ALL_KEYS, AudioSettingsError, ReplayGainMode};
use qobee_engine::{BitPerfectHealth, EffectiveOutputMode, OutputDevice, OutputMode, PlayerState};
use qobee_library::{
    Album, AlbumDetail, Artist, ArtistDetail, Genre, LibraryRoot, LibrarySource, LibraryStats,
    Playlist, PlaylistDetail, ScanOptions, SearchResults, SourceKind, Track,
};

use crate::lyrics::Lyrics;
use crate::state::AppState;

fn map_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Result returned by `scan_library`.
#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub files_visited: u64,
    pub files_indexed: u64,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn scan_library(
    path: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<ScanResult, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("not a directory: {}", path));
    }

    // Remember the root so the user can rescan it later from Settings.
    if let Err(e) = state.library().add_library_root(&path) {
        tracing::warn!(target: "qobee::commands", error = %e, "could not record library root");
    }

    let library = state.library().clone();
    let app_for_progress = app.clone();

    // Run the scan on a blocking task: it does fs walking + SQLite
    // writes and we don't want to monopolize the Tauri runtime.
    let report = tauri::async_runtime::spawn_blocking(move || {
        library.scan_folder(&root, ScanOptions::default(), |progress| {
            let _ = app_for_progress.emit("library:scan-progress", &progress);
        })
    })
    .await
    .map_err(|e| format!("scan task panicked: {e}"))?
    .map_err(map_err)?;

    let result = ScanResult {
        files_visited: report.files_visited,
        files_indexed: report.files_indexed,
        errors: report.errors,
    };
    let _ = app.emit("library:scan-finished", &result);
    Ok(result)
}

#[tauri::command]
pub fn list_albums(state: State<'_, AppState>) -> Result<Vec<Album>, String> {
    state.library().list_albums().map_err(map_err)
}

#[tauri::command]
pub fn list_artists(state: State<'_, AppState>) -> Result<Vec<Artist>, String> {
    state.library().list_artists().map_err(map_err)
}

#[tauri::command]
pub fn get_album(album_id: i64, state: State<'_, AppState>) -> Result<Option<AlbumDetail>, String> {
    state.library().get_album(album_id).map_err(map_err)
}

#[tauri::command]
pub fn get_track(track_id: i64, state: State<'_, AppState>) -> Result<Option<Track>, String> {
    state.library().get_track(track_id).map_err(map_err)
}

#[tauri::command]
pub fn play_track(track_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.player().play_track(track_id).map_err(map_err)
}

#[tauri::command]
pub fn play_album_from_track(
    album_id: i64,
    track_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .player()
        .play_album_from_track(album_id, track_id)
        .map_err(map_err)
}

#[tauri::command]
pub fn pause(state: State<'_, AppState>) -> Result<(), String> {
    state.player().pause().map_err(map_err)
}

#[tauri::command]
pub fn resume(state: State<'_, AppState>) -> Result<(), String> {
    state.player().resume().map_err(map_err)
}

#[tauri::command]
pub fn seek(position_seconds: f64, state: State<'_, AppState>) -> Result<(), String> {
    state.player().seek(position_seconds).map_err(map_err)
}

#[tauri::command]
pub fn set_volume(volume: f32, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_volume(volume).map_err(map_err)
}

#[tauri::command]
pub fn next(state: State<'_, AppState>) -> Result<(), String> {
    state.player().next().map_err(map_err)
}

#[tauri::command]
pub fn prev(state: State<'_, AppState>) -> Result<(), String> {
    state.player().previous().map_err(map_err)
}

#[tauri::command]
pub fn get_player_state(state: State<'_, AppState>) -> Result<PlayerState, String> {
    Ok(state.player().state())
}

#[tauri::command]
pub fn get_output_mode(state: State<'_, AppState>) -> Result<EffectiveOutputMode, String> {
    Ok(state.player().output_mode())
}

/// Read the user-facing output mode (Auto / Shared / Exclusive).
/// `get_output_mode` reports the *effective* mode (what is actually
/// running right now), this one reports what the user has selected.
#[tauri::command]
pub fn get_user_output_mode(state: State<'_, AppState>) -> Result<OutputMode, String> {
    Ok(state.player().current_output_mode())
}

#[tauri::command]
pub fn set_output_mode(mode: OutputMode, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_output_mode(mode).map_err(map_err)?;
    // Persist so the choice survives restarts.
    let value = match mode {
        OutputMode::Auto => "auto",
        OutputMode::Shared => "shared",
        OutputMode::Exclusive => "exclusive",
    };
    let _ = state.library().set_setting("audio.output_mode", value);
    Ok(())
}

// ---------------------------------------------------------------------------
// Output devices
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_output_devices(state: State<'_, AppState>) -> Result<Vec<OutputDevice>, String> {
    state.player().list_output_devices().map_err(map_err)
}

#[tauri::command]
pub fn get_selected_output_device(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.player().selected_output_device())
}

#[tauri::command]
pub fn set_output_device(
    device_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.player().set_output_device(device_id).map_err(map_err)
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn search(
    query: String,
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<SearchResults, String> {
    state
        .library()
        .search(&query, limit.unwrap_or(50))
        .map_err(map_err)
}

// ---------------------------------------------------------------------------
// Genres
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_genres(state: State<'_, AppState>) -> Result<Vec<Genre>, String> {
    state.library().list_genres().map_err(map_err)
}

#[tauri::command]
pub fn list_albums_by_genre(
    genre: String,
    state: State<'_, AppState>,
) -> Result<Vec<Album>, String> {
    state
        .library()
        .list_albums_by_genre(&genre)
        .map_err(map_err)
}

// ---------------------------------------------------------------------------
// Recently played (home)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn recently_played_tracks(
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<Track>, String> {
    state
        .library()
        .recently_played(limit.unwrap_or(20))
        .map_err(map_err)
}

#[tauri::command]
pub fn recently_played_albums(
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<Album>, String> {
    state
        .library()
        .recently_played_albums(limit.unwrap_or(12))
        .map_err(map_err)
}

#[tauri::command]
pub fn recently_played_artists(
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<Artist>, String> {
    state
        .library()
        .recently_played_artists(limit.unwrap_or(8))
        .map_err(map_err)
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_playlists(state: State<'_, AppState>) -> Result<Vec<Playlist>, String> {
    state.library().list_playlists().map_err(map_err)
}

#[tauri::command]
pub fn get_playlist(
    playlist_id: i64,
    state: State<'_, AppState>,
) -> Result<Option<PlaylistDetail>, String> {
    state.library().get_playlist(playlist_id).map_err(map_err)
}

#[tauri::command]
pub fn create_playlist(name: String, state: State<'_, AppState>) -> Result<Playlist, String> {
    state.library().create_playlist(&name).map_err(map_err)
}

#[tauri::command]
pub fn delete_playlist(playlist_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .library()
        .delete_playlist(playlist_id)
        .map_err(map_err)
}

#[tauri::command]
pub fn rename_playlist(
    playlist_id: i64,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .library()
        .rename_playlist(playlist_id, &name)
        .map_err(map_err)
}

#[tauri::command]
pub fn add_to_playlist(
    playlist_id: i64,
    track_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .library()
        .add_to_playlist(playlist_id, &track_ids)
        .map_err(map_err)
}

#[tauri::command]
pub fn remove_from_playlist(
    playlist_id: i64,
    position: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .library()
        .remove_from_playlist(playlist_id, position)
        .map_err(map_err)
}

#[tauri::command]
pub fn play_playlist_from_track(
    playlist_id: i64,
    track_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .player()
        .play_playlist_from_track(playlist_id, track_id)
        .map_err(map_err)
}

#[tauri::command]
pub fn play_tracks(
    track_ids: Vec<i64>,
    start: Option<usize>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .player()
        .play_tracks(track_ids, start)
        .map_err(map_err)
}

/// Insert `track_ids` immediately after the currently playing track.
/// If the queue is empty, start playback with these tracks.
#[tauri::command]
pub fn play_next(track_ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    state.player().play_next(track_ids).map_err(map_err)
}

/// Append `track_ids` at the end of the current queue.
#[tauri::command]
pub fn add_to_queue(track_ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    state.player().add_to_queue(track_ids).map_err(map_err)
}

/// Resolve every track id of an album, in track order, so the UI can
/// pass them to `play_next` / `add_to_queue` from a context menu.
#[tauri::command]
pub fn track_ids_for_album(album_id: i64, state: State<'_, AppState>) -> Result<Vec<i64>, String> {
    let detail = state.library().get_album(album_id).map_err(map_err)?;
    Ok(detail
        .map(|d| d.tracks.iter().map(|t| t.id).collect())
        .unwrap_or_default())
}

/// Look up the album id that owns `track_id`. Used by the player bar
/// so clicking on the album name navigates to the right page.
#[tauri::command]
pub fn album_id_for_track(
    track_id: i64,
    state: State<'_, AppState>,
) -> Result<Option<i64>, String> {
    let detail = state
        .library()
        .get_album_for_track(track_id)
        .map_err(map_err)?;
    Ok(detail.map(|d| d.album.id))
}

/// Every track id from the named artist's catalog, in album order.
#[tauri::command]
pub fn track_ids_by_artist(name: String, state: State<'_, AppState>) -> Result<Vec<i64>, String> {
    state.library().track_ids_by_artist(&name).map_err(map_err)
}

// ---------------------------------------------------------------------------
// Queue inspection / mutation
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct QueueView {
    pub items: Vec<i64>,
    pub cursor: Option<usize>,
}

#[tauri::command]
pub fn get_queue(state: State<'_, AppState>) -> Result<QueueView, String> {
    let snap = state.player().queue_snapshot();
    Ok(QueueView {
        items: snap.items,
        cursor: snap.cursor,
    })
}

#[tauri::command]
pub fn queue_remove_at(idx: usize, state: State<'_, AppState>) -> Result<(), String> {
    state.player().queue_remove_at(idx).map_err(map_err)
}

#[tauri::command]
pub fn queue_move(from: usize, to: usize, state: State<'_, AppState>) -> Result<(), String> {
    state.player().queue_move(from, to).map_err(map_err)
}

#[tauri::command]
pub fn queue_jump_to(idx: usize, state: State<'_, AppState>) -> Result<(), String> {
    state.player().queue_jump_to(idx).map_err(map_err)
}

/// Resolve a list of track ids to full Track records (so the UI can
/// render the queue with title/artist/cover without N round-trips).
#[tauri::command]
pub fn get_tracks(track_ids: Vec<i64>, state: State<'_, AppState>) -> Result<Vec<Track>, String> {
    let lib = state.library();
    let mut out = Vec::with_capacity(track_ids.len());
    for id in track_ids {
        if let Some(t) = lib.get_track(id).map_err(map_err)? {
            out.push(t);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Favorites
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn add_favorite(track_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.library().add_favorite(track_id).map_err(map_err)
}

#[tauri::command]
pub fn remove_favorite(track_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.library().remove_favorite(track_id).map_err(map_err)
}

#[tauri::command]
pub fn is_favorite(track_id: i64, state: State<'_, AppState>) -> Result<bool, String> {
    state.library().is_favorite(track_id).map_err(map_err)
}

#[tauri::command]
pub fn list_favorite_track_ids(state: State<'_, AppState>) -> Result<Vec<i64>, String> {
    state.library().list_favorite_track_ids().map_err(map_err)
}

/// Resolve every favorite track id to a full Track record, in
/// favorited-at-desc order. Used by the Favorites screen.
#[tauri::command]
pub fn list_favorites(state: State<'_, AppState>) -> Result<Vec<Track>, String> {
    let lib = state.library();
    let ids = lib.list_favorite_track_ids().map_err(map_err)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(t) = lib.get_track(id).map_err(map_err)? {
            out.push(t);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Settings (key/value preferences persisted in the SQLite DB)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_setting(key: String, state: State<'_, AppState>) -> Result<Option<String>, String> {
    state.library().get_setting(&key).map_err(map_err)
}

#[tauri::command]
pub fn set_setting(key: String, value: String, state: State<'_, AppState>) -> Result<(), String> {
    state.library().set_setting(&key, &value).map_err(map_err)
}

#[tauri::command]
pub fn list_settings(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let pairs = state.library().list_settings().map_err(map_err)?;
    Ok(pairs.into_iter().collect())
}

#[tauri::command]
pub fn clear_settings(state: State<'_, AppState>) -> Result<(), String> {
    state.library().clear_settings().map_err(map_err)
}

// ---------------------------------------------------------------------------
// Audio settings (audio.* keys persisted via AudioSettingsStore)
// ---------------------------------------------------------------------------

/// Map an [`AudioSettingsError`] to a user-friendly string. Every
/// returned message starts with `"Setting invalid: "` so the UI can
/// pattern-match on the prefix to decide on a toast vs an inline
/// validation error.
fn format_settings_error(err: AudioSettingsError) -> String {
    format!("Setting invalid: {}", err)
}

/// Read a single `audio.*` setting. Returns `None` when the key is
/// unknown, which lets the UI distinguish a missing key from a
/// `null`-valued one (only `audio.convolver_ir_path` ever produces
/// `null`).
#[tauri::command]
pub fn get_audio_setting(
    key: String,
    state: State<'_, AppState>,
) -> Result<Option<serde_json::Value>, String> {
    Ok(state.audio_settings().get(&key))
}

/// Validate, persist, and apply an `audio.*` setting. The store
/// guarantees atomicity: a failed `set` leaves the engine snapshot
/// strictly unchanged.
#[tauri::command]
pub fn set_audio_setting(
    key: String,
    value: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .audio_settings()
        .set(&key, value)
        .map_err(format_settings_error)
}

/// Snapshot every persisted `audio.*` key. The frontend uses this on
/// mount to populate the audio settings panel without round-tripping
/// each key individually.
#[tauri::command]
pub fn list_audio_settings(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, serde_json::Value>, String> {
    let store = state.audio_settings();
    let mut out = std::collections::HashMap::with_capacity(ALL_KEYS.len());
    for key in ALL_KEYS {
        if let Some(v) = store.get(key) {
            out.insert((*key).to_string(), v);
        }
    }
    Ok(out)
}

/// Reset every `audio.*` key to its default and push the fresh
/// snapshot to the engine. Used by the "Restore defaults" button in
/// the audio settings panel.
#[tauri::command]
pub fn reset_audio_settings(state: State<'_, AppState>) -> Result<(), String> {
    state
        .audio_settings()
        .write_defaults()
        .map_err(format_settings_error)
}

// ---------------------------------------------------------------------------
// Bit-Perfect Health (R5) — read-only diagnostics for the audio panel.
// ---------------------------------------------------------------------------

/// Snapshot of the device's mix format, returned by
/// [`get_device_mix_format`]. Sample rate and channel count come from
/// `IAudioClient::GetMixFormat()` on Windows and from the CPAL default
/// config elsewhere; `bit_depth` is the WASAPI valid-bits-per-sample
/// when available and `None` on platforms without a precise depth.
#[derive(Debug, Serialize)]
pub struct DeviceMixFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: Option<u8>,
}

/// Latest [`BitPerfectHealth`] snapshot known to the engine. Returns
/// `None` while idle / stopped (no device open) so the UI can hide
/// the source/track block (R5.7).
#[tauri::command]
pub fn get_bit_perfect_health(
    state: State<'_, AppState>,
) -> Result<Option<BitPerfectHealth>, String> {
    Ok(state.player().state().bit_perfect)
}

/// Read the device mix format used by the OS mixer. On Windows we go
/// straight to `IAudioClient::GetMixFormat()` so the panel can flag a
/// hidden Shared-mode resample even when Qobee itself is not playing.
/// On other operating systems we fall back to the CPAL default
/// configuration (sample rate + channel count; bit depth is `None`).
#[tauri::command]
pub fn get_device_mix_format(
    device_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<DeviceMixFormat, String> {
    // Default to whatever the player is currently routing to, so the
    // panel matches the running stream when no explicit id is sent.
    let device_id = device_id.or_else(|| state.player().selected_output_device());
    #[cfg(target_os = "windows")]
    {
        use wasapi::{initialize_mta, DeviceEnumerator, Direction};
        // COM init is idempotent here (the app may already have
        // initialised on this thread); failures are non-fatal.
        let _ = initialize_mta().ok();
        let enumerator =
            DeviceEnumerator::new().map_err(|e| format!("device enumerator: {e:?}"))?;
        let device = if let Some(want) = device_id.as_deref() {
            let mut found = None;
            if let Ok(coll) = enumerator.get_device_collection(&Direction::Render) {
                if let Ok(n) = coll.get_nbr_devices() {
                    for i in 0..n {
                        if let Ok(d) = coll.get_device_at_index(i) {
                            if let Ok(name) = d.get_friendlyname() {
                                if name == want {
                                    found = Some(d);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            match found {
                Some(d) => d,
                None => enumerator
                    .get_default_device(&Direction::Render)
                    .map_err(|e| format!("default device: {e:?}"))?,
            }
        } else {
            enumerator
                .get_default_device(&Direction::Render)
                .map_err(|e| format!("default device: {e:?}"))?
        };
        let client = device
            .get_iaudioclient()
            .map_err(|e| format!("get_iaudioclient: {e:?}"))?;
        let fmt = client
            .get_mixformat()
            .map_err(|e| format!("get_mixformat: {e:?}"))?;
        let valid = fmt.get_validbitspersample();
        let bit_depth = if valid > 0 && valid <= u8::MAX as u16 {
            Some(valid as u8)
        } else {
            None
        };
        Ok(DeviceMixFormat {
            sample_rate: fmt.get_samplespersec(),
            channels: fmt.get_nchannels(),
            bit_depth,
        })
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Reuse the engine's CPAL-backed device enumeration so we
        // don't need a direct cpal dep in the Tauri shell.
        let devices = qobee_engine::backend_cpal_shared::list_output_devices()
            .map_err(|e| format!("list devices: {e}"))?;
        let pick = match device_id.as_deref() {
            Some(want) => devices.into_iter().find(|d| d.id == want),
            None => devices.into_iter().find(|d| d.is_default).or_else(|| {
                qobee_engine::backend_cpal_shared::list_output_devices()
                    .ok()
                    .and_then(|mut v| v.pop())
            }),
        }
        .ok_or_else(|| "no default output device".to_string())?;
        Ok(DeviceMixFormat {
            sample_rate: pick.default_sample_rate,
            channels: pick.channels,
            bit_depth: None,
        })
    }
}

/// Open the Windows "Sound" control panel (`mmsys.cpl`) on the
/// Playback tab. No-op + warning log on other operating systems so
/// the UI can call it unconditionally.
#[tauri::command]
pub fn open_windows_sound_settings() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        Command::new("cmd")
            .args(["/c", "start", "mmsys.cpl,1"])
            .spawn()
            .map_err(|e| format!("spawn mmsys.cpl: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        tracing::warn!(
            target: "qobee::commands",
            "open_windows_sound_settings is a no-op on non-Windows platforms"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Convolver IR (R9) — load, unload, status
// ---------------------------------------------------------------------------

/// Snapshot of the convolver's runtime state for the
/// `ConvolverIrPicker` UI panel (R9.10).
#[derive(Debug, Serialize)]
pub struct ConvolverStatus {
    /// Mirror of `audio.convolver_enabled`.
    pub enabled: bool,
    /// Persisted IR path, or `None` when no file was ever loaded.
    pub ir_path: Option<String>,
    /// Length of the active IR in taps (post-resample), or `None`
    /// when no IR is loaded.
    pub ir_len: Option<usize>,
    /// Reported convolver latency in milliseconds.
    /// `ir_len / device_sample_rate × 1000`. Zero when no IR is
    /// loaded.
    pub latency_ms: f32,
}

/// Decode + validate + resample + apply gain compensation to the
/// supplied WAV impulse response, then push it to the engine. Errors
/// are surfaced as a string to the UI; on failure the previously
/// loaded IR (if any) is left untouched.
#[tauri::command]
pub fn load_convolver_ir(path: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .player()
        .load_convolver_ir(std::path::Path::new(&path))
        .map_err(map_err)
}

/// Drop the active IR. The convolver stage falls back to bypass on
/// the next chunk boundary (R9.7).
#[tauri::command]
pub fn unload_convolver_ir(state: State<'_, AppState>) -> Result<(), String> {
    state.player().unload_convolver_ir();
    Ok(())
}

/// Aggregate the user-facing status of the convolver (R9.10):
/// enabled flag, persisted IR path, active IR length, and reported
/// latency in milliseconds.
#[tauri::command]
pub fn get_convolver_status(state: State<'_, AppState>) -> Result<ConvolverStatus, String> {
    let store = state.audio_settings();
    let enabled = store
        .get("audio.convolver_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let ir_path = store.get("audio.convolver_ir_path").and_then(|v| match v {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    });
    // Engine reports the active IR length in taps (zero when none
    // loaded). The setter pre-validates the IR so a non-zero length
    // implies a valid IR is in flight.
    let live_len = state.player().convolver_ir_len();
    let ir_len = if live_len == 0 { None } else { Some(live_len) };
    let sr = state.player().state().sample_rate.unwrap_or(48_000) as f32;
    let latency_ms = match ir_len {
        Some(n) if sr > 0.0 => (n as f32 / sr) * 1000.0,
        _ => 0.0,
    };
    Ok(ConvolverStatus {
        enabled,
        ir_path,
        ir_len,
        latency_ms,
    })
}

// ---------------------------------------------------------------------------
// Null-test diagnostic (R11) — generate a deterministic WAV, capture
// loopback (Shared) or pre-render (Exclusive), align by FFT
// cross-correlation, and report bit-perfect / modified / inconclusive.
// ---------------------------------------------------------------------------

/// Run the null-test diagnostic end-to-end. Blocking command: the
/// orchestrator runs the capture + alignment on the current
/// thread (Tauri spawns one worker per command). Returns a complete
/// [`qobee_core::NullTestReport`] in every case (errors surface as
/// `Inconclusive` with the message in `error`).
#[tauri::command]
pub async fn run_null_test(
    state: State<'_, AppState>,
) -> Result<qobee_core::NullTestReport, String> {
    let player = state.player().clone();
    let report = tauri::async_runtime::spawn_blocking(move || qobee_core::run_null_test(&player))
        .await
        .map_err(|e| format!("null-test task panicked: {e}"))?;
    Ok(report)
}

/// Trip the cancellation flag observed by an in-flight
/// [`run_null_test`]. Returns immediately; the running task picks
/// the flag up at the next polling tick (≤ 100 ms).
#[tauri::command]
pub fn cancel_null_test() -> Result<(), String> {
    qobee_core::cancel_null_test();
    Ok(())
}

// ---------------------------------------------------------------------------
// Library roots (folders the user wants Qobee to index)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_library_roots(state: State<'_, AppState>) -> Result<Vec<LibraryRoot>, String> {
    state.library().list_library_roots().map_err(map_err)
}

#[tauri::command]
pub fn add_library_root(path: String, state: State<'_, AppState>) -> Result<LibraryRoot, String> {
    state.library().add_library_root(&path).map_err(map_err)
}

#[tauri::command]
pub fn remove_library_root(root_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .library()
        .remove_library_root(root_id)
        .map_err(map_err)
}

// ---------------------------------------------------------------------------
// Library sources (PR1 scaffolding for upcoming Google Drive support)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_library_sources(state: State<'_, AppState>) -> Result<Vec<LibrarySource>, String> {
    state.library().list_library_sources().map_err(map_err)
}

/// Register a new remote source. Returns the new row id.
/// `kind` is a free-form snake_case string ("google_drive", etc.).
/// `config_json` is opaque to the library layer and stored verbatim.
#[tauri::command]
pub fn add_library_source(
    kind: String,
    name: String,
    config_json: String,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let kind = SourceKind::parse(&kind).ok_or_else(|| format!("unknown source kind: {kind}"))?;
    state
        .library()
        .add_library_source(kind, &name, &config_json)
        .map_err(map_err)
}

#[tauri::command]
pub fn remove_library_source(source_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .library()
        .remove_library_source(source_id)
        .map_err(map_err)
}

#[tauri::command]
pub fn set_library_source_enabled(
    source_id: i64,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .library()
        .set_library_source_enabled(source_id, enabled)
        .map_err(map_err)
}

/// Scan every saved library root in sequence. Emits `library:scan-progress`
/// throughout and `library:scan-finished` once the last root is done.
#[tauri::command]
pub async fn scan_all_roots(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<ScanResult, String> {
    let roots = state.library().list_library_roots().map_err(map_err)?;
    if roots.is_empty() {
        return Err("no library root configured".into());
    }
    let library = state.library().clone();
    let app_for_progress = app.clone();

    let aggregate = tauri::async_runtime::spawn_blocking(move || {
        let mut visited: u64 = 0;
        let mut indexed: u64 = 0;
        let mut errors: Vec<String> = Vec::new();
        for root in roots {
            let root_path = std::path::PathBuf::from(&root.path);
            if !root_path.is_dir() {
                errors.push(format!("missing root: {}", root.path));
                continue;
            }
            let progress_app = app_for_progress.clone();
            match library.scan_folder(&root_path, ScanOptions::default(), |progress| {
                let _ = progress_app.emit("library:scan-progress", &progress);
            }) {
                Ok(report) => {
                    visited = visited.saturating_add(report.files_visited);
                    indexed = indexed.saturating_add(report.files_indexed);
                    errors.extend(report.errors);
                }
                Err(e) => errors.push(format!("scan {}: {e}", root.path)),
            }
        }
        ScanResult {
            files_visited: visited,
            files_indexed: indexed,
            errors,
        }
    })
    .await
    .map_err(|e| format!("rescan task panicked: {e}"))?;

    let _ = app.emit("library:scan-finished", &aggregate);
    Ok(aggregate)
}

// ---------------------------------------------------------------------------
// Stats / maintenance
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn library_stats(state: State<'_, AppState>) -> Result<LibraryStats, String> {
    state.library().library_stats().map_err(map_err)
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    state.library().clear_history().map_err(map_err)
}

/// Wipe the on-disk cover cache. Returns the number of bytes freed.
#[tauri::command]
pub fn clear_cover_cache(state: State<'_, AppState>) -> Result<u64, String> {
    state.library().clear_cover_cache().map_err(map_err)
}

/// "Factory reset" of the library data (tracks, albums, playlists,
/// history, roots, covers). Settings are kept unless `also_settings`
/// is `true`.
#[tauri::command]
pub fn reset_library(
    also_settings: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.library().wipe_all().map_err(map_err)?;
    if also_settings.unwrap_or(false) {
        state.library().clear_settings().map_err(map_err)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Equalizer (10-band peaking)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_eq_gains(state: State<'_, AppState>) -> Result<Vec<f32>, String> {
    Ok(state.player().eq_gains_db())
}

#[tauri::command]
pub fn set_eq_gains(gains: Vec<f32>, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_eq_gains_db(gains);
    Ok(())
}

// ---------------------------------------------------------------------------
// Lyrics
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_lyrics(track_id: i64, state: State<'_, AppState>) -> Result<Lyrics, String> {
    let track = state
        .library()
        .get_track(track_id)
        .map_err(map_err)?
        .ok_or_else(|| format!("track {track_id} not found"))?;
    Ok(crate::lyrics::read_for(std::path::Path::new(&track.path)))
}

// ---------------------------------------------------------------------------
// Mini player window
// ---------------------------------------------------------------------------

/// Show / hide the mini-player window. The mini window is created on
/// demand the first time the user opens it, and reused on subsequent
/// toggles. Pass `show: true` to bring the mini up and minimize the
/// main window; `false` to do the reverse.
#[tauri::command]
pub async fn toggle_mini_player(app: tauri::AppHandle, show: bool) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let main = app.get_webview_window("main");
    let mini = app.get_webview_window("mini");

    if show {
        let mini = match mini {
            Some(w) => w,
            None => {
                // Create on demand. Same Vite entry; main.ts checks
                // the window label and renders the MiniPlayer
                // component for `mini`.
                WebviewWindowBuilder::new(&app, "mini", WebviewUrl::App("index.html".into()))
                    .title("Qobee Mini")
                    .inner_size(440.0, 170.0)
                    .min_inner_size(380.0, 150.0)
                    .max_inner_size(700.0, 240.0)
                    .resizable(true)
                    .decorations(false)
                    .shadow(true)
                    .always_on_top(true)
                    .skip_taskbar(false)
                    .visible(false)
                    .build()
                    .map_err(|e| format!("create mini window: {e}"))?
            }
        };

        mini.show().map_err(|e| e.to_string())?;
        mini.set_focus().map_err(|e| e.to_string())?;

        // Hide the main window so only the mini is visible. We use
        // `hide()` (not `minimize()`) so it doesn't keep an entry in
        // the taskbar — the mini becomes the only visible Qobee
        // surface, which is what the user wants.
        if let Some(m) = main {
            let _ = m.hide();
        }
    } else {
        if let Some(m) = mini {
            let _ = m.hide();
        }
        if let Some(m) = main {
            let _ = m.unminimize();
            let _ = m.show();
            let _ = m.set_focus();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ReplayGain
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_replaygain_mode(state: State<'_, AppState>) -> Result<ReplayGainMode, String> {
    Ok(state.player().replaygain_mode())
}

#[tauri::command]
pub fn set_replaygain_mode(mode: ReplayGainMode, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_replaygain_mode(mode);
    // Persist so the choice survives restarts (key chosen to align
    // with other settings: "audio.replaygain_mode" -> "off"|"track"|"album").
    let _ = state
        .library()
        .set_setting("audio.replaygain_mode", mode.as_setting());
    Ok(())
}

// ---------------------------------------------------------------------------
// Repeat / shuffle
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_repeat_mode(state: State<'_, AppState>) -> Result<RepeatMode, String> {
    Ok(state.player().repeat_mode())
}

#[tauri::command]
pub fn set_repeat_mode(mode: RepeatMode, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_repeat_mode(mode);
    Ok(())
}

#[tauri::command]
pub fn get_shuffle(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.player().shuffle())
}

#[tauri::command]
pub fn set_shuffle(on: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_shuffle(on);
    Ok(())
}

#[tauri::command]
pub fn play_random_album(state: State<'_, AppState>) -> Result<(), String> {
    state.player().play_random_album().map_err(map_err)
}

#[tauri::command]
pub fn get_endless(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.player().endless())
}

#[tauri::command]
pub fn set_endless(on: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_endless(on);
    Ok(())
}

// ---------------------------------------------------------------------------
// Artist detail
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_artist_detail(
    name: String,
    state: State<'_, AppState>,
) -> Result<Option<ArtistDetail>, String> {
    state.library().get_artist_detail(&name).map_err(map_err)
}

// ---------------------------------------------------------------------------
// Album size
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn album_size_bytes(album_id: i64, state: State<'_, AppState>) -> Result<u64, String> {
    state.library().album_size_bytes(album_id).map_err(map_err)
}

// ---------------------------------------------------------------------------
// Discord Rich Presence
// ---------------------------------------------------------------------------
//
// All four endpoints are intentionally infallible from the JS side: a
// closed Discord, a missing IPC pipe, or any other failure must NEVER
// surface to the UI. The worker reconnects silently in the background.

/// Boot the presence worker. Idempotent — calling it again is a no-op.
#[tauri::command]
pub fn discord_init(state: State<'_, AppState>) -> Result<(), String> {
    state.discord().init();
    Ok(())
}

/// Replace the Discord application id used for the IPC handshake.
/// Required: Rich Presence only shows up if Qobee is registered as a
/// Discord application (<https://discord.com/developers/applications>)
/// and the resulting "Application ID" is set here.
#[tauri::command]
pub fn discord_set_client_id(client_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.discord().set_client_id(client_id);
    Ok(())
}

/// Snapshot the worker's current connection state. The UI uses this
/// to show "Connected" / "Connecting…" / "No client ID set" without
/// poking Discord directly.
#[tauri::command]
pub fn discord_status(state: State<'_, AppState>) -> Result<crate::discord::DiscordStatus, String> {
    Ok(state.discord().status())
}

/// Allow / forbid the background cover-host worker to upload local
/// album art to a public service so Discord can display it. The
/// integration works without this — the default `qobee` asset is
/// used instead — but the user must explicitly opt in to publish
/// any bytes off-device.
#[tauri::command]
pub fn discord_set_cover_upload_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.discord().set_cover_upload_enabled(enabled);
    Ok(())
}

/// Push the currently playing track. Pass `null` to hide the activity
/// without dropping the IPC connection.
#[tauri::command]
pub fn discord_update_track(
    track: Option<crate::discord::TrackPresence>,
    paused: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.discord().update_track(track, paused.unwrap_or(false));
    Ok(())
}

/// Toggle play / paused. Keeps the last-known track metadata.
#[tauri::command]
pub fn discord_set_paused(paused: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.discord().set_paused(paused);
    Ok(())
}

/// Hide the rich presence. The worker stays connected so the next
/// update shows up immediately.
#[tauri::command]
pub fn discord_clear_presence(state: State<'_, AppState>) -> Result<(), String> {
    state.discord().clear();
    Ok(())
}
