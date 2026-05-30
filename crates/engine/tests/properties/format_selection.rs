//! Property test for the pure WASAPI output-format selection logic
//! (`qobee_engine::select_format`).
//!
//! Implements **Property 6** from the `qobee-beta-feedback-improvements`
//! design: native-stereo (more generally, exact channel-match) is
//! preferred when offered by the device, and the `upmix` flag is set
//! exactly when the chosen layout is wider than the source.
//!
//! The selector under test is platform-independent (no WASAPI/COM
//! types), so this property runs on every host, not just Windows.

use proptest::collection::vec;
use proptest::prelude::*;
use qobee_engine::{select_format, CandidateFormat, UpmixInfo};

/// Common audio sample rates the device might report. The exact
/// value only matters for tie-breaking inside a channel rank, which
/// this property does not assert; we keep a realistic spread so the
/// generator exercises both equal-rate and mixed-rate candidate sets.
fn any_sample_rate() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(44_100u32),
        Just(48_000u32),
        Just(88_200u32),
        Just(96_000u32),
        Just(176_400u32),
        Just(192_000u32),
    ]
}

/// Realistic device channel layouts: mono, stereo, quad, 5.1, 7.1.
/// Restricting to this set (rather than `1..=8`) keeps native-match
/// collisions with `src_channels` frequent, so the "native preferred"
/// branch is well exercised across a 256-case run.
fn any_channels() -> impl Strategy<Value = u16> {
    prop_oneof![Just(1u16), Just(2u16), Just(4u16), Just(6u16), Just(8u16)]
}

/// A single device-reported candidate format.
fn any_candidate() -> impl Strategy<Value = CandidateFormat> {
    (any_sample_rate(), any_channels()).prop_map(|(sr, ch)| CandidateFormat::new(sr, ch))
}

/// A non-empty list of candidates (the device always offers at least
/// one accepted layout when negotiation reaches the selector).
fn any_candidates() -> impl Strategy<Value = Vec<CandidateFormat>> {
    vec(any_candidate(), 1..=12)
}

// Feature: qobee-beta-feedback-improvements, Property 6: Sélection de format
//
// Stéréo native préférée (plus généralement, correspondance exacte du
// nombre de canaux quand le périphérique la propose) et drapeau upmix
// exact : `upmix.is_some()` ssi `chosen.channels > src_channels`, avec
// `src`/`dst` corrects.
//
// **Validates: Requirements 9.4, 9.5**
#[test]
fn property_6_format_selection_native_preferred_and_upmix_flag_exact() {
    proptest!(
        ProptestConfig::with_cases(256),
        |(candidates in any_candidates(), src_channels in 1u16..=8u16)| {
            // A non-empty candidate list must always yield a choice.
            let sel = select_format(&candidates, src_channels)
                .expect("non-empty candidates must yield Some(FormatSelection)");
            let chosen = sel.chosen;

            // The chosen layout is always one the device actually
            // offered — the selector never fabricates a format.
            prop_assert!(
                candidates.contains(&chosen),
                "chosen {:?} is not among the candidates {:?}",
                chosen, candidates
            );

            // R9.4 — native preferred: if any candidate matches the
            // source channel count exactly, the chosen layout must
            // match it (no upmix to surround, no downmix), and the
            // upmix flag must be cleared.
            let has_native = candidates.iter().any(|c| c.channels == src_channels);
            if has_native {
                prop_assert_eq!(
                    chosen.channels, src_channels,
                    "a native ({}-ch) layout was offered but not chosen (chosen {:?})",
                    src_channels, chosen
                );
                prop_assert!(
                    sel.upmix.is_none(),
                    "native choice must not flag upmix, got {:?}", sel.upmix
                );
            }

            // R9.5 — upmix flag is set exactly when the chosen layout
            // is wider than the source, and then carries the exact
            // src/dst channel counts; otherwise it is None.
            if chosen.channels > src_channels {
                prop_assert_eq!(
                    sel.upmix,
                    Some(UpmixInfo {
                        src_channels,
                        dst_channels: chosen.channels,
                    }),
                    "wider chosen layout must flag an exact upmix"
                );
            } else {
                prop_assert!(
                    sel.upmix.is_none(),
                    "no upmix expected when chosen ({} ch) <= src ({} ch), got {:?}",
                    chosen.channels, src_channels, sel.upmix
                );
            }
        }
    );
}

// Feature: qobee-beta-feedback-improvements, Property 6 (edge): the
// selector returns `None` only when the candidate list is empty.
//
// **Validates: Requirements 9.4, 9.5**
#[test]
fn property_6_empty_candidates_is_the_only_none() {
    proptest!(
        ProptestConfig::with_cases(64),
        |(src_channels in 1u16..=8u16)| {
            prop_assert!(
                select_format(&[], src_channels).is_none(),
                "empty candidates must yield None"
            );
        }
    );
}
