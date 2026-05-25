// Media Session API integration.
//
// The browser exposes `navigator.mediaSession` so OS-level media keys
// (the play/pause key on a keyboard, AirPods double-tap, Bluetooth
// remote, Windows quick controls…) can drive the player. We just push
// metadata + register handlers; the OS does the rest.

import {
  nextTrack,
  pause,
  prevTrack,
  resume,
  seek,
  type Track,
  coverUrl,
} from "./api";

let registered = false;
let lastTrackId: string | null = null;

/** Wire the static action handlers once. Idempotent. */
export function ensureMediaSession(): void {
  if (registered) return;
  if (typeof navigator === "undefined" || !navigator.mediaSession) return;
  registered = true;

  navigator.mediaSession.setActionHandler("play", () => {
    void resume();
  });
  navigator.mediaSession.setActionHandler("pause", () => {
    void pause();
  });
  navigator.mediaSession.setActionHandler("previoustrack", () => {
    void prevTrack();
  });
  navigator.mediaSession.setActionHandler("nexttrack", () => {
    void nextTrack();
  });
  navigator.mediaSession.setActionHandler("seekto", (details) => {
    if (typeof details.seekTime === "number") {
      void seek(details.seekTime);
    }
  });
  // Per the spec, "stop" lets OS controls (e.g. notification close)
  // halt playback. We treat it as pause; full stop would clear the
  // queue, which feels too destructive for an OS-level button.
  navigator.mediaSession.setActionHandler("stop", () => {
    void pause();
  });
}

/** Push the currently playing track's metadata to the OS. */
export function setMediaMetadata(track: Track | null): void {
  if (typeof navigator === "undefined" || !navigator.mediaSession) return;
  ensureMediaSession();

  if (!track) {
    navigator.mediaSession.metadata = null;
    navigator.mediaSession.playbackState = "none";
    lastTrackId = null;
    return;
  }
  const id = String(track.id);
  if (id === lastTrackId) return;
  lastTrackId = id;

  const artwork: MediaImage[] = [];
  const art = coverUrl(track.cover_key);
  if (art) {
    // Same image at multiple "advertised" sizes so Windows / macOS
    // pick the right one. The actual file is whatever the cover
    // cache holds — not regenerated per size.
    artwork.push(
      { src: art, sizes: "96x96", type: "image/jpeg" },
      { src: art, sizes: "256x256", type: "image/jpeg" },
      { src: art, sizes: "512x512", type: "image/jpeg" }
    );
  }
  navigator.mediaSession.metadata = new MediaMetadata({
    title: track.title,
    artist: track.artist,
    album: track.album,
    artwork,
  });
}

/** Reflect engine state into the OS-level "is currently playing" flag. */
export function setMediaPlaybackState(state: "playing" | "paused" | "none"): void {
  if (typeof navigator === "undefined" || !navigator.mediaSession) return;
  navigator.mediaSession.playbackState = state;
}

/** Report current position so OS scrubbers (e.g. Windows lock screen)
 *  show progress. Called per position event. */
export function setMediaPositionState(
  duration: number,
  position: number,
  rate = 1
): void {
  if (typeof navigator === "undefined" || !navigator.mediaSession) return;
  if (typeof navigator.mediaSession.setPositionState !== "function") return;
  if (!Number.isFinite(duration) || duration <= 0) return;
  // Spec requires position <= duration.
  const clamped = Math.max(0, Math.min(position, duration));
  try {
    navigator.mediaSession.setPositionState({
      duration,
      position: clamped,
      playbackRate: rate,
    });
  } catch {
    // Some webviews throw on bad ranges; ignore.
  }
}
