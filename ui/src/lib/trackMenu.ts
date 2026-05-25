// Builders for the right-click context menu.
//
// Centralized so every list (album, playlist, search results, recent…)
// gets the same vocabulary. Each builder returns a typed array of
// menu entries the caller passes straight to `contextMenu.open(...)`.

import {
  addFavorite,
  addToQueue,
  isFavorite,
  playNext,
  removeFavorite,
  trackIdsForAlbum,
  type Album,
  type Track,
} from "./api";
import type { ContextMenuEntry } from "./contextMenu.svelte";
import { contextMenu } from "./contextMenu.svelte";
import { playlistPicker } from "./playlistPicker.svelte";
import { propertiesDialog, type ArtistEntry } from "./propertiesDialog.svelte";
import { app } from "./stores.svelte";
import { toasts } from "./toasts.svelte";
import { formatDuration, formatQuality } from "./format";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function inferCodec(path: string): string {
  const dot = path.lastIndexOf(".");
  if (dot < 0) return "Unknown";
  const ext = path.slice(dot + 1).toLowerCase();
  const map: Record<string, string> = {
    flac: "FLAC (lossless)",
    alac: "ALAC (lossless)",
    wav: "WAV (PCM, lossless)",
    aiff: "AIFF (PCM, lossless)",
    aif: "AIFF (PCM, lossless)",
    mp3: "MP3",
    m4a: "AAC (M4A)",
    aac: "AAC",
    ogg: "Ogg Vorbis",
    opus: "Opus",
    wv: "WavPack",
    ape: "Monkey's Audio",
  };
  return map[ext] ?? ext.toUpperCase();
}

/** Build the unique list of "artist" rows for a track. Album artist is
 *  surfaced separately when it differs from the track artist. */
function trackArtists(track: Track): ArtistEntry[] {
  const out: ArtistEntry[] = [];
  if (track.artist) {
    out.push({
      name: track.artist,
      role: "Artist",
      onSelect: () => app.selectArtist(track.artist),
    });
  }
  if (track.album_artist && track.album_artist !== track.artist) {
    out.push({
      name: track.album_artist,
      role: "Album artist",
      onSelect: () => app.selectArtist(track.album_artist!),
    });
  }
  return out;
}

function showTrackProperties(track: Track): void {
  const audioTraits = [
    { label: "Codec", value: inferCodec(track.path) },
    {
      label: "Sample rate",
      value:
        track.sample_rate !== null
          ? `${(track.sample_rate / 1000).toFixed(1)} kHz`
          : "—",
    },
    {
      label: "Bit depth",
      value: track.bit_depth !== null ? `${track.bit_depth}-bit` : "—",
    },
    {
      label: "Channels",
      value: track.channels !== null ? String(track.channels) : "—",
    },
    {
      label: "Quality",
      value: formatQuality(track.sample_rate, track.bit_depth),
    },
    { label: "Duration", value: formatDuration(track.duration_seconds) },
  ];

  const details = [
    { label: "Title", value: track.title },
    { label: "Album", value: track.album },
    ...(track.track_number !== null
      ? [{ label: "Track #", value: String(track.track_number) }]
      : []),
    ...(track.disc_number !== null
      ? [{ label: "Disc #", value: String(track.disc_number) }]
      : []),
    ...(track.year !== null ? [{ label: "Year", value: String(track.year) }] : []),
    ...(track.genre ? [{ label: "Genre", value: track.genre }] : []),
    { label: "Path", value: track.path, monospace: true },
  ];

  propertiesDialog.show({
    title: track.title,
    subtitle: `${track.artist} — ${track.album}`,
    coverKey: track.cover_key,
    details,
    audioTraits,
    artists: trackArtists(track),
  });
}

function showAlbumProperties(album: Album): void {
  const details = [
    { label: "Title", value: album.title },
    ...(album.year !== null ? [{ label: "Year", value: String(album.year) }] : []),
    ...(album.genre ? [{ label: "Genre", value: album.genre }] : []),
    { label: "Tracks", value: String(album.track_count) },
    {
      label: "Duration",
      value: formatDuration(album.total_duration_seconds),
    },
    ...(album.total_size_bytes > 0
      ? [{ label: "Size", value: formatBytes(album.total_size_bytes) }]
      : []),
  ];

  propertiesDialog.show({
    title: album.title,
    subtitle: album.artist,
    coverKey: album.cover_key,
    details,
    // Album-level audio traits aren't stored in `Album` (they vary per
    // track); the Audio section is intentionally empty for albums.
    audioTraits: [],
    artists: [
      {
        name: album.artist,
        role: "Album artist",
        onSelect: () => app.selectArtist(album.artist),
      },
    ],
  });
}

