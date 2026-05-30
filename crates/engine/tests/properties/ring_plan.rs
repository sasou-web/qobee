//! Property test for [`qobee_engine::RingPlan`] (task 4.2).
//!
//! `RingPlan::compute` is the pure ring-buffer sizing policy (R10).
//! It maps a format transition `(src_sr, device_sr, src_channels,
//! dst_channels)` onto a target `capacity` and a `prefill_threshold`,
//! scaling the baseline capacity by the throughput ratio clamped to
//! `[1.0, 4.0]`. Property 7 pins the four invariants the decoder
//! thread relies on (task 14.1):
//!
//!   * `capacity >= base` — the clamp floor at `1.0` means the ring
//!     is never smaller than the baseline (R10.5).
//!   * `capacity <= base * 4` — the clamp ceiling at `4.0` bounds the
//!     added latency (R10.2).
//!   * `0 < prefill_threshold <= capacity` — there is always
//!     something to prefill, and never more than the ring holds
//!     (R10.1).
//!   * monotonic non-decrease of `capacity` with respect to the
//!     bounded scale factor — a transition that demands more
//!     throughput never shrinks the ring (R10.2).

use proptest::prelude::*;

use qobee_engine::RingPlan;

/// Baseline capacity in interleaved samples. Mirrors
/// `backend_cpal_shared::RING_CAPACITY_SAMPLES` (`48_000 * 2`, ~1 s of
/// 48 kHz stereo) so the property test exercises the same magnitude
/// the decoder thread uses.
const BASE: usize = 48_000 * 2;

/// Generator for a ring-sizing transition. Sample rates span the
/// realistic PCM range (`8 kHz`..=`768 kHz`, i.e. telephony up to
/// DSD-over-PCM territory) and channel counts span `1..=8` (mono up
/// to 7.1). Zero is excluded on purpose: the formula floors zero to
/// `1` internally, so feeding zero would only re-test that defensive
/// clamp (already covered by a unit test) instead of the sizing law.
fn any_ring_inputs() -> impl Strategy<Value = (u32, u32, u16, u16)> {
    (
        8_000u32..=768_000u32,
        8_000u32..=768_000u32,
        1u16..=8u16,
        1u16..=8u16,
    )
}

/// Re-derive the bounded scale factor exactly the way
/// `RingPlan::compute` does, so the monotonicity invariant is checked
/// against the same `[1.0, 4.0]`-clamped quantity the implementation
/// uses — without importing private helpers.
fn bounded_scale(src_sr: u32, device_sr: u32, src_channels: u16, dst_channels: u16) -> f64 {
    let sr_ratio = (device_sr.max(1) as f64) / (src_sr.max(1) as f64);
    let ch_ratio = (dst_channels.max(1) as f64) / (src_channels.max(1) as f64);
    (sr_ratio * ch_ratio).clamp(1.0, 4.0)
}

/// Assert the per-plan sizing invariants shared by every generated
/// transition (factored out so both halves of the monotonicity pair
/// are checked).
fn assert_plan_invariants(plan: &RingPlan) -> Result<(), TestCaseError> {
    // capacity >= base (scale clamped to >= 1.0).
    prop_assert!(
        plan.capacity >= BASE,
        "capacity {} dropped below base {BASE}",
        plan.capacity
    );
    // capacity <= base * 4 (scale clamped to <= 4.0; round at the
    // clamp boundary is exact since base * 4.0 is integer-valued).
    prop_assert!(
        plan.capacity <= BASE * 4,
        "capacity {} exceeded base*4 {}",
        plan.capacity,
        BASE * 4
    );
    // 0 < prefill_threshold <= capacity.
    prop_assert!(
        plan.prefill_threshold > 0,
        "prefill_threshold must be strictly positive, got {}",
        plan.prefill_threshold
    );
    prop_assert!(
        plan.prefill_threshold <= plan.capacity,
        "prefill_threshold {} exceeded capacity {}",
        plan.prefill_threshold,
        plan.capacity
    );
    Ok(())
}

// Feature: qobee-beta-feedback-improvements, Property 7: Invariants de
// dimensionnement du Ring_Buffer.
//
// **Validates: Requirements 10.1, 10.2, 10.5**
#[test]
fn property_7_ring_plan_sizing_invariants() {
    proptest!(
        ProptestConfig::with_cases(256),
        |((src_sr_a, device_sr_a, src_ch_a, dst_ch_a) in any_ring_inputs(),
          (src_sr_b, device_sr_b, src_ch_b, dst_ch_b) in any_ring_inputs())| {

            let plan_a = RingPlan::compute(BASE, src_sr_a, device_sr_a, src_ch_a, dst_ch_a);
            let plan_b = RingPlan::compute(BASE, src_sr_b, device_sr_b, src_ch_b, dst_ch_b);

            // Per-plan invariants: capacity bounds and prefill bounds.
            assert_plan_invariants(&plan_a)?;
            assert_plan_invariants(&plan_b)?;

            // Monotonic non-decrease of capacity wrt the bounded scale
            // factor: round(base * scale) is monotone in `scale`, so a
            // transition with a larger (clamped) scale must not yield a
            // smaller capacity.
            let scale_a = bounded_scale(src_sr_a, device_sr_a, src_ch_a, dst_ch_a);
            let scale_b = bounded_scale(src_sr_b, device_sr_b, src_ch_b, dst_ch_b);
            if scale_a <= scale_b {
                prop_assert!(
                    plan_a.capacity <= plan_b.capacity,
                    "monotonicity violated: scale_a {scale_a} <= scale_b {scale_b} \
                     but capacity_a {} > capacity_b {}",
                    plan_a.capacity,
                    plan_b.capacity
                );
            } else {
                prop_assert!(
                    plan_a.capacity >= plan_b.capacity,
                    "monotonicity violated: scale_a {scale_a} > scale_b {scale_b} \
                     but capacity_a {} < capacity_b {}",
                    plan_a.capacity,
                    plan_b.capacity
                );
            }
        }
    );
}
