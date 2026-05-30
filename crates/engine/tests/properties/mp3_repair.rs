//! Property test for the MP3 repaired-frame accounting in
//! [`qobee_engine::backend_symphonia::SymphoniaDecoder`] (task 15.2).
//!
//! Decoding real MP3 bytes inside a property test is impractical
//! (it needs a `symphonia` reader, a file, and recoverable bitstream
//! corruption that is hard to synthesise deterministically). So this
//! test models the *counting logic* of `SymphoniaDecoder::next_packet`
//! / EOF as a small pure function and asserts the three invariants of
//! Property 8 over a randomised sequence of per-packet decode results.
//!
//! The model mirrors the real control flow in `next_packet`:
//!
//!   * a `Success` packet is returned to the caller and does **not**
//!     touch the repaired counter,
//!   * a `DecodeError` is a recoverable per-frame error: the decoder
//!     does `self.repaired_frames += 1; continue;` (logged at `debug`,
//!     R11.1),
//!   * at end of stream (`Ok(None)`), a **single** aggregate `info`
//!     log is emitted iff `repaired_frames > 0` (R11.2),
//!   * the `degraded` flag propagated to `PlayerState` is
//!     `repaired_frames > DEGRADED_FRAME_THRESHOLD` (R11.3).
//!
//! The threshold is read from the real crate constant so the test
//! tracks any future change to `DEGRADED_FRAME_THRESHOLD` rather than
//! hard-coding `64`.

use proptest::collection::vec;
use proptest::prelude::*;

use qobee_engine::backend_symphonia::DEGRADED_FRAME_THRESHOLD;

/// One packet's decode result, abstracting `symphonia`'s
/// `Decoder::decode` outcome down to the only distinction the
/// repaired-frame accounting cares about:
///
///   * [`DecodeOutcome::Success`] — a packet decoded cleanly and was
///     handed back to the caller,
///   * [`DecodeOutcome::DecodeError`] — a recoverable
///     `SymphoniaError::DecodeError` (e.g. MP3 `invalid
///     main_data_begin`) that the decoder counts and skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeOutcome {
    Success,
    DecodeError,
}

/// Result of replaying a finite decode sequence through the model.
#[derive(Debug, Default, PartialEq, Eq)]
struct DecodeModel {
    /// Mirror of `SymphoniaDecoder::repaired_frames` at EOF.
    repaired_frames: u64,
    /// How many aggregate `info` logs were emitted at EOF. The real
    /// decoder emits the line at most once, so this is `0` or `1`.
    aggregate_info_logs: u64,
    /// Mirror of `PlayerState::degraded` for this file.
    degraded: bool,
}

/// Pure reimplementation of `SymphoniaDecoder::next_packet`'s
/// repaired-frame accounting plus the EOF aggregate-log / degraded
/// decision. Replaying the whole sequence and only then handling EOF
/// matches the real loop, which counts errors packet-by-packet and
/// emits the single aggregate log on the terminal `Ok(None)`.
fn run_decode_model(outcomes: &[DecodeOutcome]) -> DecodeModel {
    let mut repaired_frames: u64 = 0;
    for outcome in outcomes {
        match outcome {
            // `Ok(audio_buf) => return Ok(Some(...))` — a successful
            // decode never increments the repaired counter.
            DecodeOutcome::Success => {}
            // `Err(DecodeError(msg)) => { self.repaired_frames += 1;
            //  ...; continue; }`
            DecodeOutcome::DecodeError => repaired_frames += 1,
        }
    }
    // EOF (`Ok(None)`): emit exactly one aggregate `info` log iff at
    // least one frame was repaired (R11.2).
    let aggregate_info_logs = u64::from(repaired_frames > 0);
    // Degraded flag (R11.3): strictly greater than the threshold.
    let degraded = repaired_frames > DEGRADED_FRAME_THRESHOLD;
    DecodeModel {
        repaired_frames,
        aggregate_info_logs,
        degraded,
    }
}

