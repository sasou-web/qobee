// TypeScript mirror of the Rust transport bus enum
// `qobee_core::PlayerEvent` (see `crates/core/src/lib.rs`).
//
// We only model the *new* R8 transport variants that the player
// store consumes. The legacy variants (`state_changed`, `position`,
// `end_of_track`, `bit_perfect_changed`, `error`) ride the same
// channel but are handled by other callers (`stores.svelte.ts`,
// the bit-perfect topic, etc.) — we accept them in the union with
// `unknown` payload shapes so `apply()` can ignore them safely
// without TypeScript forcing us to model their innards.
//
// Sync policy: hand-written, kept tiny on purpose. If you change a
// variant in `crates/core/src/lib.rs::PlayerEvent`, mirror it here
// in the same PR.

/** Coarse classification of recoverable player errors. Mirrors
 *  `qobee_core::PlayerErrorKind`. */
export type PlayerErrorKind =
  | "file_not_found"
  | "decode_failed"
  | "device_unavailable"
  | "other";

/** Track metadata projected onto the R8 transport bus. Mirrors
 *  `qobee_core::TrackMeta`. `cover_bytes` is `#[serde(skip)]` on
 *  the wire so it never appears in the DTO. */
export interface TrackMetaDto {
  id: number;
  title: string;
  artist: string;
  album: string;
  duration_ms: number;
}

/** Discriminated union mirroring the new R8 variants of
 *  `qobee_core::PlayerEvent`. The legacy variants share the same
 *  `type` discriminant space and are intentionally typed loosely
 *  so they can be ignored by `playerStore.apply()`. */
export type PlayerEventDto =
  | {
      type: "started";
      track: TrackMetaDto;
      position_ms: number;
      duration_ms: number;
    }
  | { type: "paused"; position_ms: number }
  | { type: "resumed"; position_ms: number }
  | { type: "stopped" }
  | { type: "track_changed"; track: TrackMetaDto }
  | { type: "position_tick"; position_ms: number }
  | { type: "errored"; kind: PlayerErrorKind; message: string }
  // --- Legacy variants, handled by other channels. -----------------
  | { type: "state_changed"; state: unknown }
  | { type: "position"; position_seconds: number }
  | { type: "end_of_track" }
  | { type: "bit_perfect_changed"; health: unknown }
  | { type: "error"; message: string };
