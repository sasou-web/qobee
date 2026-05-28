// Reactive store mirroring the R8 transport bus
// (`qobee_core::PlayerEvent`, see `crates/core/src/lib.rs`).
//
// The store is the single source of truth for the UI: every piece
// of "is this track playing?" UX (PlayButton, MiniPlayer, page
// glyphs…) reads from it. It is fed by a single `listen()` call
// in `App.svelte::onMount`; downstream components never subscribe
// to `player:event` themselves — that would multiply the listener
// across page mount/unmount cycles.

import type {
  PlayerEventDto,
  PlayerErrorKind,
  TrackMetaDto,
} from "./playerEvent";

export type PlayerStoreStatus =
  | "idle"
  | "loading"
  | "playing"
  | "paused"
  | "stopped"
  | "error";

/** Last error scoped to a specific track. `trackId` is null when
 *  the engine reported an error without a current track loaded
 *  (rare — boot-time device failure). */
export interface PlayerStoreError {
  trackId: number | null;
  kind: PlayerErrorKind;
  message: string;
}

/** Identifies what a UI surface (PlayButton on a row, an album
 *  card, the mini-player) is "about". The `kind` field is
 *  informational: track / album / playlist clicks all eventually
 *  resolve to a starting track on the engine side, so matching is
 *  done on track id alone. */
export interface PlayerTarget {
  kind: "track" | "album" | "playlist";
  id: number;
}

class PlayerStore {
  /** Coarse playback status. `loading` is reserved for transient
   *  states the UI sets locally between an `invoke("play")` and
   *  the `Started` event coming back; the bus itself never emits
   *  `loading`. */
  status = $state<PlayerStoreStatus>("idle");

  /** Currently loaded track (null while idle / stopped). */
  currentTrack = $state<TrackMetaDto | null>(null);

  /** Last position reported by the engine, in milliseconds. */
  positionMs = $state<number>(0);

  /** Duration of the current track, in milliseconds. */
  durationMs = $state<number>(0);

  /** Last error scoped to a track id so the PlayButton can light
   *  up only on the row that failed. Cleared when a fresh
   *  `Started` event arrives for the same track. */
  lastError = $state<PlayerStoreError | null>(null);

  /** True when the given UI target points at the track we
   *  currently have loaded. Used by the PlayButton to decide
   *  whether it owns the global play state or starts a new
   *  playback. The `kind` field is intentionally ignored: an
   *  album card and a track row that resolve to the same starting
   *  track id are considered "the same" for the purposes of the
   *  play/pause toggle. */
  matches(target: PlayerTarget): boolean {
    return this.currentTrack !== null && this.currentTrack.id === target.id;
  }

  /** Apply a single transport event from the broadcast bus.
   *
   *  Mapping (source: `qobee_core::PlayerEvent` + design.md
   *  §Player_Sync):
   *  - `started`        → playing, snapshot track & timeline,
   *                        clear lastError if it pointed at this
   *                        same track (a retry "fixed" the error)
   *  - `paused`         → paused, keep currentTrack & duration
   *  - `resumed`        → playing, keep currentTrack & duration
   *  - `stopped`        → stopped, keep last seen track for UI
   *                        history (PlayButton hides anyway)
   *  - `track_changed`  → swap track & duration, reset position,
   *                        keep playing if we were already
   *                        running (anything but stopped/idle)
   *  - `position_tick`  → just the timeline cursor
   *  - `errored`        → error, scope to currentTrack?.id
   *  - legacy variants  → no-op (handled by other channels) */
  apply(ev: PlayerEventDto): void {
    switch (ev.type) {
      case "started": {
        this.currentTrack = ev.track;
        this.positionMs = ev.position_ms;
        this.durationMs = ev.duration_ms;
        this.status = "playing";
        if (this.lastError && this.lastError.trackId === ev.track.id) {
          this.lastError = null;
        }
        break;
      }
      case "paused": {
        this.positionMs = ev.position_ms;
        this.status = "paused";
        break;
      }
      case "resumed": {
        this.positionMs = ev.position_ms;
        this.status = "playing";
        break;
      }
      case "stopped": {
        this.status = "stopped";
        break;
      }
      case "track_changed": {
        this.currentTrack = ev.track;
        this.durationMs = ev.track.duration_ms;
        this.positionMs = 0;
        // Preserve "playing" momentum across a gapless transition;
        // only `stopped` / `idle` should not silently re-arm.
        if (this.status !== "stopped" && this.status !== "idle") {
          this.status = "playing";
        }
        break;
      }
      case "position_tick": {
        this.positionMs = ev.position_ms;
        break;
      }
      case "errored": {
        this.lastError = {
          trackId: this.currentTrack?.id ?? null,
          kind: ev.kind,
          message: ev.message,
        };
        this.status = "error";
        break;
      }
      // Legacy variants (`state_changed`, `position`,
      // `end_of_track`, `bit_perfect_changed`, `error`) are
      // intentionally ignored: they share the same Tauri channel
      // but are consumed by other subscribers (the legacy
      // `stores.svelte.ts`, the bit-perfect topic, etc.).
      default:
        break;
    }
  }
}

/** Singleton: there is exactly one player on a Qobee instance. */
export const playerStore = new PlayerStore();