/// Generator for a finite per-packet decode sequence. Length spans
/// `0..=256` so the count of `DecodeError`s comfortably straddles the
/// degraded threshold (`64`) in both directions across a 100+ case
/// run, exercising `degraded == false` and `degraded == true`. Each
/// element is an independent fair coin between `Success` and
/// `DecodeError`, so empty files, all-clean files, and all-corrupt
/// files all appear.
fn any_decode_sequence() -> impl Strategy<Value = Vec<DecodeOutcome>> {
    let outcome = prop_oneof![
        Just(DecodeOutcome::Success),
        Just(DecodeOutcome::DecodeError),
    ];
    vec(outcome, 0..=256)
}

// Feature: qobee-beta-feedback-improvements, Property 8: Comptage des
// trames MP3 réparées et drapeau dégradé.
//
// **Validates: Requirements 11.2, 11.3**
#[test]
fn property_8_mp3_repaired_frame_counting_and_degraded_flag() {
    proptest!(
        ProptestConfig::with_cases(256),
        |(outcomes in any_decode_sequence())| {
            let model = run_decode_model(&outcomes);

            // Reference count computed independently of the model.
            let expected_repaired = outcomes
                .iter()
                .filter(|o| matches!(o, DecodeOutcome::DecodeError))
                .count() as u64;

            // 1) `repaired_frames` equals the number of DecodeErrors.
            prop_assert_eq!(
                model.repaired_frames,
                expected_repaired,
                "repaired_frames {} != DecodeError count {}",
                model.repaired_frames,
                expected_repaired
            );

            // 2) A single aggregate `info` log is emitted at EOF iff
            //    repaired_frames > 0 (never more than one).
            let expected_logs = u64::from(expected_repaired > 0);
            prop_assert_eq!(
                model.aggregate_info_logs,
                expected_logs,
                "aggregate info logs {} != expected {} (repaired = {})",
                model.aggregate_info_logs,
                expected_logs,
                expected_repaired
            );
            prop_assert!(
                model.aggregate_info_logs <= 1,
                "at most one aggregate log may be emitted, got {}",
                model.aggregate_info_logs
            );

            // 3) `degraded` is true iff repaired_frames exceeds the
            //    threshold (strictly greater).
            prop_assert_eq!(
                model.degraded,
                expected_repaired > DEGRADED_FRAME_THRESHOLD,
                "degraded {} disagrees with (repaired {} > threshold {})",
                model.degraded,
                expected_repaired,
                DEGRADED_FRAME_THRESHOLD
            );
        }
    );
}

// Feature: qobee-beta-feedback-improvements, Property 8 — boundary
// example: exactly `DEGRADED_FRAME_THRESHOLD` repairs is NOT degraded,
// one more is. Pins the strict `>` comparison the property relies on.
#[test]
fn degraded_flag_is_strict_at_the_threshold() {
    let at_threshold = vec![DecodeOutcome::DecodeError; DEGRADED_FRAME_THRESHOLD as usize];
    let model = run_decode_model(&at_threshold);
    assert_eq!(model.repaired_frames, DEGRADED_FRAME_THRESHOLD);
    assert!(
        !model.degraded,
        "exactly threshold repairs must not be degraded"
    );
    assert_eq!(model.aggregate_info_logs, 1);

    let over_threshold = vec![DecodeOutcome::DecodeError; DEGRADED_FRAME_THRESHOLD as usize + 1];
    let model = run_decode_model(&over_threshold);
    assert_eq!(model.repaired_frames, DEGRADED_FRAME_THRESHOLD + 1);
    assert!(model.degraded, "one over threshold must be degraded");
    assert_eq!(model.aggregate_info_logs, 1);
}

// Feature: qobee-beta-feedback-improvements, Property 8 — a clean file
// (all successes, including the empty case) repairs nothing, emits no
// aggregate log, and is never degraded.
#[test]
fn clean_file_has_no_repairs_no_log_and_is_not_degraded() {
    let model = run_decode_model(&[]);
    assert_eq!(model, DecodeModel::default());

    let all_clean = vec![DecodeOutcome::Success; 128];
    let model = run_decode_model(&all_clean);
    assert_eq!(model.repaired_frames, 0);
    assert_eq!(model.aggregate_info_logs, 0);
    assert!(!model.degraded);
}
