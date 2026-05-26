//! Property tests for the volume curve helpers (task 23).
//!
//! Two properties are validated here:
//!
//! * **Property 8** — for both `VolumeCurve::Logarithmic` and
//!   `VolumeCurve::Quadratic`, the curve helpers anchor `v = 0` to
//!   strict mute (`slider_to_linear == 0.0`, `slider_to_db == -inf`)
//!   and `v = 1` to strict unity (`slider_to_linear == 1.0`,
//!   `slider_to_db == 0.0`), and remain monotone non-decreasing on a
//!   fine grid spanning `[0, 1]`. The floor is fuzzed over
//!   `[-100, -20]` dB to make sure the contract holds well outside
//!   the design's default of `-60 dB`.
//!
//! * **Property 31** — `format_db_for_display` produces a string that
//!   parses back to `round1(slider_to_db(v))` for any random slider
//!   position (with the `-inf dB` / `0.0 dB` endpoint contracts at
//!   `v = 0` and `v = 1`). This is the property that lets the TS
//!   counterpart in `ui/src/lib/volumeFormat.ts` (task 29) stay in
//!   lock-step with the Rust formatter without round-tripping through
//!   Tauri on every player tick.

use proptest::prelude::*;
use qobee_engine::audio_settings::VolumeCurve;
use qobee_engine::volume::{format_db_for_display, slider_to_db, slider_to_linear};

/// Strategy that picks one of the two volume curves uniformly.
fn any_curve() -> impl Strategy<Value = VolumeCurve> {
    prop_oneof![
        Just(VolumeCurve::Logarithmic),
        Just(VolumeCurve::Quadratic),
    ]
}

