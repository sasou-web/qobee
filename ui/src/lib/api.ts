// Typed wrapper around the Tauri command surface.
//
// Every backend command lives in `src-tauri/src/commands.rs`. The
// argument and return types here mirror those Rust functions exactly.
// If you change one, change both.

import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------------------------------------------------------------------------
// Domain types (mirror crates/library/src/model.rs and engine types)
// ---------------------------------------------------------------------------

export interface Track {
  id: number;
  track_uid: string;
  path: string;
  title: string;
  artist: string;
  album: string;
  album_artist: string | null;
  track_number: number | null;
  disc_number: number | null;
  year: number | null;
  genre: string | null;
  duration_seconds: number;
  sample_rate: number | null;
  bit_depth: number | null;
  channels: number | null;
  cover_key: string | null;
}

export interface Album {
  id: number;
  title: string;
  artist: string;
  year: number | null;
  track_count: number;
  cover_key: string | null;
  total_duration_seconds: number;
  total_size_bytes: number;
  genre: string | null;
  date_added: number;
}

export interface Artist {
  id: number;
  name: string;
  album_count: number;
  track_count: number;
  cover_key: string | null;
}

export interface AlbumDetail {
  album: Album;
  tracks: Track[];
}

export type AlbumKind = "single" | "ep" | "album";

export interface AlbumWithKind {
  album: Album;
  kind: AlbumKind;
  duration_seconds: number;
}

export interface ArtistDetail {
  name: string;
  track_count: number;
  albums: AlbumWithKind[];
}

export type RepeatMode = "off" | "track" | "queue";

export interface Genre {
  name: string;
  track_count: number;
  cover_key: string | null;
}

export interface Playlist {
  id: number;
  name: string;
  created_at: number;
  updated_at: number;
  track_count: number;
  cover_key: string | null;
}

export interface PlaylistDetail {
  playlist: Playlist;
  tracks: Track[];
}

export interface SearchResults {
  query: string;
  tracks: Track[];
  albums: Album[];
  artists: Artist[];
}

export interface LibraryRoot {
  id: number;
  path: string;
  added_at: number;
}

/** Kind of a remote library source. PR1 only ships the type and
 *  the persistence layer; backends arrive in PR2/PR3. */
export type SourceKind = "local" | "google_drive";

export interface LibrarySource {
  id: number;
  kind: SourceKind;
  name: string;
  /** Backend-specific JSON-encoded configuration. Always
   *  `"{}"` for local sources today. */
  config: string;
  enabled: boolean;
  added_at: number;
}

export interface LibraryStats {
  track_count: number;
  album_count: number;
  artist_count: number;
  playlist_count: number;
  history_count: number;
  db_size_bytes: number;
  covers_size_bytes: number;
  data_dir: string;
  covers_dir: string;
}

export interface ScanProgress {
  files_visited: number;
  files_indexed: number;
  current: string | null;
}

export interface ScanResult {
  files_visited: number;
  files_indexed: number;
  errors: string[];
}

export type PlaybackStatus =
  | "idle"
  | "loading"
  | "playing"
  | "paused"
  | "stopped"
  | "errored";

export type EffectiveOutputMode = "shared" | "exclusive";

export type OutputMode = "auto" | "shared" | "exclusive";

export interface OutputDevice {
  id: string;
  name: string;
  is_default: boolean;
  default_sample_rate: number;
  channels: number;
}

export interface PlayerState {
  status: PlaybackStatus;
  current_track_id: string | null;
  position_seconds: number;
  duration_seconds: number;
  volume: number;
  output_mode: EffectiveOutputMode;
  sample_rate: number | null;
  bit_depth: number | null;
  channels: number | null;
  is_bit_perfect: boolean;
  /// Reported RG attenuation in dB (R1.5). Negative when the
  /// pre-gain stage clamped the demanded RG gain to fit the
  /// configured true-peak ceiling; null otherwise.
  rg_attenuation_db: number | null;
  /// Aggregated bit-perfect health snapshot (R5). Populated
  /// whenever a device is currently open; absent while idle.
  bit_perfect?: BitPerfectHealth | null;
  error: string | null;
}

