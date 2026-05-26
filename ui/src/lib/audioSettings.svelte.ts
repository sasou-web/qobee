// Reactive audio settings store (Svelte 5 runes).
//
// Mirrors the 19 `audio.*` keys persisted by `AudioSettingsStore` on
// the Rust side (`crates/core/src/audio_settings.rs`). Two-way bound
// from the AudioSettingsPanel: every UI control reads from this
// store and pushes mutations through `updateAudio(key, value)` —
// which calls `setAudioSetting` and patches the local state on
// success.
//
// The store keeps the previous `volumeSettings` surface alive
// (PlayerBar reads `curve` / `floorDb` from it on every position
// tick) so callers don't need to migrate at the same time as the
// settings page.

import {
  getAudioSetting,
  listAudioSettings,
  setAudioSetting,
} from "./api";
import type { VolumeCurve } from "./volumeFormat";

export type DitherProfile = "tpdf" | "shaped_hp" | "shaped_f_weighted";
export type PeakLimiterMode = "off" | "soft_clip" | "lookahead_limiter";
export type CrossfeedPreset = "bauer" | "bauer_strong" | "custom";
export type ResamplerQuality = "standard" | "best";

export interface AudioSettingsState {
  // R1
  rg_peak_protection: boolean;
  rg_safety_headroom_db: number;
  // R2
  dither_profile: DitherProfile;
  // R3
  peak_limiter_mode: PeakLimiterMode;
  peak_limiter_ceiling_dbfs: number;
  peak_limiter_lookahead_ms: number;
  peak_limiter_release_ms: number;
  // R4
  volume_curve: VolumeCurve;
  volume_floor_db: number;
  // R6
  crossfeed_enabled: boolean;
  crossfeed_preset: CrossfeedPreset;
  crossfeed_delay_us: number;
  crossfeed_lp_cutoff_hz: number;
  // R8
  resampler_quality: ResamplerQuality;
  // R9
  convolver_enabled: boolean;
  convolver_ir_path: string | null;
  convolver_gain_db: number;
  // R10
  balance: number;
  trim_db_per_channel: number[];
  // metadata
  loaded: boolean;
}

/** Spec defaults — kept in sync with `AudioSettings::default()`. */
export const AUDIO_DEFAULTS: AudioSettingsState = {
  rg_peak_protection: true,
  rg_safety_headroom_db: 1.0,
  dither_profile: "shaped_f_weighted",
  peak_limiter_mode: "lookahead_limiter",
  peak_limiter_ceiling_dbfs: -1.0,
  peak_limiter_lookahead_ms: 5.0,
  peak_limiter_release_ms: 100.0,
  volume_curve: "logarithmic",
  volume_floor_db: -60.0,
  crossfeed_enabled: false,
  crossfeed_preset: "bauer",
  crossfeed_delay_us: 300.0,
  crossfeed_lp_cutoff_hz: 700.0,
  resampler_quality: "best",
  convolver_enabled: false,
  convolver_ir_path: null,
  convolver_gain_db: -6.0,
  balance: 0.0,
  trim_db_per_channel: [],
  loaded: false,
};

const state = $state<AudioSettingsState>({ ...AUDIO_DEFAULTS });
let inflight: Promise<void> | null = null;

// -- Helpers -----------------------------------------------------------------

function isDitherProfile(v: unknown): v is DitherProfile {
  return v === "tpdf" || v === "shaped_hp" || v === "shaped_f_weighted";
}
function isPeakLimiterMode(v: unknown): v is PeakLimiterMode {
  return v === "off" || v === "soft_clip" || v === "lookahead_limiter";
}
function isVolumeCurve(v: unknown): v is VolumeCurve {
  return v === "logarithmic" || v === "quadratic";
}
function isCrossfeedPreset(v: unknown): v is CrossfeedPreset {
  return v === "bauer" || v === "bauer_strong" || v === "custom";
}
function isResamplerQuality(v: unknown): v is ResamplerQuality {
  return v === "standard" || v === "best";
}
function asFiniteNumber(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}
function asBool(v: unknown): boolean | null {
  return typeof v === "boolean" ? v : null;
}

/**
 * Patch the in-memory snapshot from a JSON value coming back from
 * the engine. Unknown / wrong-typed payloads are ignored so the spec
 * defaults already in `state` survive a partial response.
 */
