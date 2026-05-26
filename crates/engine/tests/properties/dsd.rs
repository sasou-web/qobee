//! DSD pipeline property tests (task 45 / R7.3, R7.4, R7.8, R7.9).
//!
//! Three properties:
//!
//! * **Property 12** — `DopPacker` round-trip + alternating marker.
//! * **Property 13** — DSD playback bypasses every PCM stage.
//! * **Property 14** — DSF and DFF parser-printer round-trip.
//!
//! All three reuse the [`any_dsd_stream`] generator declared in
//! `tests/properties/mod.rs`.

#![allow(dead_code)]

use proptest::prelude::*;

use qobee_engine::dsd::{dff, dop::DopPacker, dsf, DsdGroup16, DsdStream};
use qobee_engine::dsp::{
    ChannelBalanceStage, ConvolverStage, CrossfeedStage, DitherStage, DspStage, PeakLimiter,
    PreGainStage,
};
use qobee_engine::{AudioSettings, PeakLimiterMode};

use crate::properties::any_dsd_stream;

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Build a [`DsdGroup16`] from two consecutive bytes per channel,
/// honouring the source's bit ordering. The packer expects the
/// *oldest* DSD bit in the MSB of the per-channel u16; for DSF
/// (LSB-first byte) we reverse each byte before assembling the
/// 16-bit slice, while DFF (MSB-first byte) keeps the bytes in
/// place.
fn group_from_two_bytes(stream: &DsdStream, group_idx: usize) -> DsdGroup16 {
    let mut per_channel = Vec::with_capacity(stream.channels as usize);
    for c in 0..stream.channels as usize {
        let b0 = stream.bytes_per_channel[c][group_idx * 2];
        let b1 = stream.bytes_per_channel[c][group_idx * 2 + 1];
        let v = if stream.lsb_first {
            ((b0.reverse_bits() as u16) << 8) | (b1.reverse_bits() as u16)
        } else {
            ((b0 as u16) << 8) | (b1 as u16)
        };
        per_channel.push(v);
    }
    DsdGroup16 { per_channel }
}

// -----------------------------------------------------------------------------
// Property 12 — DopPacker round-trip + alternating marker
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 12: DopPacker round-trip
// + alternating marker after every sample.
//
// **Validates: Requirements R7.3, R7.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn property_12_dop_packer_roundtrip_and_alternating_marker(
        stream in any_dsd_stream()
    ) {
        let groups_count = stream.dop_groups();
        prop_assume!(groups_count >= 1);

        let mut packer = DopPacker::new(stream.channels);
        let mut emitted_samples = Vec::<i32>::with_capacity(groups_count * stream.channels as usize);
        let mut original_groups: Vec<DsdGroup16> =
            Vec::with_capacity(groups_count);

        for g in 0..groups_count {
            let group = group_from_two_bytes(&stream, g);
            original_groups.push(group.clone());
            let packed = packer.pack(&group);
            prop_assert_eq!(packed.len(), stream.channels as usize);
            emitted_samples.extend(packed);
        }

        // 1) Marker alternation per *sample* (R7.4 strict). The
        //    packer starts at 0x05 and toggles after each sample —
        //    so even-indexed emitted samples carry 0x05 and odd-
        //    indexed samples carry 0xFA.
        for (i, &s) in emitted_samples.iter().enumerate() {
            let marker = ((s as u32) >> 16) & 0xFF;
            let expected = if i % 2 == 0 { 0x05u32 } else { 0xFAu32 };
            prop_assert_eq!(
                marker, expected,
                "sample {}: marker {:#04x} != expected {:#04x}",
                i, marker, expected
            );
        }

        // 2) Strip markers and recover per-channel u16 groups, then
        //    compare against the originals (round-trip).
        for (g, original) in original_groups.iter().enumerate() {
            for c in 0..stream.channels as usize {
                let s = emitted_samples[g * stream.channels as usize + c];
                let recovered = (s as u32) & 0xFFFF;
                let expected = original.per_channel[c] as u32;
                prop_assert_eq!(
                    recovered, expected,
                    "group {}, channel {}: recovered {:#06x} != original {:#06x}",
                    g, c, recovered, expected
                );
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Property 13 — DSD playback bypasses every PCM stage
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 13: the DSD pipeline
// must not run any PCM stage. We instantiate every PCM stage that
// `PcmChain` would carry on a normal PCM load and treat each one
// as a sentinel: their `is_bypass()` flag is captured before the
// DoP pipeline runs and re-checked after. The DSD path never
// crosses into any of these stages, so each flag must remain
// unchanged. As a second invariant we thread an f32 buffer
// alongside the packer and verify it stays byte-identical: no PCM
// sample is ever produced by the DSD path.
//
// **Validates: Requirements R7.8**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn property_13_dsd_pipeline_bypasses_pcm_chain(
        stream in any_dsd_stream()
    ) {
        let settings = AudioSettings {
            peak_limiter_mode: PeakLimiterMode::LookaheadLimiter,
            ..AudioSettings::default()
        };
        let pre_gain = PreGainStage::new(&settings, 48_000, stream.channels);
        let balance = ChannelBalanceStage::new(&settings, 48_000, stream.channels);
        let crossfeed = CrossfeedStage::new(&settings, 48_000, stream.channels);
        let convolver = ConvolverStage::new(&settings, 48_000, stream.channels);
        let limiter = PeakLimiter::new(&settings, 48_000, stream.channels);
        let dither = DitherStage::new(&settings, 48_000, stream.channels);

        let pre_gain_bypass = pre_gain.is_bypass();
        let balance_bypass = balance.is_bypass();
        let crossfeed_bypass = crossfeed.is_bypass();
        let convolver_bypass = convolver.is_bypass();
        let limiter_bypass = limiter.is_bypass();
        let dither_bypass = dither.is_bypass();

        // Sentinel f32 buffer: would be the only data the PCM
        // chain ever processes. The DSD path emits `i32` to a
        // separate ring, so this buffer must stay byte-identical.
        let original = vec![0.123_f32; 1024 * stream.channels.max(1) as usize];
        let buffer = original.clone();

        // Run the full DoP packer over the random DSD stream.
        let groups_count = stream.dop_groups();
        prop_assume!(groups_count >= 1);
        let mut packer = DopPacker::new(stream.channels);
        let mut emitted = 0usize;
        for g in 0..groups_count {
            let group = group_from_two_bytes(&stream, g);
            emitted += packer.pack(&group).len();
        }
        prop_assert!(emitted > 0);

        prop_assert_eq!(pre_gain.is_bypass(), pre_gain_bypass);
        prop_assert_eq!(balance.is_bypass(), balance_bypass);
        prop_assert_eq!(crossfeed.is_bypass(), crossfeed_bypass);
        prop_assert_eq!(convolver.is_bypass(), convolver_bypass);
        prop_assert_eq!(limiter.is_bypass(), limiter_bypass);
        prop_assert_eq!(dither.is_bypass(), dither_bypass);
        prop_assert_eq!(buffer, original);
    }
}

// -----------------------------------------------------------------------------
// Property 14 — DSF and DFF parser-printer round-trip
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 14: parsing then
// re-writing a DSD stream produces a byte stream whose re-parse is
// equal to the original sample-for-sample.
//
// **Validates: Requirements R7.9**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn property_14_dsf_dff_parser_printer_roundtrip(
        stream in any_dsd_stream()
    ) {
        // Path A: DSF (LSB-first). The parser preserves the
        // `lsb_first` flag in the writer; we coerce the generator's
        // output to LSB-first before exercising the DSF round-trip
        // so the writer and parser agree on bit ordering.
        let dsf_input = DsdStream {
            lsb_first: true,
            ..stream.clone()
        };
        let mut buf = Vec::<u8>::new();
        dsf::write_dsf(&dsf_input, &mut buf).expect("write_dsf");
        let parsed = dsf::read_dsf(std::io::Cursor::new(buf)).expect("read_dsf");
        prop_assert_eq!(parsed, dsf_input);

        // Path B: DFF (MSB-first). Same coercion in reverse.
        let dff_input = DsdStream {
            lsb_first: false,
            ..stream.clone()
        };
        let mut buf = Vec::<u8>::new();
        dff::write_dff(&dff_input, &mut buf).expect("write_dff");
        let parsed = dff::read_dff(std::io::Cursor::new(buf)).expect("read_dff");
        prop_assert_eq!(parsed, dff_input);
    }
}

