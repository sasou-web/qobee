//! Volume curve helpers — slider ↔ dB ↔ linear gain (R4).
//!
//! All conversions agree on the boundary contract that anchors the
//! whole pre-gain pipeline:
//!
//!   * `v = 0.0` is **mute-exact** (`slider_to_linear == 0.0`,
//!     `slider_to_db == −∞`). The logarithmic curve does *not* leak a
//!     residual `10^(floor_db/20)` — that is what makes the volume
//!     slider's bottom position a true mute (R4.2).
//!   * `v = 1.0` is **unity-exact** (`slider_to_linear == 1.0`,
//!     `slider_to_db == 0.0`), regardless of the active curve. This is
//!     what lets the bit-perfect badge stay green at full volume
//!     (R4.3).
//!   * The mapping is **strictly monotone non-decreasing** on
//!     `(0.0, 1.0]` for both curves, so that nudging the slider by an
//!     arbitrarily small `ε` never lowers the audible gain.
//!
//! Two curves are supported and live behind [`VolumeCurve`]:
//!
//!   * [`VolumeCurve::Quadratic`] — legacy `gain = v²`. Kept for
//!     compatibility with builds that ran before the spec landed.
//!   * [`VolumeCurve::Logarithmic`] — `gain_db = lerp(floor_db, 0, v)`,
//!     i.e. linear in dB. Default per the spec; gives the audiophile
//!     "fine resolution at the bottom of the slider" feel and matches
//!     what foobar2000 / MusicBee do on their own logarithmic preset.
//!
//! The rest of the engine (decoder thread, callback ramp, UI) reads
//! the user's slider position and asks this module to translate it.
//! The module itself holds no state and performs no allocation outside
//! [`format_db_for_display`].

use crate::audio_settings::VolumeCurve;

/// Convert a slider position `v ∈ [0.0, 1.0]` to a dB value.
///
/// Returns:
///
///   * [`f32::NEG_INFINITY`] when `v ≤ 0.0` (R4.2 — mute is bottomless).
///   * `0.0` when `v ≈ 1.0` to within `1e-9` (R4.3 — unity is exact).
///   * Otherwise the dB value implied by the active curve. For
///     [`VolumeCurve::Logarithmic`] this is `lerp(floor_db, 0, v)`;
///     for [`VolumeCurve::Quadratic`] it is `20 * log10(v²)`.
pub fn slider_to_db(slider: f32, curve: VolumeCurve, floor_db: f32) -> f32 {
    if slider.is_nan() || slider <= 0.0 {
        // NaN and `v <= 0.0` both map to mute — the only safe answer
        // when the input is malformed or the slider is at the bottom.
        return f32::NEG_INFINITY;
    }
    if (slider - 1.0).abs() < 1e-9 {
        return 0.0;
    }
    let v = slider.clamp(0.0, 1.0);
    match curve {
        VolumeCurve::Quadratic => 20.0 * (v * v).log10(),
        VolumeCurve::Logarithmic => floor_db + (0.0 - floor_db) * v,
    }
}

/// Inverse of [`slider_to_db`]. Maps a dB value back to a slider
/// position in `[0.0, 1.0]`.
///
///   * `db ≤ floor_db` (or `-∞`) → `0.0` (mute).
///   * `db ≥ 0.0` → `1.0` (unity).
///   * Otherwise the slider position implied by inverting the curve.
///
/// The caller does not need to clamp `db` themselves: this function
/// always returns a value in `[0.0, 1.0]`.
pub fn db_to_slider(db: f32, curve: VolumeCurve, floor_db: f32) -> f32 {
    if !db.is_finite() {
        // `-inf` ⇒ mute, `+inf`/NaN ⇒ unity (defensive — neither
        // should happen via the normal slider/step paths).
        return if db == f32::NEG_INFINITY { 0.0 } else { 1.0 };
    }
    if db >= 0.0 {
        return 1.0;
    }
    if db <= floor_db {
        return 0.0;
    }
    let v = match curve {
        // db = 20 * log10(v²) ⇒ v = 10^(db / 40).
        VolumeCurve::Quadratic => 10f32.powf(db / 40.0),
        // db = floor + (0 - floor) * v ⇒ v = (db - floor) / (-floor).
        VolumeCurve::Logarithmic => (db - floor_db) / (0.0 - floor_db),
    };
    v.clamp(0.0, 1.0)
}

