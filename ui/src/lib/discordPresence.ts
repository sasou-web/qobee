// Discord Rich Presence — frontend singleton.
//
// Mirrors the backend worker (src-tauri/src/discord.rs). The actual
// IPC and reconnection logic lives in Rust; this file is a thin,
// non-blocking dispatcher with the public API the rest of the app
// uses:
//
//   discordPresence.init();
//   discordPresence.updateTrack(track, position, isPaused);
//   discordPresence.setPaused(isPaused);
//   discordPresence.clearPresence();
//
// All four methods are fire-and-forget and never throw to callers.
// Discord being closed, missing IPC pipes, or any other backend
// failure is handled silently in Rust — the UI is not concerned.

import { invoke } from "@tauri-apps/api/core";
import { type Track } from "./api";

/** Connection state mirrored from `crate::discord::DiscordStatus`. */
export type DiscordStatus =
  | "disabled"
  | "no_client_id"
  | "connecting"
  | "connected";

/** Payload mirrored 1:1 from `crate::discord::TrackPresence`. */
interface TrackPresencePayload {
  title: string;
  artist: string;
  album: string;
  cover_url: string | null;
  /** Local cache key (e.g. `aa/<hash>.jpg`). The backend hosts the
   *  bytes on a public URL and patches the activity in place once
   *  the upload finishes, so the album art the user sees in Qobee
   *  ends up on Discord too. */
  cover_key: string | null;
  duration_seconds: number;
  position_seconds: number;
}

class DiscordPresenceImpl {
  private started = false;
  private enabled = true;
  /** Last full payload we sent, used to skip no-op IPC calls. */
  private lastSentKey: string | null = null;

  /** Boot the backend worker. Idempotent; safe to call repeatedly. */
  async init(): Promise<void> {
    if (this.started) return;
    this.started = true;
    try {
      await invoke("discord_init");
    } catch {
      // Silent fallback: backend command surface should never refuse
      // this call, but if Tauri isn't ready yet we just retry on the
      // next caller event.
      this.started = false;
    }
  }

  /**
   * Set the Discord Application ID used for the IPC handshake. An
   * empty string means "use the default app id baked into the
   * binary" — useful when the user clears the field in Settings to
   * revert to the public Qobee app.
   */
  async setClientId(clientId: string): Promise<void> {
    try {
      await invoke("discord_set_client_id", { clientId: clientId.trim() });
      // Force the next updateTrack to bypass the dedup so the new
      // application's art/identity is applied right away.
      this.lastSentKey = null;
    } catch {
      // ignored
    }
  }

  /** Snapshot the worker's current connection state. */
  async getStatus(): Promise<DiscordStatus> {
    try {
      return (await invoke<DiscordStatus>("discord_status")) ?? "disabled";
    } catch {
      return "disabled";
    }
  }

  /**
   * Allow / forbid the background cover-host worker to upload local
   * album art to a public service so Discord can display it. Off by
   * default for privacy — must be opt-in.
   */
  async setCoverUploadEnabled(on: boolean): Promise<void> {
    try {
      await invoke("discord_set_cover_upload_enabled", { enabled: on });
    } catch {
      // ignored
    }
  }

  /**
   * Push the currently playing track. The frontend resolves cover
   * art to a public URL when one is available; local file paths and
   * the `qobee-cover://` custom protocol cannot be rendered by
   * Discord, so they are dropped (`cover_url: null`) and the
   * default app artwork shows instead.
   */
  async updateTrack(
    track: Track | null,
    positionSeconds: number,
    paused: boolean
  ): Promise<void> {
    if (!this.enabled) {
      await this.clearPresence();
      return;
    }
    if (!this.started) await this.init();

    if (!track) {
      await this.clearPresence();
      return;
    }

    const payload: TrackPresencePayload = {
      title: track.title || "Unknown Title",
      artist: track.artist || "Unknown Artist",
      album: track.album || "",
      cover_url: resolvePublicCover(track),
      cover_key: track.cover_key && track.cover_key.length > 0 ? track.cover_key : null,
      duration_seconds: Math.max(0, track.duration_seconds || 0),
      position_seconds: Math.max(0, positionSeconds || 0),
    };

    // Coalesce: skip the round-trip if nothing material changed.
    // Position drift inside a single track doesn't need to flow
    // through IPC — the backend already derives the elapsed bar
    // from a wall-clock anchor, so re-sending the same payload would
    // just bump Discord's rate limit for no visual benefit.
    const key = serializeKey(payload, paused);
    if (key === this.lastSentKey) return;
    this.lastSentKey = key;

    try {
      await invoke("discord_update_track", { track: payload, paused });
    } catch {
      // Reset the dedup key so the next call retries.
      this.lastSentKey = null;
    }
  }

  /**
   * Toggle play / paused without touching the track metadata. Cheap
   * — bypasses the dedup so the play/paused badge updates instantly.
   */
  async setPaused(paused: boolean): Promise<void> {
    if (!this.enabled) return;
    if (!this.started) await this.init();
    // Append to the dedup key so a later updateTrack with the same
    // metadata doesn't get suppressed.
    if (this.lastSentKey) this.lastSentKey += `|p=${paused ? 1 : 0}`;
    try {
      await invoke("discord_set_paused", { paused });
    } catch {
      // ignored, see init()
    }
  }

  /** Hide the rich presence. The backend stays connected. */
  async clearPresence(): Promise<void> {
    this.lastSentKey = null;
    if (!this.started) return;
    try {
      await invoke("discord_clear_presence");
    } catch {
      // ignored
    }
  }

  /**
   * Master switch driven by the user setting. When turned off, the
   * presence is cleared immediately; when turned back on, the next
   * `updateTrack` re-publishes.
   */
  async setEnabled(on: boolean): Promise<void> {
    if (this.enabled === on) return;
    this.enabled = on;
    if (!on) {
      await this.clearPresence();
    }
  }
}

/**
 * Discord renders artwork from either a registered asset key or a
 * publicly reachable URL. The frontend cannot host local images, so
 * we only forward `cover_url` when the track already references an
 * `https://` URL (e.g. a future remote-cover feature). For local
 * library covers we leave `cover_url: null` and let the backend's
 * cover host upload the bytes and patch the activity in place.
 */
function resolvePublicCover(track: Track): string | null {
  if (!track.cover_key) return null;
  const trimmed = track.cover_key.trim();
  if (/^https?:\/\//i.test(trimmed)) return trimmed;
  return null;
}

function serializeKey(payload: TrackPresencePayload, paused: boolean): string {
  // 5-second buckets on the wall-clock start so seeks larger than
  // that trigger a republish, while sub-second jitter (the position
  // event fires at decoder cadence) does not.
  const startBucket = Math.floor(
    ((Date.now() / 1000) - payload.position_seconds) / 5
  );
  return [
    payload.title,
    payload.artist,
    payload.album,
    payload.cover_url ?? "",
    payload.cover_key ?? "",
    Math.round(payload.duration_seconds),
    startBucket,
    paused ? "1" : "0",
  ].join("\u0001");
}

export const discordPresence = new DiscordPresenceImpl();
