//! Property tests for [`qobee_engine::BitPerfectHealth`] (task 27).
//!
//! Three properties pin the tri-state truth table contract:
//!
//!   * **Property 9** — corrélation flags ↔ état: the resulting
//!     `status` matches the truth table specified by R5.2.
//!   * **Property 10** — mismatch SR signalé: in Shared mode with a
//!     non-matching device rate, the French
//!     `Double rééchantillonnage caché` message contains both rates.
//!   * **Property 30** — badge bit-perfect ↔ tous étages unité +
//!     Exclusive: ≥ 200 cases verify that `status == Green` is the
//!     exact "all-unity Exclusive" boolean.
//!
//! All three share [`any_bit_perfect_inputs`], which is the
//! sole generator the spec authorises here.

use proptest::prelude::*;

use qobee_engine::{
    BitPerfectHealth, BitPerfectStatus, EffectiveOutputMode, PeakLimiterMode,
};

use crate::properties::any_bit_perfect_inputs;

/// Re-derive the "all PCM stages clean" flag the same way
/// `BitPerfectHealth::compute` does, so the property tests stay
/// pinned to the same definition without importing private helpers.
fn all_stages_clean(h: &BitPerfectHealth) -> bool {
    h.unity_volume
        && h.unity_pregain
        && h.eq_bypass
        && h.crossfeed_off
        && h.convolver_off
        && h.limiter_off
        && h.dither_bypass
        && h.balance_off
}

// Feature: audio-quality-improvements, Property 9: corrélation flags ↔ état.
//
// **Validates: Requirements R5.2, R5.3**
#[test]
fn property_9_status_matches_tri_state_truth_table() {
    proptest!(
        ProptestConfig::with_cases(200),
        |(inputs in any_bit_perfect_inputs())| {
            let (state, settings, device_sr, src_ch, device_ch,
                 eq_bypass, convolver_off, dither_bypass) = inputs;

            let h = BitPerfectHealth::compute(
                &state, &settings, device_sr, src_ch, device_ch,
                eq_bypass, convolver_off, dither_bypass,
            );

            let clean = all_stages_clean(&h);
            let want = match state.output_mode {
                EffectiveOutputMode::Exclusive
                    if clean && h.is_native_rate && !h.upmix_active =>
                {
                    BitPerfectStatus::Green
                }
                EffectiveOutputMode::Shared if clean && h.is_native_rate => {
                    BitPerfectStatus::Amber
                }
                _ => BitPerfectStatus::Red,
            };
            prop_assert!(
                h.status == want,
                "tri-state truth table mismatch: mode={:?}, clean={}, native={}, upmix={} ⇒ expected {:?}, got {:?}",
                state.output_mode, clean, h.is_native_rate, h.upmix_active, want, h.status
            );
        }
    );
}

// Feature: audio-quality-improvements, Property 10: mismatch SR signalé.
//
// **Validates: Requirements R5.3**
#[test]
fn property_10_shared_sr_mismatch_emits_french_message() {
    proptest!(
        ProptestConfig::with_cases(200),
        |(inputs in any_bit_perfect_inputs())| {
            let (state, settings, device_sr, src_ch, device_ch,
                 eq_bypass, convolver_off, dither_bypass) = inputs;

            // Only meaningful in Shared with both rates known and
            // distinct. Other configurations are covered by Property 9.
            let dev = match device_sr { Some(d) => d, None => return Ok(()) };
            let src = match state.sample_rate { Some(s) => s, None => return Ok(()) };
            if !matches!(state.output_mode, EffectiveOutputMode::Shared) { return Ok(()); }
            if dev == src { return Ok(()); }

            let h = BitPerfectHealth::compute(
                &state, &settings, device_sr, src_ch, device_ch,
                eq_bypass, convolver_off, dither_bypass,
            );

            let dev_s = dev.to_string();
            let src_s = src.to_string();
            let hit = h.messages.iter().any(|m| {
                m.contains("Double rééchantillonnage caché")
                    && m.contains(&dev_s)
                    && m.contains(&src_s)
            });
            prop_assert!(
                hit,
                "expected French 'Double rééchantillonnage caché' message mentioning both {} and {}, got {:?}",
                dev_s, src_s, h.messages
            );
        }
    );
}

// Feature: audio-quality-improvements, Property 30: badge bit-perfect
// ↔ tous étages unité + Exclusive.
//
// **Validates: Requirements R5.2, R5.7, R3.9, R6.7, R9.9, R10.5**
#[test]
fn property_30_green_iff_exclusive_all_unity_native_no_upmix() {
    proptest!(
        ProptestConfig::with_cases(200),
        |(inputs in any_bit_perfect_inputs())| {
            let (state, settings, device_sr, src_ch, device_ch,
                 eq_bypass, convolver_off, dither_bypass) = inputs;

            let h = BitPerfectHealth::compute(
                &state, &settings, device_sr, src_ch, device_ch,
                eq_bypass, convolver_off, dither_bypass,
            );

            // Predicate evaluated *before* the function runs, from
            // the inputs themselves, so the property is independent
            // of the implementation we're validating.
            let exclusive = matches!(state.output_mode, EffectiveOutputMode::Exclusive);
            let unity_volume = (state.volume - 1.0).abs() < 1e-4;
            let unity_pregain = unity_volume
                && state.rg_attenuation_db.map(|d| d.abs() < 1e-4).unwrap_or(true);
            let crossfeed_off = !settings.crossfeed_enabled || src_ch != 2;
            let limiter_off = matches!(settings.peak_limiter_mode, PeakLimiterMode::Off);
            let balance_off = settings.balance.abs() < 1e-9
                && settings.trim_db_per_channel.iter().all(|t| t.abs() < 1e-6);
            let upmix = src_ch != 0 && device_ch != 0 && src_ch != device_ch;
            let native_rate = match (device_sr, state.sample_rate) {
                (Some(d), Some(s)) => d == s,
                _ => true,
            };

            let want_green = exclusive
                && unity_volume
                && unity_pregain
                && eq_bypass
                && crossfeed_off
                && convolver_off
                && limiter_off
                && dither_bypass
                && balance_off
                && native_rate
                && !upmix;

            let is_green = h.status == BitPerfectStatus::Green;
            prop_assert!(
                is_green == want_green,
                "Green ↔ all-unity Exclusive failed: \
                 exclusive={}, unity_volume={}, unity_pregain={}, \
                 eq={}, crossfeed_off={}, convolver_off={}, \
                 limiter_off={}, dither_bypass={}, balance_off={}, \
                 native={}, upmix={} ⇒ expected green={}, got {:?}",
                exclusive, unity_volume, unity_pregain,
                eq_bypass, crossfeed_off, convolver_off,
                limiter_off, dither_bypass, balance_off,
                native_rate, upmix, want_green, h.status
            );
        }
    );
}