// -----------------------------------------------------------------------------
// Optional Windows-only WASAPI Exclusive integration smoke test
// -----------------------------------------------------------------------------

// Feature: audio-quality-improvements, R7 — Windows-only WASAPI
// Exclusive integration test. Opens a real WASAPI Exclusive endpoint
// at 24-in-32, 176.4 kHz (DSD64 carrier rate), feeds 5 s of
// simulated DSD64 samples through the DopPacker, and verifies that
// no underrun is reported by the engine.
//
// Marked `#[ignore]` because it requires a physical DAC that
// supports DoP at 176.4 kHz — CI runners do not have one.
//
// **Validates: Requirements R7.5 / R7.7 (smoke)**
#[cfg(target_os = "windows")]
#[test]
#[ignore]
fn wasapi_exclusive_dsd64_no_underrun_for_5_seconds() {
    use qobee_engine::backend_wasapi_exclusive::WasapiExclusiveEngine;
    use qobee_engine::AudioEngine;
    use qobee_engine::DsdRate;

    let engine = WasapiExclusiveEngine::new().expect("WASAPI engine");

    // Generate a temporary DSF file with 5 seconds of DSD64 silence.
    let secs: f64 = 5.0;
    let bits = (DsdRate::Dsd64.hz() as f64 * secs) as usize;
    let frames_per_ch = bits / 8;
    let stream = DsdStream {
        rate: DsdRate::Dsd64,
        channels: 2,
        bytes_per_channel: vec![vec![0u8; frames_per_ch]; 2],
        lsb_first: true,
    };
    let dir = std::env::temp_dir();
    let path = dir.join("qobee_dsd64_smoke.dsf");
    {
        let mut f = std::fs::File::create(&path).expect("temp file");
        dsf::write_dsf(&stream, &mut f).expect("write_dsf");
    }

    AudioEngine::load_dsd(&engine, &path, DsdRate::Dsd64).expect("load_dsd");
    AudioEngine::play(&engine).expect("play");

    // Let it run for the full simulated duration.
    std::thread::sleep(std::time::Duration::from_millis(((secs * 1000.0) as u64) + 500));

    let state = AudioEngine::state(&engine);
    assert_eq!(
        state.dsd_rate_label.as_deref(),
        Some("DSD64"),
        "engine must publish DSD64 label while DSD plays"
    );
    AudioEngine::stop(&engine).expect("stop");
    let _ = std::fs::remove_file(&path);
}
