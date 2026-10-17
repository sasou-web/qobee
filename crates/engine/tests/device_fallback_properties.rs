//! Property tests for output-device fallback resolution.
//!
//! Feature: audio-engine-simplification, Property 4 — resolving a
//! requested device id never errors solely because the id is absent;
//! an absent id falls back to the system default.
//!
//! `resolve_output_device` talks to the live CPAL host. On a headless
//! machine there may be no default output device at all, in which case
//! resolution legitimately returns `Err(BackendUnavailable)` for every
//! input. We treat that uniformly-no-device case as a skip so the
//! property is exercised wherever audio hardware exists and is inert
//! (rather than falsely failing) where it does not.

use proptest::prelude::*;
use qobee_engine::{list_output_devices, resolve_output_device};

fn has_audio_output() -> bool {
    resolve_output_device(None).is_ok()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Property 4: for any requested id, resolution returns the
    /// requested device when present, otherwise the default with a
    /// fallback flag — and never errors merely because the id is absent.
    #[test]
    fn fallback_resolution_never_errors_on_absent_id(
        // Arbitrary ids unlikely to ever match a real device name, plus
        // occasionally a real device id pulled from the live list.
        suffix in "[a-zA-Z0-9 _-]{0,24}",
        use_real in any::<bool>(),
    ) {
        prop_assume!(has_audio_output());

        let real_ids: Vec<String> = list_output_devices()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.id)
            .collect();

        let requested: String = if use_real && !real_ids.is_empty() {
            real_ids[0].clone()
        } else {
            format!("qobee-nonexistent-device-{suffix}")
        };

        let (_, fell_back) = resolve_output_device(Some(&requested))
            .expect("resolution must not error on an absent id");

        let present = real_ids.iter().any(|id| id == &requested);
        if present {
            prop_assert!(!fell_back, "present id should not flag a fallback");
        } else {
            prop_assert!(fell_back, "absent id must flag a fallback to default");
        }
    }
}

#[test]
fn none_request_selects_default_without_fallback() {
    if !has_audio_output() {
        eprintln!("no audio output device; skipping");
        return;
    }
    let (_, fell_back) = resolve_output_device(None).expect("default resolves");
    assert!(!fell_back, "None request must not flag a fallback");
}