/**
 * Build the context menu for a single track.
 *
 * `goToAlbum` and `goToArtist` callbacks default to navigation through
 * the global app store; pass overrides if a screen wants different
 * routing (e.g. clearing a search filter on its way out).
 */
export async function buildTrackMenu(
  track: Track,
  opts: {
    onGoToAlbum?: () => void;
    onGoToArtist?: () => void;
  } = {}
): Promise<ContextMenuEntry[]> {
  // Look up favorite state up front so the entry label reflects
  // reality. Failures fall back to "Add to favorites" — the action
  // itself will re-check on the backend.
  let fav = false;
  try {
    fav = await isFavorite(track.id);
  } catch {
    fav = false;
  }

  const entries: ContextMenuEntry[] = [
    {
      label: "Play next",
      onSelect: async () => {
        try {
          await playNext([track.id]);
          toasts.info(`Playing next: ${track.title}`);
        } catch (e) {
          app.lastError = String(e);
        }
      },
    },
    {
      label: "Add to queue",
      onSelect: async () => {
        try {
          await addToQueue([track.id]);
          toasts.info("Added to queue.");
        } catch (e) {
          app.lastError = String(e);
        }
      },
    },
    {
      label: "Add to playlist…",
      onSelect: () => playlistPicker.show([track.id]),
    },
    "separator",
    {
      label: fav ? "Remove from favorites" : "Add to favorites",
      onSelect: async () => {
        try {
          if (fav) {
            await removeFavorite(track.id);
            toasts.info("Removed from favorites.");
          } else {
            await addFavorite(track.id);
            toasts.success("Added to favorites.");
          }
        } catch (e) {
          app.lastError = String(e);
        }
      },
    },
    "separator",
    {
      label: "Go to artist",
      onSelect: () => {
        if (opts.onGoToArtist) opts.onGoToArtist();
        else app.selectArtist(track.artist);
      },
    },
    {
      label: "Go to album",
      onSelect: async () => {
        if (opts.onGoToAlbum) {
          opts.onGoToAlbum();
          return;
        }
        // Tracks don't carry album_id — selectAlbum needs the numeric
        // id. We skip if we don't have it; album detail screens
        // already provide their own override via `onGoToAlbum`.
      },
      // Disabled by default unless caller wires `onGoToAlbum`. The
      // album-detail screen overrides this on a per-track basis.
      disabled: !opts.onGoToAlbum,
    },
    "separator",
    {
      label: "Properties",
      onSelect: () => showTrackProperties(track),
    },
  ];

  return entries;
}

/**
 * Open the track context menu directly from a `oncontextmenu` handler.
 * Wraps the async builder so callers can pass it inline.
 */
export function openTrackMenu(
  event: MouseEvent,
  track: Track,
  opts?: { onGoToAlbum?: () => void; onGoToArtist?: () => void }
): void {
  // Prevent the default browser menu immediately, even before the
  // async favorite lookup resolves.
  event.preventDefault();
  event.stopPropagation();
  void buildTrackMenu(track, opts).then((items) => {
    // Re-emit a synthetic event so contextMenu.open's preventDefault
    // is a no-op on an already-handled event but the coords stick.
    contextMenu.open(
      new MouseEvent("contextmenu", {
        clientX: event.clientX,
        clientY: event.clientY,
      }),
      items
    );
  });
}

/**
 * Build the context menu for an album card.
 */
export function buildAlbumMenu(album: Album): ContextMenuEntry[] {
  return [
    {
      label: "Play next",
      onSelect: async () => {
        try {
          const ids = await trackIdsForAlbum(album.id);
          await playNext(ids);
          toasts.info(
            ids.length === 1 ? "Playing next." : `Playing next: ${ids.length} tracks`
          );
        } catch (e) {
          app.lastError = String(e);
        }
      },
    },
    {
      label: "Add to queue",
      onSelect: async () => {
        try {
          const ids = await trackIdsForAlbum(album.id);
          await addToQueue(ids);
          toasts.info(
            ids.length === 1 ? "Added 1 track to queue." : `Added ${ids.length} tracks to queue.`
          );
        } catch (e) {
          app.lastError = String(e);
        }
      },
    },
    {
      label: "Add to playlist…",
      onSelect: async () => {
        try {
          const ids = await trackIdsForAlbum(album.id);
          if (ids.length > 0) playlistPicker.show(ids);
        } catch (e) {
          app.lastError = String(e);
        }
      },
    },
    "separator",
    {
      label: "Go to artist",
      onSelect: () => app.selectArtist(album.artist),
    },
    {
      label: "Go to album",
      onSelect: () => app.selectAlbum(album.id),
    },
    "separator",
    {
      label: "Properties",
      onSelect: () => showAlbumProperties(album),
    },
  ];
}

export function openAlbumMenu(event: MouseEvent, album: Album): void {
  contextMenu.open(event, buildAlbumMenu(album));
}