function patchFromMap(map: Record<string, unknown>): void {
  const b = (k: keyof AudioSettingsState, v: unknown) => {
    const x = asBool(v);
    if (x !== null) (state as unknown as Record<string, unknown>)[k] = x;
  };
  const n = (k: keyof AudioSettingsState, v: unknown) => {
    const x = asFiniteNumber(v);
    if (x !== null) (state as unknown as Record<string, unknown>)[k] = x;
  };

  b("rg_peak_protection", map["audio.rg_peak_protection"]);
  n("rg_safety_headroom_db", map["audio.rg_safety_headroom_db"]);

  if (isDitherProfile(map["audio.dither_profile"]))
    state.dither_profile = map["audio.dither_profile"];

  if (isPeakLimiterMode(map["audio.peak_limiter_mode"]))
    state.peak_limiter_mode = map["audio.peak_limiter_mode"];
  n("peak_limiter_ceiling_dbfs", map["audio.peak_limiter_ceiling_dbfs"]);
  n("peak_limiter_lookahead_ms", map["audio.peak_limiter_lookahead_ms"]);
  n("peak_limiter_release_ms", map["audio.peak_limiter_release_ms"]);

  if (isVolumeCurve(map["audio.volume_curve"]))
    state.volume_curve = map["audio.volume_curve"];
  n("volume_floor_db", map["audio.volume_floor_db"]);

  b("crossfeed_enabled", map["audio.crossfeed_enabled"]);
  if (isCrossfeedPreset(map["audio.crossfeed_preset"]))
    state.crossfeed_preset = map["audio.crossfeed_preset"];
  n("crossfeed_delay_us", map["audio.crossfeed_delay_us"]);
  n("crossfeed_lp_cutoff_hz", map["audio.crossfeed_lp_cutoff_hz"]);

  if (isResamplerQuality(map["audio.resampler_quality"]))
    state.resampler_quality = map["audio.resampler_quality"];

  b("convolver_enabled", map["audio.convolver_enabled"]);
  const ir = map["audio.convolver_ir_path"];
  state.convolver_ir_path =
    typeof ir === "string" && ir.length > 0 ? ir : null;
  n("convolver_gain_db", map["audio.convolver_gain_db"]);

  n("balance", map["audio.balance"]);
  const trim = map["audio.trim_db_per_channel"];
  if (Array.isArray(trim)) {
    state.trim_db_per_channel = trim
      .filter((x): x is number => typeof x === "number" && Number.isFinite(x))
      .slice(0, 8);
  }
}

// -- Load --------------------------------------------------------------------

/**
 * Fetch the full snapshot from the engine in a single round-trip.
 * Subsequent calls are no-ops; pass `force = true` from a "reset to
 * defaults" handler to refresh the cache.
 */
export async function loadAudioSettings(force = false): Promise<void> {
  if (state.loaded && !force) return;
  if (inflight) return inflight;
  inflight = (async () => {
    try {
      const map = await listAudioSettings();
      patchFromMap(map);
    } catch {
      // Best-effort: keep the spec defaults already on `state`.
    } finally {
      state.loaded = true;
      inflight = null;
    }
  })();
  return inflight;
}

// -- Update ------------------------------------------------------------------

/**
 * The exhaustive map between every `audio.*` key and the matching
 * field in the local store. Used by `updateAudio` to patch the
 * cached snapshot after a successful write without re-listing the
 * full settings table.
 */
type Patcher<V> = (v: V) => void;
const PATCHERS: Record<string, Patcher<unknown>> = {
  "audio.rg_peak_protection": (v) => {
    if (typeof v === "boolean") state.rg_peak_protection = v;
  },
  "audio.rg_safety_headroom_db": (v) => {
    if (typeof v === "number") state.rg_safety_headroom_db = v;
  },
  "audio.dither_profile": (v) => {
    if (isDitherProfile(v)) state.dither_profile = v;
  },
  "audio.peak_limiter_mode": (v) => {
    if (isPeakLimiterMode(v)) state.peak_limiter_mode = v;
  },
  "audio.peak_limiter_ceiling_dbfs": (v) => {
    if (typeof v === "number") state.peak_limiter_ceiling_dbfs = v;
  },
  "audio.peak_limiter_lookahead_ms": (v) => {
    if (typeof v === "number") state.peak_limiter_lookahead_ms = v;
  },
  "audio.peak_limiter_release_ms": (v) => {
    if (typeof v === "number") state.peak_limiter_release_ms = v;
  },
  "audio.volume_curve": (v) => {
    if (isVolumeCurve(v)) state.volume_curve = v;
  },
  "audio.volume_floor_db": (v) => {
    if (typeof v === "number") state.volume_floor_db = v;
  },
  "audio.crossfeed_enabled": (v) => {
    if (typeof v === "boolean") state.crossfeed_enabled = v;
  },
  "audio.crossfeed_preset": (v) => {
    if (isCrossfeedPreset(v)) state.crossfeed_preset = v;
  },
  "audio.crossfeed_delay_us": (v) => {
    if (typeof v === "number") state.crossfeed_delay_us = v;
  },
  "audio.crossfeed_lp_cutoff_hz": (v) => {
    if (typeof v === "number") state.crossfeed_lp_cutoff_hz = v;
  },
  "audio.resampler_quality": (v) => {
    if (isResamplerQuality(v)) state.resampler_quality = v;
  },
  "audio.convolver_enabled": (v) => {
    if (typeof v === "boolean") state.convolver_enabled = v;
  },
  "audio.convolver_ir_path": (v) => {
    state.convolver_ir_path =
      typeof v === "string" && v.length > 0 ? v : null;
  },
  "audio.convolver_gain_db": (v) => {
    if (typeof v === "number") state.convolver_gain_db = v;
  },
  "audio.balance": (v) => {
    if (typeof v === "number") state.balance = v;
  },
  "audio.trim_db_per_channel": (v) => {
    if (Array.isArray(v)) {
      state.trim_db_per_channel = v
        .filter(
          (x): x is number => typeof x === "number" && Number.isFinite(x),
        )
        .slice(0, 8);
    }
  },
};

