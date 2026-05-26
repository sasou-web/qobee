// Volume curve helpers — TS counterpart of `crates/engine/src/volume.rs`.
//
// The Rust side owns the canonical implementation; this file mirrors
// it sample-for-sample so the player bar can update its dB readout
// every position tick without round-tripping through Tauri. Property
// 31 (`crates/engine/tests/properties/volume.rs`) pins both sides to
// the same contract:
//
//   * `slider = 0` → "-inf dB" / linear 0
//   * `slider = 1` → "0.0 dB" / linear 1
//   * Logarithmic: `gain_db = lerp(floor_db, 0, slider)`
//   * Quadratic:   `gain = slider * slider`
//
// Keep the function signatures and the rounding rule
// (`round1 = (x * 10).round() / 10`, with signed-zero normalisation)
// identical to the Rust side.

export type VolumeCurve = "logarithmic" | "quadratic";

const UNITY_EPS = 1e-9;

/** Convert a slider position to dB. Mirrors `volume::slider_to_db`. */
export function sliderToDb(
  slider: number,
  curve: VolumeCurve,
  floorDb: number
): number {
  if (Number.isNaN(slider) || slider <= 0) return Number.NEGATIVE_INFINITY;
  if (Math.abs(slider - 1) < UNITY_EPS) return 0;
  const v = Math.min(1, Math.max(0, slider));
  return curve === "quadratic"
    ? 20 * Math.log10(v * v)
    : floorDb + (0 - floorDb) * v;
}

/** Inverse of `sliderToDb`. Mirrors `volume::db_to_slider`. */
export function dbToSlider(
  db: number,
  curve: VolumeCurve,
  floorDb: number
): number {
  if (!Number.isFinite(db)) return db === Number.NEGATIVE_INFINITY ? 0 : 1;
  if (db >= 0) return 1;
  if (db <= floorDb) return 0;
  const v =
    curve === "quadratic"
      ? Math.pow(10, db / 40)
      : (db - floorDb) / (0 - floorDb);
  return Math.min(1, Math.max(0, v));
}

/** Translate a slider position into the linear gain. */
export function sliderToLinear(
  slider: number,
  curve: VolumeCurve,
  floorDb: number
): number {
  if (Number.isNaN(slider) || slider <= 0) return 0;
  if (Math.abs(slider - 1) < UNITY_EPS) return 1;
  const v = Math.min(1, Math.max(0, slider));
  if (curve === "quadratic") return v * v;
  const db = floorDb + (0 - floorDb) * v;
  return Math.pow(10, db / 20);
}

/** Bump the slider by `deltaDb`. Mirrors `volume::step_db`. */
export function stepDb(
  slider: number,
  deltaDb: number,
  curve: VolumeCurve,
  floorDb: number
): number {
  const cur = sliderToDb(slider, curve, floorDb);
  // -inf + finite = -inf; clamping then lifts to floor_db, which
  // db_to_slider maps to 0 (mute) so a muted slider stays muted on a
  // positive media-key step. Matches the Rust contract verbatim.
  const next = Math.min(0, Math.max(floorDb, cur + deltaDb));
  return Math.min(1, Math.max(0, dbToSlider(next, curve, floorDb)));
}

/**
 * Render a slider position as the user-facing dB string used by the
 * volume display. Matches `volume::format_db_for_display`:
 *   * `slider = 0`         → `"-inf dB"`
 *   * `slider = 1`         → `"0.0 dB"`
 *   * other                → one-decimal dB with signed-zero
 *                            normalised to `"0.0 dB"`.
 */
export function formatVolumeDb(
  slider: number,
  curve: VolumeCurve,
  floorDb: number
): string {
  const db = sliderToDb(slider, curve, floorDb);
  if (db === Number.NEGATIVE_INFINITY) return "-inf dB";
  const rounded = Math.round(db * 10) / 10;
  // `Object.is(-0, 0)` is `false`; normalise so we never display
  // "-0.0 dB" for values that round to zero.
  const normalised = rounded === 0 ? 0 : rounded;
  return `${normalised.toFixed(1)} dB`;
}