// Discriminated union mirroring qobee_core::PlayerEvent.
export type PlayerEvent =
  | { type: "state_changed"; state: PlayerState }
  | { type: "position"; position_seconds: number }
  | { type: "end_of_track" }
  | { type: "bit_perfect_changed"; health: BitPerfectHealth }
  | { type: "error"; message: string };

// ---------------------------------------------------------------------------
// Bit-Perfect Health (R5)
// ---------------------------------------------------------------------------

export type BitPerfectStatus = "green" | "amber" | "red";

export interface BitPerfectHealth {
  status: BitPerfectStatus;
  source_sample_rate: number | null;
  device_sample_rate: number | null;
  source_bit_depth: number | null;
  effective_output_mode: EffectiveOutputMode;
  is_native_rate: boolean;
  unity_volume: boolean;
  unity_pregain: boolean;
  eq_bypass: boolean;
  crossfeed_off: boolean;
  convolver_off: boolean;
  limiter_off: boolean;
  dither_bypass: boolean;
  balance_off: boolean;
  upmix_active: boolean;
  messages: string[];
}

export interface DeviceMixFormat {
  sample_rate: number;
  channels: number;
  bit_depth: number | null;
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

export async function scanLibrary(path: string): Promise<ScanResult> {
  return invoke<ScanResult>("scan_library", { path });
}

export async function listAlbums(): Promise<Album[]> {
  return invoke<Album[]>("list_albums");
}

export async function listArtists(): Promise<Artist[]> {
  return invoke<Artist[]>("list_artists");
}

export async function getAlbum(albumId: number): Promise<AlbumDetail | null> {
  return invoke<AlbumDetail | null>("get_album", { albumId });
}

export async function getTrack(trackId: number): Promise<Track | null> {
  return invoke<Track | null>("get_track", { trackId });
}

export async function playTrack(trackId: number): Promise<void> {
  await invoke("play_track", { trackId });
}

export async function playAlbumFromTrack(
  albumId: number,
  trackId: number
): Promise<void> {
  await invoke("play_album_from_track", { albumId, trackId });
}

/** Start an album from its first track. Backs the R3 `Play_Button`
 *  on album cards / album detail headers; the UI carries only the
 *  album id and shouldn't have to look up the first track id with
 *  an extra round-trip. */
export async function playAlbum(albumId: number): Promise<void> {
  await invoke("play_album", { albumId });
}

export async function pause(): Promise<void> {
  await invoke("pause");
}

export async function resume(): Promise<void> {
  await invoke("resume");
}

export async function seek(positionSeconds: number): Promise<void> {
  await invoke("seek", { positionSeconds });
}

export async function setVolume(volume: number): Promise<void> {
  await invoke("set_volume", { volume });
}

export async function nextTrack(): Promise<void> {
  await invoke("next");
}

export async function prevTrack(): Promise<void> {
  await invoke("prev");
}

export async function getPlayerState(): Promise<PlayerState> {
  return invoke<PlayerState>("get_player_state");
}

export async function getOutputMode(): Promise<EffectiveOutputMode> {
  return invoke<EffectiveOutputMode>("get_output_mode");
}

export async function setOutputMode(mode: OutputMode): Promise<void> {
  await invoke("set_output_mode", { mode });
}

export async function getUserOutputMode(): Promise<OutputMode> {
  return invoke<OutputMode>("get_user_output_mode");
}

export async function listOutputDevices(): Promise<OutputDevice[]> {
  return invoke<OutputDevice[]>("list_output_devices");
}

export async function getSelectedOutputDevice(): Promise<string | null> {
  return invoke<string | null>("get_selected_output_device");
}

export async function setOutputDevice(deviceId: string | null): Promise<void> {
  await invoke("set_output_device", { deviceId });
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

export async function search(
  query: string,
  limit = 50
): Promise<SearchResults> {
  return invoke<SearchResults>("search", { query, limit });
}

// ---------------------------------------------------------------------------
// Genres
// ---------------------------------------------------------------------------

export async function listGenres(): Promise<Genre[]> {
  return invoke<Genre[]>("list_genres");
}

export async function listAlbumsByGenre(genre: string): Promise<Album[]> {
  return invoke<Album[]>("list_albums_by_genre", { genre });
}

// ---------------------------------------------------------------------------
// Recently played (home)
// ---------------------------------------------------------------------------

export async function recentlyPlayedTracks(limit = 20): Promise<Track[]> {
  return invoke<Track[]>("recently_played_tracks", { limit });
}

export async function recentlyPlayedAlbums(limit = 12): Promise<Album[]> {
  return invoke<Album[]>("recently_played_albums", { limit });
}

export async function recentlyPlayedArtists(limit = 8): Promise<Artist[]> {
  return invoke<Artist[]>("recently_played_artists", { limit });
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

export async function listPlaylists(): Promise<Playlist[]> {
  return invoke<Playlist[]>("list_playlists");
}

export async function getPlaylist(
  playlistId: number
): Promise<PlaylistDetail | null> {
  return invoke<PlaylistDetail | null>("get_playlist", { playlistId });
}

export async function createPlaylist(name: string): Promise<Playlist> {
  return invoke<Playlist>("create_playlist", { name });
}

export async function deletePlaylist(playlistId: number): Promise<void> {
  await invoke("delete_playlist", { playlistId });
}

export async function renamePlaylist(
  playlistId: number,
  name: string
): Promise<void> {
  await invoke("rename_playlist", { playlistId, name });
}

export async function addToPlaylist(
  playlistId: number,
  trackIds: number[]
): Promise<void> {
  await invoke("add_to_playlist", { playlistId, trackIds });
}

export async function removeFromPlaylist(
  playlistId: number,
  position: number
): Promise<void> {
  await invoke("remove_from_playlist", { playlistId, position });
}

export async function playPlaylistFromTrack(
  playlistId: number,
  trackId: number
): Promise<void> {
  await invoke("play_playlist_from_track", { playlistId, trackId });
}

/** Start a playlist from its first track. Companion to
 *  [`playAlbum`] for the R3 `Play_Button` on playlist cards. */
export async function playPlaylist(playlistId: number): Promise<void> {
  await invoke("play_playlist", { playlistId });
}

export async function playTracks(
  trackIds: number[],
  start: number | null = null
): Promise<void> {
  await invoke("play_tracks", { trackIds, start });
}

export async function playNext(trackIds: number[]): Promise<void> {
  await invoke("play_next", { trackIds });
}

export async function addToQueue(trackIds: number[]): Promise<void> {
  await invoke("add_to_queue", { trackIds });
}

export async function trackIdsForAlbum(albumId: number): Promise<number[]> {
  return invoke<number[]>("track_ids_for_album", { albumId });
}

export async function albumIdForTrack(trackId: number): Promise<number | null> {
  return invoke<number | null>("album_id_for_track", { trackId });
}

export async function trackIdsByArtist(name: string): Promise<number[]> {
  return invoke<number[]>("track_ids_by_artist", { name });
}

// ---------------------------------------------------------------------------
// Favorites
// ---------------------------------------------------------------------------

export async function addFavorite(trackId: number): Promise<void> {
  await invoke("add_favorite", { trackId });
}

export async function removeFavorite(trackId: number): Promise<void> {
  await invoke("remove_favorite", { trackId });
}

export async function isFavorite(trackId: number): Promise<boolean> {
  return invoke<boolean>("is_favorite", { trackId });
}

export async function listFavoriteTrackIds(): Promise<number[]> {
  return invoke<number[]>("list_favorite_track_ids");
}

export async function listFavorites(): Promise<Track[]> {
  return invoke<Track[]>("list_favorites");
}

// ---------------------------------------------------------------------------
// Queue inspection
// ---------------------------------------------------------------------------

export interface QueueView {
  items: number[];
  cursor: number | null;
}

export async function getQueue(): Promise<QueueView> {
  return invoke<QueueView>("get_queue");
}

export async function queueRemoveAt(idx: number): Promise<void> {
  await invoke("queue_remove_at", { idx });
}

export async function queueMove(from: number, to: number): Promise<void> {
  await invoke("queue_move", { from, to });
}

export async function queueJumpTo(idx: number): Promise<void> {
  await invoke("queue_jump_to", { idx });
}

export async function getTracks(trackIds: number[]): Promise<Track[]> {
  return invoke<Track[]>("get_tracks", { trackIds });
}

// ---------------------------------------------------------------------------
// Event helpers
// ---------------------------------------------------------------------------

export async function onPlayerState(
  cb: (state: PlayerState) => void
): Promise<UnlistenFn> {
  return listen<PlayerEvent>("player:state", (e) => {
    if (e.payload.type === "state_changed") cb(e.payload.state);
  });
}

export async function onPlayerPosition(
  cb: (positionSeconds: number) => void
): Promise<UnlistenFn> {
  return listen<PlayerEvent>("player:position", (e) => {
    if (e.payload.type === "position") cb(e.payload.position_seconds);
  });
}

export async function onPlayerEndOfTrack(
  cb: () => void
): Promise<UnlistenFn> {
  return listen<PlayerEvent>("player:end-of-track", (_e) => cb());
}

export async function onPlayerError(
  cb: (message: string) => void
): Promise<UnlistenFn> {
  return listen<PlayerEvent>("player:error", (e) => {
    if (e.payload.type === "error") cb(e.payload.message);
  });
}

export async function onScanProgress(
  cb: (progress: ScanProgress) => void
): Promise<UnlistenFn> {
  return listen<ScanProgress>("library:scan-progress", (e) => cb(e.payload));
}

export async function onScanFinished(
  cb: (result: ScanResult) => void
): Promise<UnlistenFn> {
  return listen<ScanResult>("library:scan-finished", (e) => cb(e.payload));
}

// ---------------------------------------------------------------------------
// Cover URL helper
// ---------------------------------------------------------------------------

/**
 * Build the URL to fetch a cover image. Cover bytes never travel over
 * IPC; the Tauri shell registers a `qobee-cover://` URI scheme that
 * streams files from the disk cache. `convertFileSrc` builds the right
 * URL for the current platform (Windows / macOS / Linux all use
 * different forms).
 */
export function coverUrl(coverKey: string | null | undefined): string | null {
  if (!coverKey) return null;
  return convertFileSrc(coverKey, "qobee-cover");
}

// ---------------------------------------------------------------------------
// Settings (key/value preferences)
// ---------------------------------------------------------------------------

export async function getSetting(key: string): Promise<string | null> {
  return invoke<string | null>("get_setting", { key });
}

export async function setSetting(key: string, value: string): Promise<void> {
  await invoke("set_setting", { key, value });
}

export async function listSettings(): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("list_settings");
}

export async function clearSettings(): Promise<void> {
  await invoke("clear_settings");
}

// ---------------------------------------------------------------------------
// Audio settings (typed wrappers around the audio.* keys persisted by
// AudioSettingsStore — see crates/core/src/audio_settings.rs)
// ---------------------------------------------------------------------------

/**
 * Read a single `audio.*` setting. Returns `null` when the key is
 * unknown to the store. Only `audio.convolver_ir_path` ever returns
 * `null` for a *known* key, so callers should treat `null` as
 * "missing" for every other setting.
 *
 * The generic parameter lets callers narrow the JSON value to the
 * concrete shape they expect (`number`, `boolean`, an enum string,
 * or an array of numbers for `audio.trim_db_per_channel`).
 */
export async function getAudioSetting<T = unknown>(
  key: string
): Promise<T | null> {
  return invoke<T | null>("get_audio_setting", { key });
}

/**
 * Validate, persist, and apply a single `audio.*` setting. Rejects
 * with a `"Setting invalid: ..."` string when the value's type or
 * range is wrong; the engine snapshot is left untouched in that case.
 */
export async function setAudioSetting<T>(
  key: string,
  value: T
): Promise<void> {
  await invoke("set_audio_setting", { key, value });
}

/**
 * Snapshot every persisted `audio.*` key in one round-trip. The
 * audio settings panel uses this on mount.
 */
export async function listAudioSettings(): Promise<Record<string, unknown>> {
  return invoke<Record<string, unknown>>("list_audio_settings");
}

/**
 * Reset every `audio.*` key to its default value. Backs the
 * "Restore defaults" button.
 */
export async function resetAudioSettings(): Promise<void> {
  await invoke("reset_audio_settings");
}

// ---------------------------------------------------------------------------
// Bit-Perfect Health (R5) — diagnostics for the audio panel
// ---------------------------------------------------------------------------

/**
 * Snapshot of the engine's last-known [`BitPerfectHealth`]. Resolves
 * to `null` while no device is open (idle/stopped), in which case
 * the UI panel hides the source/track block (R5.7).
 */
export async function getBitPerfectHealth(): Promise<BitPerfectHealth | null> {
  return invoke<BitPerfectHealth | null>("get_bit_perfect_health");
}

/**
 * Read the device mix format used by the OS mixer. On Windows this
 * goes through `IAudioClient::GetMixFormat()`; on other platforms it
 * returns the CPAL default config (`bit_depth` is then `null`).
 */
export async function getDeviceMixFormat(
  deviceId?: string | null
): Promise<DeviceMixFormat> {
  return invoke<DeviceMixFormat>("get_device_mix_format", {
    deviceId: deviceId ?? null,
  });
}

/**
 * Open the Windows "Sound" control panel. No-op (with a backend
 * warning log) on other operating systems so the UI can call this
 * unconditionally.
 */
export async function openWindowsSoundSettings(): Promise<void> {
  await invoke("open_windows_sound_settings");
}

/**
 * Open an external URL in the user's default browser. Used by the
 * Google Drive error screen (R5.2) to take the user to the
 * `docs/google-cloud-setup.md` walkthrough when an authorization
 * fails, and by Settings → Drive for the "How to configure"
 * affordance. The backend validates the URL starts with
 * `http(s)://` so this can't be turned into a generic file opener.
 */
export async function shellOpen(url: string): Promise<void> {
  await invoke("shell_open", { url });
}

/** Subscribe to the dedicated `player:bit-perfect` Tauri topic. */
export async function onBitPerfectChanged(
  cb: (health: BitPerfectHealth) => void
): Promise<UnlistenFn> {
  return listen<PlayerEvent>("player:bit-perfect", (e) => {
    if (e.payload.type === "bit_perfect_changed") cb(e.payload.health);
  });
}

// ---------------------------------------------------------------------------
// Convolver IR (R9) — load, unload, status
// ---------------------------------------------------------------------------

export interface ConvolverStatus {
  enabled: boolean;
  ir_path: string | null;
  ir_len: number | null;
  latency_ms: number;
}

/**
 * Decode + validate + resample + apply gain compensation to the
 * supplied WAV impulse response and push it to the engine's
 * `Convolver_Stage`. Rejects with a descriptive string on failure;
 * the previously-loaded IR (if any) stays untouched.
 */
export async function loadConvolverIr(path: string): Promise<void> {
  await invoke("load_convolver_ir", { path });
}

/** Drop the active convolver IR. The stage falls back to bypass. */
export async function unloadConvolverIr(): Promise<void> {
  await invoke("unload_convolver_ir");
}

/**
 * Snapshot of the convolver runtime state for the
 * `ConvolverIrPicker` panel: enabled flag, persisted IR path,
 * active IR length in taps, and reported latency in milliseconds.
 */
export async function getConvolverStatus(): Promise<ConvolverStatus> {
  return invoke<ConvolverStatus>("get_convolver_status");
}

// ---------------------------------------------------------------------------
// Null-test diagnostic (R11)
// ---------------------------------------------------------------------------

export type NullTestConclusion = "bit_perfect" | "modified" | "inconclusive";
export type NullTestSource = "loopback" | "pre_render";

export interface NullTestReport {
  conclusion: NullTestConclusion;
  mode: EffectiveOutputMode;
  captured_via: NullTestSource;
  peak_diff_dbfs: number;
  rms_diff_dbfs: number;
  samples_diff_count: number;
  aligned_frames: number;
  error: string | null;
}

/**
 * Run the null-test diagnostic end-to-end: generate a deterministic
 * 5 s WAV, capture it back via WASAPI loopback (Shared) or via the
 * engine's pre-render sink (Exclusive), align by FFT cross-correlation
 * and produce a conclusion. Always resolves with a report; failures
 * surface as `conclusion = "inconclusive"` with `error` populated.
 */
export async function runNullTest(): Promise<NullTestReport> {
  return invoke<NullTestReport>("run_null_test");
}

/**
 * Trip the cancellation flag so the in-flight `runNullTest` returns
 * an `Inconclusive` report at its next polling tick (≤ 100 ms).
 */
export async function cancelNullTest(): Promise<void> {
  await invoke("cancel_null_test");
}

// ---------------------------------------------------------------------------
// Library roots / stats / maintenance
// ---------------------------------------------------------------------------

export async function listLibraryRoots(): Promise<LibraryRoot[]> {
  return invoke<LibraryRoot[]>("list_library_roots");
}

export async function addLibraryRoot(path: string): Promise<LibraryRoot> {
  return invoke<LibraryRoot>("add_library_root", { path });
}

export async function removeLibraryRoot(rootId: number): Promise<void> {
  await invoke("remove_library_root", { rootId });
}

// ---------------------------------------------------------------------------
// Library sources (PR1 scaffolding for upcoming Google Drive backend)
// ---------------------------------------------------------------------------

export async function listLibrarySources(): Promise<LibrarySource[]> {
  return invoke<LibrarySource[]>("list_library_sources");
}

export async function addLibrarySource(
  kind: SourceKind,
  name: string,
  configJson: string,
): Promise<number> {
  return invoke<number>("add_library_source", { kind, name, configJson });
}

export async function removeLibrarySource(sourceId: number): Promise<void> {
  await invoke("remove_library_source", { sourceId });
}

export async function setLibrarySourceEnabled(
  sourceId: number,
  enabled: boolean,
): Promise<void> {
  await invoke("set_library_source_enabled", { sourceId, enabled });
}

// ---------------------------------------------------------------------------
// Google Drive backend (PR2)
// ---------------------------------------------------------------------------

export interface OAuthStartResult {
  session_id: string;
  auth_url: string;
}

export interface OAuthFinishResult {
  source_id: number;
  email: string | null;
  display_name: string | null;
}

export interface DriveStatus {
  source_id: number;
  connected: boolean;
  email: string | null;
  display_name: string | null;
  message: string | null;
}

export interface DriveListItem {
  id: string;
  name: string;
  mime_type: string;
  is_folder: boolean;
}

/** Start the OAuth desktop flow. Opens the browser at the consent
 *  screen and binds a localhost listener for the redirect. */
export async function driveOAuthStart(
  clientId: string,
  clientSecret: string,
): Promise<OAuthStartResult> {
  return invoke<OAuthStartResult>("drive_oauth_start", {
    clientId,
    clientSecret,
  });
}

/** Wait for the user to finish the consent screen, then exchange
 *  the code for tokens and persist them in the OS keychain. The
 *  promise resolves once the source row is created in the DB. */
export async function driveOAuthWait(
  sessionId: string,
  name: string,
): Promise<OAuthFinishResult> {
  return invoke<OAuthFinishResult>("drive_oauth_wait", { sessionId, name });
}

export async function driveOAuthCancel(sessionId: string): Promise<void> {
  await invoke("drive_oauth_cancel", { sessionId });
}

export async function driveStatus(sourceId: number): Promise<DriveStatus> {
  return invoke<DriveStatus>("drive_status", { sourceId });
}

export async function driveDisconnect(sourceId: number): Promise<void> {
  await invoke("drive_disconnect", { sourceId });
}

export async function driveListFolder(
  sourceId: number,
  folderId: string,
): Promise<DriveListItem[]> {
  return invoke<DriveListItem[]>("drive_list_folder", { sourceId, folderId });
}

export async function driveSetFolder(
  sourceId: number,
  folderId: string,
  folderName: string,
): Promise<void> {
  await invoke("drive_set_folder", { sourceId, folderId, folderName });
}

export interface DriveIndexResult {
  files_visited: number;
  files_indexed: number;
  errors: string[];
}

export async function driveIndex(sourceId: number): Promise<DriveIndexResult> {
  return invoke<DriveIndexResult>("drive_index", { sourceId });
}

export interface DriveSyncResult {
  pulled: number;
  pushed: number;
  source_id: number;
}

/** Push local favorites for `sourceId` to Drive. */
export async function driveSyncPush(sourceId: number): Promise<DriveSyncResult> {
  return invoke<DriveSyncResult>("drive_sync_push", { sourceId });
}

/** Replace local favorites for `sourceId` with whatever Drive holds. */
export async function driveSyncPull(sourceId: number): Promise<DriveSyncResult> {
  return invoke<DriveSyncResult>("drive_sync_pull", { sourceId });
}

/** Two-way sync: pull then push. Local wins on conflicts. */
export async function driveSync(sourceId: number): Promise<DriveSyncResult> {
  return invoke<DriveSyncResult>("drive_sync", { sourceId });
}

export async function scanAllRoots(): Promise<ScanResult> {
  return invoke<ScanResult>("scan_all_roots");
}

export async function libraryStats(): Promise<LibraryStats> {
  return invoke<LibraryStats>("library_stats");
}

export async function clearHistory(): Promise<void> {
  await invoke("clear_history");
}

export async function clearCoverCache(): Promise<number> {
  return invoke<number>("clear_cover_cache");
}

export async function resetLibrary(alsoSettings = false): Promise<void> {
  await invoke("reset_library", { alsoSettings });
}

// ---------------------------------------------------------------------------
// Equalizer / repeat / shuffle / artist / album size
// ---------------------------------------------------------------------------

export async function getEqGains(): Promise<number[]> {
  return invoke<number[]>("get_eq_gains");
}

export async function setEqGains(gains: number[]): Promise<void> {
  await invoke("set_eq_gains", { gains });
}

export type ReplayGainMode = "off" | "track" | "album";

export async function getReplayGainMode(): Promise<ReplayGainMode> {
  return invoke<ReplayGainMode>("get_replaygain_mode");
}

export async function setReplayGainMode(mode: ReplayGainMode): Promise<void> {
  await invoke("set_replaygain_mode", { mode });
}

// ---------------------------------------------------------------------------
// Lyrics
// ---------------------------------------------------------------------------

export interface LyricLine {
  ms: number;
  text: string;
}

export interface Lyrics {
  unsynced: string | null;
  synced: LyricLine[] | null;
}

export async function getLyrics(trackId: number): Promise<Lyrics> {
  return invoke<Lyrics>("get_lyrics", { trackId });
}

export async function toggleMiniPlayer(show: boolean): Promise<void> {
  await invoke("toggle_mini_player", { show });
}

export async function getRepeatMode(): Promise<RepeatMode> {
  return invoke<RepeatMode>("get_repeat_mode");
}

export async function setRepeatMode(mode: RepeatMode): Promise<void> {
  await invoke("set_repeat_mode", { mode });
}

export async function getShuffle(): Promise<boolean> {
  return invoke<boolean>("get_shuffle");
}

export async function setShuffle(on: boolean): Promise<void> {
  await invoke("set_shuffle", { on });
}

export async function playRandomAlbum(): Promise<void> {
  await invoke("play_random_album");
}

export async function getEndless(): Promise<boolean> {
  return invoke<boolean>("get_endless");
}

export async function setEndless(on: boolean): Promise<void> {
  await invoke("set_endless", { on });
}

export async function getArtistDetail(name: string): Promise<ArtistDetail | null> {
  return invoke<ArtistDetail | null>("get_artist_detail", { name });
}

export async function albumSizeBytes(albumId: number): Promise<number> {
  return invoke<number>("album_size_bytes", { albumId });
}

// ---------------------------------------------------------------------------
// Windows integration (taskbar / tray / context menus / autostart)
// ---------------------------------------------------------------------------

/**
 * Status reported by `commands_integration::get_windows_integration_status`.
 *
 * `platform_supported` is `false` on macOS / Linux: every toggle in
 * the panel is then disabled and persisted as no-ops.
 */
export interface WindowsIntegrationStatus {
  platform_supported: boolean;
  autostart: boolean;
  start_minimized: boolean;
  minimize_to_tray_on_close: boolean;
  show_tray_icon: boolean;
  audio_file_context_menu: boolean;
  folder_context_menu: boolean;
  protocol_handler: boolean;
  jump_list: boolean;
  app_user_model_id: string;
}

/** Partial update fed to `set_windows_integration`. */
export interface WindowsIntegrationToggle {
  autostart?: boolean;
  start_minimized?: boolean;
  minimize_to_tray_on_close?: boolean;
  show_tray_icon?: boolean;
  audio_file_context_menu?: boolean;
  folder_context_menu?: boolean;
  protocol_handler?: boolean;
  jump_list?: boolean;
}

export async function getWindowsIntegrationStatus(): Promise<WindowsIntegrationStatus> {
  return await invoke<WindowsIntegrationStatus>(
    "get_windows_integration_status",
  );
}

export async function setWindowsIntegration(
  update: WindowsIntegrationToggle,
): Promise<WindowsIntegrationStatus> {
  return await invoke<WindowsIntegrationStatus>("set_windows_integration", {
    update,
  });
}

// ---------------------------------------------------------------------------
// Window_Manager preferences (R7 — task 10.1)
// ---------------------------------------------------------------------------

/**
 * Snapshot of the three Window_Manager preferences persisted in
 * the SQLite `settings` table:
 *
 * - `close_behavior` (R7.3): how closing the main window should
 *   behave — quit the process, hide it to the system tray, or
 *   keep it running invisibly in the background.
 * - `tray_enabled` (R7.1): whether to show a tray icon with
 *   transport controls. Tray creation/destruction at runtime is
 *   task 10.2's job; this flag only stores the user choice.
 * - `notify_on_track_change` (R7.7): whether to fire a native OS
 *   notification on every track change. The Tauri-side fan-out
 *   (`dispatch_player_event`) already reads this key on every
 *   track change with a 5-second throttle.
 */
export type CloseBehavior =
  | "quit"
  | "minimize_to_tray"
  | "keep_running_in_background";

export interface WindowSettings {
  close_behavior: CloseBehavior;
  tray_enabled: boolean;
  notify_on_track_change: boolean;
}

/** Read the persisted Window_Manager preferences (R7.3, R7.7,
 *  R7.8). Defaults are applied server-side when rows are missing
 *  on a fresh install (`quit` / `false` / `true`). */
export async function getWindowSettings(): Promise<WindowSettings> {
  return invoke<WindowSettings>("get_window_settings");
}

/** Persist `windows.close_behavior`. Backend rejects values
 *  outside the [`CloseBehavior`] union with a typed error string. */
export async function setCloseBehavior(
  value: CloseBehavior,
): Promise<WindowSettings> {
  return invoke<WindowSettings>("set_close_behavior", { value });
}

/** Persist `windows.tray_enabled`. */
export async function setTrayEnabled(value: boolean): Promise<WindowSettings> {
  return invoke<WindowSettings>("set_tray_enabled", { value });
}

/** Persist `windows.notify_on_track_change`. The fan-out task
 *  reads this on every `TrackChanged` event to gate the OS
 *  notification (R7.7). */
export async function setNotifyOnTrackChange(
  value: boolean,
): Promise<WindowSettings> {
  return invoke<WindowSettings>("set_notify_on_track_change", { value });
}