/// Translate a slider position into the linear gain that the
/// pre-gain stage should apply.
///
///   * `v = 0.0` → `0.0` (mute-exact, R4.2).
///   * `v ≈ 1.0` → `1.0` (unity-exact, R4.3).
///   * Otherwise `10^(slider_to_db / 20)`.
///
/// Used by both [`crate::dsp::pre_gain::PreGainStage`] when it
/// recomputes its combined gain and by the audio callback's volume
/// ramp target (task 22).
pub fn slider_to_linear(slider: f32, curve: VolumeCurve, floor_db: f32) -> f32 {
    if slider.is_nan() || slider <= 0.0 {
        return 0.0;
    }
    if (slider - 1.0).abs() < 1e-9 {
        return 1.0;
    }
    let v = slider.clamp(0.0, 1.0);
    match curve {
        VolumeCurve::Quadratic => v * v,
        VolumeCurve::Logarithmic => {
            let db = floor_db + (0.0 - floor_db) * v;
            10f32.powf(db / 20.0)
        }
    }
}

/// Bump the slider by `delta_db` (positive = louder, negative =
/// quieter) and return the new slider position.
///
/// The algorithm mirrors the design's pseudocode:
///
/// ```text
/// new_db = clamp(slider_to_db(v) + delta_db, floor_db, 0.0)
/// new_v  = db_to_slider(new_db).clamp(0.0, 1.0)
/// ```
///
/// Boundary behaviour:
///
///   * Stepping up from unity stays at unity (`new_db` capped at
///     `0.0`).
///   * Stepping down past the floor pins the slider at mute
///     (`new_db` capped at `floor_db`, which `db_to_slider` maps
///     back to `v = 0`).
///   * Stepping out of mute by a positive delta does *not* unmute:
///     `db_to_slider(floor_db) == 0` for both curves, so a muted
///     slider stays muted under media-key stepping. The user has to
///     drag the slider above zero by direct interaction first.
///
/// Used by media-key handlers (UI task 29) where each keypress is
/// "+2 dB" / "−2 dB".
pub fn step_db(slider: f32, delta_db: f32, curve: VolumeCurve, floor_db: f32) -> f32 {
    let cur_db = slider_to_db(slider, curve, floor_db);
    // `(-inf) + finite = -inf`, then `max(floor_db)` lifts it to the
    // floor — exactly what we want when stepping up from mute.
    let new_db = (cur_db + delta_db).min(0.0).max(floor_db);
    db_to_slider(new_db, curve, floor_db).clamp(0.0, 1.0)
}

