//! Tauri command surface exposed to the frontend.
//!
//! Each function maps directly to a `tauri::command` and returns a
//! `Result<T, String>` so errors render nicely in JS.
//!
//! **Cover art rule:** these commands only ever return *paths* or
//! *URLs*. Image bytes never cross the IPC boundary. The frontend loads
//! covers via the `qobee-cover://` custom protocol registered in
//! [`crate::lib`].

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Emitter, State};

use qobee_engine::{EffectiveOutputMode, OutputDevice, OutputMode, PlayerState};
use qobee_library::{
    Album, AlbumDetail, Artist, ArtistDetail, Genre, LibraryRoot, LibraryStats, Playlist,
    PlaylistDetail, ScanOptions, SearchResults, Track,
};
use qobee_core::queue::RepeatMode;

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

#[tauri::command]
pub fn set_output_mode(mode: OutputMode, state: State<'_, AppState>) -> Result<(), String> {
    state.player().set_output_mode(mode).map_err(map_err)
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
    state.library().list_albums_by_genre(&genre).map_err(map_err)
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
pub fn create_playlist(
    name: String,
    state: State<'_, AppState>,
) -> Result<Playlist, String> {
    state.library().create_playlist(&name).map_err(map_err)
}

#[tauri::command]
pub fn delete_playlist(playlist_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state.library().delete_playlist(playlist_id).map_err(map_err)
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
    state.player().play_tracks(track_ids, start).map_err(map_err)
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
    Ok(detail.map(|d| d.tracks.iter().map(|t| t.id).collect()).unwrap_or_default())
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
pub fn track_ids_by_artist(
    name: String,
    state: State<'_, AppState>,
) -> Result<Vec<i64>, String> {
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
pub fn set_setting(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
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
    state.library().remove_library_root(root_id).map_err(map_err)
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
/// Discord application (https://discord.com/developers/applications)
/// and the resulting "Application ID" is set here.
#[tauri::command]
pub fn discord_set_client_id(
    client_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.discord().set_client_id(client_id);
    Ok(())
}

/// Snapshot the worker's current connection state. The UI uses this
/// to show "Connected" / "Connecting…" / "No client ID set" without
/// poking Discord directly.
#[tauri::command]
pub fn discord_status(
    state: State<'_, AppState>,
) -> Result<crate::discord::DiscordStatus, String> {
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
