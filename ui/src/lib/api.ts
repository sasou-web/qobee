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
  error: string | null;
}

// Discriminated union mirroring qobee_core::PlayerEvent.
export type PlayerEvent =
  | { type: "state_changed"; state: PlayerState }
  | { type: "position"; position_seconds: number }
  | { type: "end_of_track" }
  | { type: "error"; message: string };

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