/// Render a slider position as the user-facing dB string used by the
/// volume display (Property 31).
///
///   * `v = 0.0` → `"-inf dB"`.
///   * `v ≈ 1.0` → `"0.0 dB"` (unity-exact).
///   * Otherwise the dB value rounded to one decimal place, e.g.
///     `"-12.3 dB"`. The `-0.0` corner case is normalised to
///     `"0.0 dB"` so the slider never displays a phantom minus sign
///     on values that round to zero.
///
/// The string format `<-?\d+\.\d> dB` is what the UI's TS
/// counterpart (`ui/src/lib/volumeFormat.ts`, task 29) reproduces so
/// the player bar can update without round-tripping through Tauri.
pub fn format_db_for_display(slider: f32, curve: VolumeCurve, floor_db: f32) -> String {
    let db = slider_to_db(slider, curve, floor_db);
    if db == f32::NEG_INFINITY {
        return "-inf dB".to_string();
    }
    // Round to one decimal place, then erase any signed zero that
    // would otherwise produce "-0.0 dB" for values that round to 0.
    let rounded = (db * 10.0).round() / 10.0;
    let normalised = if rounded == 0.0 { 0.0 } else { rounded };
    format!("{normalised:.1} dB")
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: f32 = -60.0;

    // ---- slider_to_db -------------------------------------------------------

    #[test]
    fn slider_to_db_zero_is_negative_infinity_for_both_curves() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            assert_eq!(slider_to_db(0.0, curve, FLOOR), f32::NEG_INFINITY);
            // Negative slider values defensively map to mute too.
            assert_eq!(slider_to_db(-0.5, curve, FLOOR), f32::NEG_INFINITY);
            // NaN is neither > 0 nor < 0, so the `!(slider > 0.0)`
            // guard catches it.
            assert_eq!(slider_to_db(f32::NAN, curve, FLOOR), f32::NEG_INFINITY);
        }
    }

    #[test]
    fn slider_to_db_one_is_exactly_zero_for_both_curves() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            assert_eq!(slider_to_db(1.0, curve, FLOOR), 0.0);
        }
    }

    #[test]
    fn slider_to_db_logarithmic_lerps_floor_to_zero() {
        // gain_db = floor + (0 - floor) * v, with floor = -60.
        // v=0.5 ⇒ -30 dB, v=0.25 ⇒ -45 dB, v=0.75 ⇒ -15 dB.
        let curve = VolumeCurve::Logarithmic;
        assert!((slider_to_db(0.5, curve, FLOOR) - (-30.0)).abs() < 1e-5);
        assert!((slider_to_db(0.25, curve, FLOOR) - (-45.0)).abs() < 1e-5);
        assert!((slider_to_db(0.75, curve, FLOOR) - (-15.0)).abs() < 1e-5);
    }

    #[test]
    fn slider_to_db_quadratic_matches_legacy_formula() {
        let curve = VolumeCurve::Quadratic;
        // db = 20 * log10(v²)
        // v=0.5 ⇒ 20*log10(0.25) ≈ -12.04 dB
        // v=0.1 ⇒ 20*log10(0.01) = -40 dB exact.
        assert!((slider_to_db(0.5, curve, FLOOR) - (-12.041_2)).abs() < 1e-3);
        assert!((slider_to_db(0.1, curve, FLOOR) - (-40.0)).abs() < 1e-4);
    }

    #[test]
    fn slider_to_db_is_strictly_monotonic_on_open_interval() {
        // Sample a fine grid on (0, 1) and verify monotonicity for
        // both curves at the design's default floor.
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            let mut prev = f32::NEG_INFINITY;
            for i in 1..=1000 {
                let v = i as f32 / 1000.0;
                let db = slider_to_db(v, curve, FLOOR);
                assert!(
                    db >= prev,
                    "curve {:?} not monotone at v={v}: {db} < prev {prev}",
                    curve
                );
                prev = db;
            }
        }
    }

    // ---- db_to_slider -------------------------------------------------------

    #[test]
    fn db_to_slider_round_trips_through_slider_to_db() {
        // Round-trip is only well-defined on the curve's audible
        // range. For `Logarithmic` that's `v ∈ (0, 1]` ⇔
        // `db ∈ (floor_db, 0]`; for `Quadratic` it's `v` such that
        // `slider_to_db(v) ≥ floor_db`, i.e. `v ≥ 10^(floor/40)`.
        // Below the floor both directions saturate to mute, which is
        // a one-way collapse, so we sample inside the audible range.
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            let v_min = match curve {
                VolumeCurve::Logarithmic => 0.001,
                VolumeCurve::Quadratic => 10f32.powf(FLOOR / 40.0) + 0.001,
            };
            let mut v = v_min;
            while v <= 0.999 {
                let db = slider_to_db(v, curve, FLOOR);
                let v2 = db_to_slider(db, curve, FLOOR);
                assert!(
                    (v - v2).abs() < 1e-4,
                    "round-trip failure {curve:?}: v={v} → db={db} → v={v2}"
                );
                v += 0.001;
            }
        }
    }

    #[test]
    fn db_to_slider_handles_extremes() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            assert_eq!(db_to_slider(f32::NEG_INFINITY, curve, FLOOR), 0.0);
            assert_eq!(db_to_slider(0.0, curve, FLOOR), 1.0);
            assert_eq!(db_to_slider(5.0, curve, FLOOR), 1.0); // saturated above unity
            assert_eq!(db_to_slider(FLOOR, curve, FLOOR), 0.0);
            assert_eq!(db_to_slider(FLOOR - 10.0, curve, FLOOR), 0.0);
        }
    }

    // ---- slider_to_linear ---------------------------------------------------

    #[test]
    fn slider_to_linear_anchors_match_pre_gain_stage() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            assert_eq!(
                slider_to_linear(0.0, curve, FLOOR),
                0.0,
                "v=0 must produce strict mute on {curve:?}"
            );
            assert_eq!(
                slider_to_linear(1.0, curve, FLOOR),
                1.0,
                "v=1 must produce strict unity on {curve:?}"
            );
        }
    }

    #[test]
    fn slider_to_linear_quadratic_matches_legacy_v_squared() {
        let curve = VolumeCurve::Quadratic;
        for i in 1..=99 {
            let v = i as f32 / 100.0;
            let want = v * v;
            let got = slider_to_linear(v, curve, FLOOR);
            assert!(
                (got - want).abs() < 1e-6,
                "v={v} legacy v²={want}, got {got}"
            );
        }
    }

    #[test]
    fn slider_to_linear_logarithmic_strictly_positive_above_zero() {
        let curve = VolumeCurve::Logarithmic;
        for i in 1..=1000 {
            let v = i as f32 / 1000.0;
            let g = slider_to_linear(v, curve, FLOOR);
            assert!(g > 0.0, "v={v} produced zero linear gain");
        }
    }

    #[test]
    fn slider_to_linear_is_monotone_strict_on_open_interval() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            let mut prev = 0.0_f32;
            for i in 1..=1000 {
                let v = i as f32 / 1000.0;
                let g = slider_to_linear(v, curve, FLOOR);
                assert!(g > prev, "curve {curve:?} not strictly monotone at v={v}");
                prev = g;
            }
        }
    }

    // ---- step_db ------------------------------------------------------------

    #[test]
    fn step_db_from_mute_stays_muted() {
        // Stepping out of mute by a positive delta does not unmute,
        // because the floor itself is treated as the slider's mute
        // boundary (`db_to_slider(floor_db, …) == 0`). The user has
        // to raise the slider above zero by direct interaction
        // before media-key stepping starts moving it. Verified for
        // both curves so the contract is uniform.
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            let v = step_db(0.0, 2.0, curve, FLOOR);
            assert_eq!(v, 0.0, "{curve:?}: positive step from mute must stay muted");
        }
    }

    #[test]
    fn step_db_caps_at_unity() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            let v = step_db(1.0, 6.0, curve, FLOOR);
            assert_eq!(v, 1.0);
        }
    }

    #[test]
    fn step_db_caps_at_floor_on_negative_step() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            // From v=0.5 step -1000 dB → must land at the floor
            // (slider=0, mute) since the clamp pins new_db to floor.
            let v = step_db(0.5, -1000.0, curve, FLOOR);
            assert_eq!(v, 0.0);
        }
    }

    #[test]
    fn step_db_actually_changes_db_by_delta() {
        // Pick a mid-slider that yields a finite, non-saturating dB
        // for both curves; verify that stepping by +2 dB indeed
        // shifts the audible dB by +2 dB (within rounding).
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            let v0 = 0.5_f32;
            let db0 = slider_to_db(v0, curve, FLOOR);
            let v1 = step_db(v0, 2.0, curve, FLOOR);
            let db1 = slider_to_db(v1, curve, FLOOR);
            let expected = (db0 + 2.0).clamp(FLOOR, 0.0);
            assert!(
                (db1 - expected).abs() < 1e-3,
                "{curve:?}: db0={db0}, db1={db1}, expected={expected}"
            );
        }
    }

    // ---- format_db_for_display ---------------------------------------------

    #[test]
    fn format_at_zero_is_minus_inf() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            assert_eq!(format_db_for_display(0.0, curve, FLOOR), "-inf dB");
        }
    }

    #[test]
    fn format_at_one_is_exact_zero_db() {
        for curve in [VolumeCurve::Logarithmic, VolumeCurve::Quadratic] {
            assert_eq!(format_db_for_display(1.0, curve, FLOOR), "0.0 dB");
        }
    }

    #[test]
    fn format_rounds_to_one_decimal() {
        // Pick a logarithmic slider that maps to a fractional dB
        // value with > 1 decimal of precision and verify the
        // rendered string only carries one decimal.
        let s = format_db_for_display(0.795, VolumeCurve::Logarithmic, FLOOR);
        // floor + (0 - floor) * v = -60 + 60 * 0.795 = -12.3 dB.
        assert_eq!(s, "-12.3 dB");
    }

    #[test]
    fn format_normalises_signed_zero() {
        // Pick a slider close enough to 1.0 that the dB rounds to
        // 0.0 but is actually slightly negative — the formatter must
        // not display "-0.0 dB".
        let s = format_db_for_display(0.9995, VolumeCurve::Logarithmic, FLOOR);
        assert_eq!(s, "0.0 dB");
    }

    #[test]
    fn format_uses_proper_db_suffix() {
        // Sanity-check the "<num> dB" suffix on a representative
        // non-boundary value.
        let s = format_db_for_display(0.5, VolumeCurve::Logarithmic, FLOOR);
        assert!(
            s.ends_with(" dB") && !s.contains("inf"),
            "unexpected format string: {s:?}"
        );
    }
}
