// Volume mapping helpers — TS counterpart of `map_volume` in
// `crates/engine/src/rodio_engine.rs`.
//
// The simplified engine maps the slider position linearly to the
// output gain (identity, clamped). The user-selectable volume curve and
// floor were removed with the audiophile stack.
//
//   * slider = 0 → linear 0 (mute) / "0%"
//   * slider = 1 → linear 1 (unity) / "100%"

/** Translate a slider position in `[0, 1]` into the linear gain. */
export function sliderToLinear(slider: number): number {
  if (Number.isNaN(slider) || slider <= 0) return 0;
  if (slider >= 1) return 1;
  return slider;
}

/** Render a slider position as a user-facing percentage string. */
export function formatVolumePercent(slider: number): string {
  const clamped = Math.min(1, Math.max(0, Number.isNaN(slider) ? 0 : slider));
  return `${Math.round(clamped * 100)}%`;
}
