//! Property test for the selected-output-device round-trip.
//!
//! Feature: audio-engine-simplification, Property 3 — setting a device
//! id and reading it back returns exactly the value that was set.
//!
//! `RodioEngine` opens its output stream lazily (on the first load), so
//! constructing one and round-tripping the selected device id does not
//! require any audio hardware.

use proptest::prelude::*;
use qobee_engine::{new_shared, AudioEngine};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Property 3: `set_output_device(x)` then `selected_output_device()`
    /// returns `x`, for an arbitrary `Option<String>`.
    #[test]
    fn selected_output_device_round_trips(
        id in proptest::option::of("[a-zA-Z0-9 _.:-]{0,40}"),
    ) {
        let engine = new_shared().expect("engine builds");
        engine.set_output_device(id.clone()).expect("set device");
        prop_assert_eq!(engine.selected_output_device(), id);
    }
}

#[test]
fn default_selection_is_none() {
    let engine = new_shared().expect("engine builds");
    assert_eq!(engine.selected_output_device(), None);
}