/// Round to one decimal place using the same rule as
/// `format_db_for_display` (and the TS counterpart in
/// `ui/src/lib/volumeFormat.ts`): half-away-from-zero rounding via
/// `(x * 10).round() / 10`, with the signed-zero corner case
/// normalised so a value that rounds to `0.0` never carries a phantom
/// minus sign.
fn round1(db: f32) -> f32 {
    let r = (db * 10.0).round() / 10.0;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// Parse the formatter's output back to an f32 dB value.
///
/// Accepts `"-inf dB"` (mute) and `"<num> dB"` (one decimal). Panics
/// on any other shape — the formatter is contract-bound to one of
/// these two forms and a deviation is itself a test failure.
fn parse_db_string(s: &str) -> f32 {
    if s == "-inf dB" {
        return f32::NEG_INFINITY;
    }
    let body = s
        .strip_suffix(" dB")
        .expect("formatter string must end in ' dB'");
    body.parse::<f32>()
        .expect("dB body must be a finite f32 literal")
}

// -----------------------------------------------------------------------------
// Property 8 — monotonie + bornes exactes
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 8: Volume_Curve est
// monotone non-décroissante et a des bornes exactes (mute à v=0, unité
// à v=1) pour les deux courbes.
//
// **Validates: Requirements R4.1, R4.2, R4.3**
#[test]
fn property_8_volume_curve_monotone_with_exact_bounds() {
    proptest!(
        ProptestConfig::with_cases(100),
        |(curve in any_curve(),
          floor_db in -100.0f32..=-20.0f32,
          slider in 0.0f32..=1.0f32)| {

            // Bound at v = 0 — strict mute on both curves (R4.2).
            // The logarithmic curve must not leak a residual
            // `10^(floor_db/20)`, even when the floor is fuzzed
            // outside the design's nominal `[-80, -30]` range.
            prop_assert_eq!(
                slider_to_linear(0.0, curve, floor_db),
                0.0_f32,
                "v=0 must produce exact mute on {:?} (floor_db={})",
                curve,
                floor_db
            );
            prop_assert_eq!(
                slider_to_db(0.0, curve, floor_db),
                f32::NEG_INFINITY,
                "v=0 must produce -inf dB on {:?} (floor_db={})",
                curve,
                floor_db
            );

            // Bound at v = 1 — strict unity on both curves (R4.3).
            prop_assert_eq!(
                slider_to_linear(1.0, curve, floor_db),
                1.0_f32,
                "v=1 must produce exact unity on {:?} (floor_db={})",
                curve,
                floor_db
            );
            prop_assert_eq!(
                slider_to_db(1.0, curve, floor_db),
                0.0_f32,
                "v=1 must produce exact 0.0 dB on {:?} (floor_db={})",
                curve,
                floor_db
            );

            // The fuzzed `slider` is used to confirm the helper
            // returns a finite, in-range value for arbitrary
            // positions — even at floors well outside the design's
            // nominal range.
            let g = slider_to_linear(slider, curve, floor_db);
            prop_assert!(
                g.is_finite() && g >= 0.0 && g <= 1.0 + 1e-6,
                "g={g} out of [0, 1] for v={slider} on {:?}",
                curve
            );

            // Monotonicity on a 257-point grid spanning `[0, 1]`
            // inclusive. Both curves must be non-decreasing in
            // both linear and dB scales (R4.1). 256 steps gives
            // ~6e-3 spacing, enough resolution to surface any
            // ULP-scale regression without dragging the per-case
            // cost out of the CI budget.
            let n = 256_usize;
            let mut prev_lin = 0.0_f32;
            let mut prev_db = f32::NEG_INFINITY;
            for i in 0..=n {
                let v = i as f32 / n as f32;
                let lin = slider_to_linear(v, curve, floor_db);
                let db = slider_to_db(v, curve, floor_db);
                prop_assert!(
                    lin >= prev_lin - 1e-7,
                    "linear gain decreased at v={v}: {lin} < prev {prev_lin} ({:?}, floor_db={})",
                    curve,
                    floor_db
                );
                prop_assert!(
                    db >= prev_db,
                    "dB decreased at v={v}: {db} < prev {prev_db} ({:?}, floor_db={})",
                    curve,
                    floor_db
                );
                prev_lin = lin;
                prev_db = db;
            }
        }
    );
}

// -----------------------------------------------------------------------------
// Property 31 — format_db_for_display round-trips through parse
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 31: format_db_for_display
// produit une chaîne qui se parse en l'arrondi au dixième de
// slider_to_db (avec -inf à v=0 et 0.0 à v=1). Cette propriété
// garantit que le formateur Rust et son équivalent TS dans
// `ui/src/lib/volumeFormat.ts` (task 29) restent en lock-step.
//
// **Validates: Requirements R4.1, R4.2, R4.3**
#[test]
fn property_31_format_db_for_display_round_trips() {
    proptest!(
        ProptestConfig::with_cases(200),
        |(curve in any_curve(),
          floor_db in -100.0f32..=-20.0f32,
          slider in 0.0f32..=1.0f32)| {

            let formatted = format_db_for_display(slider, curve, floor_db);
            // Contract: the formatter only ever produces "-inf dB"
            // or "<num> dB". Anything else is itself a failure
            // (parse_db_string panics rather than returning an
            // error, which surfaces as a proptest failure).
            let parsed = parse_db_string(&formatted);

            // Endpoint v = 0 — must format to the literal "-inf dB"
            // string (R4.5 / R4.2).
            if slider == 0.0 {
                prop_assert_eq!(
                    formatted.as_str(),
                    "-inf dB",
                    "v=0 must format to '-inf dB' on {:?}",
                    curve
                );
                prop_assert!(
                    parsed == f32::NEG_INFINITY,
                    "parsed -inf dB to {parsed} ({:?})",
                    curve
                );
                return Ok(());
            }

            // Endpoint v = 1 — must format to "0.0 dB" exactly
            // (R4.3, also makes sure the bit-perfect badge stays
            // green at full volume).
            if (slider - 1.0).abs() < 1e-9 {
                prop_assert_eq!(
                    formatted.as_str(),
                    "0.0 dB",
                    "v=1 must format to '0.0 dB' on {:?}",
                    curve
                );
                prop_assert_eq!(
                    parsed,
                    0.0_f32,
                    "parsed '0.0 dB' must round-trip to 0.0 on {:?}",
                    curve
                );
                return Ok(());
            }

            // General case — round1(slider_to_db(v)) must match
            // what the formatter emits. The TS counterpart in
            // `ui/src/lib/volumeFormat.ts` (task 29) reproduces
            // this same rule, so this property pins both sides to
            // a single specification.
            let raw_db = slider_to_db(slider, curve, floor_db);
            // `slider > 0` guarantees raw_db is finite for both
            // curves (Quadratic: 20*log10(v²) is finite for v > 0;
            // Logarithmic: floor + (0 - floor) * v is finite).
            prop_assert!(
                raw_db.is_finite(),
                "expected finite dB for v={slider} on {:?}, got {raw_db}",
                curve
            );
            let expected = round1(raw_db);
            // Allow a half-LSB of f32 round-trip slack on the
            // parsed value to absorb the printf rounding when the
            // raw dB lies exactly on a tie boundary (e.g. -12.35
            // formatted as "-12.4 dB" or "-12.3 dB" depending on
            // round-to-nearest-even). The 1e-4 tolerance is far
            // below the 0.1 dB step we are comparing against.
            prop_assert!(
                (parsed - expected).abs() < 1e-4,
                "round-trip mismatch on {:?} (floor_db={}) for v={slider}: \
                 raw_db={raw_db}, formatted='{formatted}', parsed={parsed}, expected={expected}",
                curve,
                floor_db
            );
        }
    );
}