/**
 * Validate, persist, and apply a single setting in one call. Returns
 * `null` on success or the engine's error message when validation
 * fails on the Rust side. The cached snapshot is patched only after
 * the engine accepts the value.
 */
export async function updateAudio(
  key: string,
  value: unknown,
): Promise<string | null> {
  try {
    await setAudioSetting(key, value);
    PATCHERS[key]?.(value);
    return null;
  } catch (e) {
    return String(e);
  }
}

/** Force-refresh a single key from the engine (round-trip). */
export async function refreshAudio(key: string): Promise<void> {
  try {
    const v = await getAudioSetting<unknown>(key);
    PATCHERS[key]?.(v);
  } catch {
    /* swallow */
  }
}

// -- Public façade -----------------------------------------------------------

/**
 * Read-only reactive view. Sub-fields are exposed via getters so
 * `$derived(audioSettings.balance)` reacts to mutations.
 */
export const audioSettings = {
  get rg_peak_protection() {
    return state.rg_peak_protection;
  },
  get rg_safety_headroom_db() {
    return state.rg_safety_headroom_db;
  },
  get dither_profile() {
    return state.dither_profile;
  },
  get peak_limiter_mode() {
    return state.peak_limiter_mode;
  },
  get peak_limiter_ceiling_dbfs() {
    return state.peak_limiter_ceiling_dbfs;
  },
  get peak_limiter_lookahead_ms() {
    return state.peak_limiter_lookahead_ms;
  },
  get peak_limiter_release_ms() {
    return state.peak_limiter_release_ms;
  },
  get volume_curve() {
    return state.volume_curve;
  },
  get volume_floor_db() {
    return state.volume_floor_db;
  },
  get crossfeed_enabled() {
    return state.crossfeed_enabled;
  },
  get crossfeed_preset() {
    return state.crossfeed_preset;
  },
  get crossfeed_delay_us() {
    return state.crossfeed_delay_us;
  },
  get crossfeed_lp_cutoff_hz() {
    return state.crossfeed_lp_cutoff_hz;
  },
  get resampler_quality() {
    return state.resampler_quality;
  },
  get convolver_enabled() {
    return state.convolver_enabled;
  },
  get convolver_ir_path() {
    return state.convolver_ir_path;
  },
  get convolver_gain_db() {
    return state.convolver_gain_db;
  },
  get balance() {
    return state.balance;
  },
  get trim_db_per_channel() {
    return state.trim_db_per_channel;
  },
  get loaded() {
    return state.loaded;
  },
  ensureLoaded: loadAudioSettings,
  refresh: () => loadAudioSettings(true),
  update: updateAudio,
  refreshKey: refreshAudio,
};

// -- Backwards-compatible volume facade --------------------------------------
//
// `PlayerBar.svelte` reads from `volumeSettings` on every tick. We
// keep that shape alive (curve / floorDb / ensureLoaded) so the
// older callers don't need to change.

export const volumeSettings = {
  get curve(): VolumeCurve {
    return state.volume_curve;
  },
  get floorDb(): number {
    return state.volume_floor_db;
  },
  get loaded(): boolean {
    return state.loaded;
  },
  ensureLoaded: loadAudioSettings,
};
