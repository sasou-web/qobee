// Single source of truth for player state in the UI.
//
// Historically the UI carried two parallel player stores: the legacy
// `app.player` (fed by the `player:state` / `player:position` Tauri
// topics) and this `playerStore` (fed by the R8 transport bus,
// `player:event`). They could drift — e.g. a PlayButton thinking a
// track played while the PlayerBar showed it paused — because
// different components read different stores.
//
// This store now unifies BOTH feeds into one object that every
// component reads:
//
//   - `player:event`  (transport bus) → `apply()`        — status,
//     track metadata, position, scoped errors.
//   - `player:state`  (legacy topic)  → `applyState()`   — format /
//     quality fields the bus does not carry (volume, sample rate,
//     bit depth) plus the boot-restore (paused) case which never
//     emits a bus event.
//   - `player:position` (legacy topic) → `applyPosition()` — timeline.
//
// The two transport feeds agree on every transition (they both
// reflect the same engine), so "last writer wins" is safe: there is
// no visible divergence because there is only one store.
//
// `init()` owns every player listener and is idempotent, so both the
// main window (App.svelte) and the separate mini-player window
// (MiniPlayer.svelte) can call it.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  getPlayerState,
  type PlayerState,
} from "./api";
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

/** Map a backend `PlaybackStatus` (which uses `errored`) onto the
 *  store's status (which uses `error`). All other variants are
 *  identical. */
function mapStatus(s: PlayerState["status"]): PlayerStoreStatus {
  return s === "errored" ? "error" : s;
}

class PlayerStore {
  // --- Transport (bus + state) ------------------------------------

  /** Coarse playback status. `loading` is reserved for transient
   *  states the UI sets locally between an `invoke("play")` and
   *  the `Started` event coming back; the bus itself never emits
   *  `loading`. */
  status = $state<PlayerStoreStatus>("idle");

  /** Currently loaded track metadata (title/artist/album/duration),
   *  from the transport bus. Null while idle / stopped, and also
   *  during a boot-restore until playback starts (the restore path
   *  does not emit a bus event — `currentTrackId` still reflects it). */
  currentTrack = $state<TrackMetaDto | null>(null);

  /** String id of the loaded track. Driven by BOTH the bus and
   *  `player:state`, so it is populated even on a boot-restore
   *  (paused) where no bus event fires. Components that compare
   *  against `String(track.id)` read this. */
  currentTrackId = $state<string | null>(null);

  /** Last position reported by the engine, in milliseconds. */
  positionMs = $state<number>(0);

  /** Duration of the current track, in milliseconds. */
  durationMs = $state<number>(0);

  /** Last error scoped to a track id so the PlayButton can light
   *  up only on the row that failed. Cleared when a fresh
   *  `Started` event arrives for the same track. */
  lastError = $state<PlayerStoreError | null>(null);

  // --- Format / quality (from `player:state` only) ----------------

  /** Software volume in `[0, 1]`. */
  volume = $state<number>(1);

  /** Negotiated output sample rate, or null while idle. */
  sampleRate = $state<number | null>(null);

  /** Negotiated output bit depth, or null while idle / unknown. */
  bitDepth = $state<number | null>(null);

  /** Negotiated output channel count, or null while idle. */
  channels = $state<number | null>(null);

  // --- Convenience getters (seconds) ------------------------------

  /** Current position in seconds (mirrors the legacy
   *  `position_seconds` field many components read). */
  get positionSeconds(): number {
    return this.positionMs / 1000;
  }

  /** Current duration in seconds. */
  get durationSeconds(): number {
    return this.durationMs / 1000;
  }

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
   *  - legacy variants  → no-op (handled by `applyState` /
   *                        `applyPosition`) */
  apply(ev: PlayerEventDto): void {
    switch (ev.type) {
      case "started": {
        this.currentTrack = ev.track;
        this.currentTrackId = String(ev.track.id);
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
        this.currentTrackId = null;
        break;
      }
      case "track_changed": {
        this.currentTrack = ev.track;
        this.currentTrackId = String(ev.track.id);
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
      // `end_of_track`, `error`) are intentionally ignored here:
      // they ride the same Tauri channel but are consumed by
      // `applyState` / `applyPosition` and the app-level error banner.
      default:
        break;
    }
  }

  /** Apply a full `PlayerState` snapshot from the legacy
   *  `player:state` topic (or the one-shot `getPlayerState()` seed).
   *  Carries the format/quality fields the transport bus does not,
   *  and is the only feed that reflects a boot-restored paused
   *  track. */
  applyState(s: PlayerState): void {
    this.status = mapStatus(s.status);
    this.currentTrackId = s.current_track_id;
    this.positionMs = Math.max(0, s.position_seconds * 1000);
    this.durationMs = Math.max(0, s.duration_seconds * 1000);
    this.volume = s.volume;
    this.sampleRate = s.sample_rate;
    this.bitDepth = s.bit_depth;
    this.channels = s.channels;
  }

  /** Apply a position-only update from the legacy `player:position`
   *  topic. */
  applyPosition(positionSeconds: number): void {
    this.positionMs = Math.max(0, positionSeconds * 1000);
  }

  // --- Lifecycle --------------------------------------------------

  private unlisten: Array<UnlistenFn> = [];
  private wired = false;

  /** Wire every player listener. Idempotent: safe to call from both
   *  the main window and the mini-player window. Seeds the store
   *  with a one-shot `getPlayerState()` so a freshly-opened window
   *  reflects whatever is already loaded.
   *
   *  Every step is individually fault-tolerant: a failure to seed or
   *  to register a listener (e.g. outside a Tauri runtime, as in unit
   *  tests or `vite preview`) is swallowed so the rest of the UI
   *  wiring keeps going. */
  async init(): Promise<void> {
    if (this.wired) return;
    this.wired = true;

    try {
      this.applyState(await getPlayerState());
    } catch {
      // best-effort seed; live events will fill the store in.
    }

    await this.safeListen("player:event", (payload) => this.apply(payload));
    await this.safeListen("player:state", (payload) => {
      if (payload.type === "state_changed") {
        this.applyState(payload.state as PlayerState);
      }
    });
    await this.safeListen("player:position", (payload) => {
      if (payload.type === "position") {
        this.applyPosition(payload.position_seconds);
      }
    });
  }

  /** Register one `player:*` listener, tolerating a missing Tauri
   *  runtime (tests / `vite preview`). */
  private async safeListen(
    topic: string,
    handle: (payload: PlayerEventDto) => void
  ): Promise<void> {
    try {
      const un = await listen<PlayerEventDto>(topic, (e) => handle(e.payload));
      this.unlisten.push(un);
    } catch {
      // No Tauri runtime — the store still works as a plain reactive
      // object driven by direct mutations (e.g. in tests).
    }
  }

  /** Tear down listeners. Mainly for tests / window teardown. */
  destroy(): void {
    for (const fn of this.unlisten) {
      try {
        fn();
      } catch {
        // ignore
      }
    }
    this.unlisten = [];
    this.wired = false;
  }
}

/** Singleton: there is exactly one player on a Qobee instance. */
export const playerStore = new PlayerStore();
