//! Property tests for the volume mapping.
//!
//! Feature: audio-engine-simplification, Property 1 — the volume
//! mapping is anchored at the endpoints and monotone non-decreasing.

use proptest::prelude::*;
use qobee_engine::map_volume;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Property 1: for any slider in `[0.0, 1.0]`, the mapping yields a
    /// linear gain in `[0.0, 1.0]`, monotone non-decreasing in the
    /// slider, with `map(0.0) == 0.0` and `map(1.0) == 1.0`.
    #[test]
    fn volume_mapping_is_anchored_and_monotonic(
        a in 0.0f32..=1.0,
        b in 0.0f32..=1.0,
    ) {
        // Endpoints.
        prop_assert_eq!(map_volume(0.0), 0.0);
        prop_assert_eq!(map_volume(1.0), 1.0);

        // In-range.
        let ga = map_volume(a);
        let gb = map_volume(b);
        prop_assert!((0.0..=1.0).contains(&ga), "gain {ga} out of range for {a}");
        prop_assert!((0.0..=1.0).contains(&gb), "gain {gb} out of range for {b}");

        // Monotone non-decreasing.
        if a <= b {
            prop_assert!(ga <= gb, "map({a})={ga} > map({b})={gb}");
        } else {
            prop_assert!(gb <= ga, "map({b})={gb} > map({a})={ga}");
        }
    }

    /// Out-of-range and NaN inputs are clamped/sanitised into `[0, 1]`.
    #[test]
    fn volume_mapping_clamps_out_of_range(v in -1000.0f32..1000.0) {
        let g = map_volume(v);
        prop_assert!((0.0..=1.0).contains(&g));
    }
}

#[test]
fn nan_volume_maps_to_mute() {
    assert_eq!(map_volume(f32::NAN), 0.0);
}
