//! Ring-buffer sizing policy for the SPSC PCM ring (`R10`).
//!
//! [`RingPlan`] is a *pure* helper — no I/O, no global state — so it
//! can be exercised exhaustively by `proptest` (Property 7, task 4.2)
//! before any wiring into `run_decoder_thread` (task 14.1). It maps a
//! format transition (source vs. device sample rate / channel count)
//! onto a target ring capacity and a prefill threshold.
//!
//! Feature: qobee-beta-feedback-improvements, R10 `RingPlan`.

/// Sizing policy for the SPSC PCM ring. Pure and side-effect free,
/// hence directly testable by property tests (R10.1, R10.2, R10.5).
///
/// The design specifies `pub(crate)`, but the engine's integration
/// property test crate (`tests/properties/ring_plan.rs`, task 4.2)
/// lives outside the crate and must be able to construct and inspect
/// a `RingPlan`. We therefore expose it as `pub` while marking it
/// `#[doc(hidden)]` so it stays out of the documented public API
/// surface — the struct remains an internal sizing helper, just one
/// the test target can reach.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingPlan {
    /// Target capacity in interleaved samples.
    pub capacity: usize,
    /// Prefill threshold (in interleaved samples) before the callback
    /// is allowed to consume after a transition (R10.1).
    pub prefill_threshold: usize,
}

impl RingPlan {
    /// Compute a ring plan for a format transition.
    ///
    /// `base` is the baseline capacity (`RING_CAPACITY_SAMPLES`).
    /// `src_sr` / `device_sr` and `src_channels` / `dst_channels`
    /// describe the source and device sides of the transition. The
    /// capacity grows proportionally to the throughput ratio
    /// (`sr_ratio * ch_ratio`) so a higher device sample rate or an
    /// upmix that multiplies the sample throughput gets more slack
    /// (R10.2); the scale is clamped to `[1.0, 4.0]` so the capacity
    /// is never smaller than `base` and the latency never balloons.
    /// The prefill threshold is half the capacity (~50%), enough to
    /// absorb renegotiation latency without delaying startup (R10.1).
    pub fn compute(
        base: usize,
        src_sr: u32,
        device_sr: u32,
        src_channels: u16,
        dst_channels: u16,
    ) -> Self {
        let sr_ratio = (device_sr.max(1) as f64) / (src_sr.max(1) as f64);
        let ch_ratio = (dst_channels.max(1) as f64) / (src_channels.max(1) as f64);
        // jamais < base, plafonne x4
        let scale = (sr_ratio * ch_ratio).clamp(1.0, 4.0);
        let capacity = ((base as f64) * scale).round() as usize;
        // Pre-remplir ~50% de la capacite.
        let prefill_threshold = capacity / 2;
        Self {
            capacity,
            prefill_threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Baseline used by the decoder thread; mirrors
    /// `backend_cpal_shared::RING_CAPACITY_SAMPLES`.
    const BASE: usize = 48_000 * 2;

    #[test]
    fn no_transition_keeps_base_capacity() {
        // Same SR, same channel count → scale clamps to 1.0.
        let plan = RingPlan::compute(BASE, 48_000, 48_000, 2, 2);
        assert_eq!(plan.capacity, BASE);
        assert_eq!(plan.prefill_threshold, BASE / 2);
    }

    #[test]
    fn lower_device_rate_still_floors_at_base() {
        // device slower than source → raw scale < 1.0, clamped to 1.0.
        let plan = RingPlan::compute(BASE, 96_000, 44_100, 2, 2);
        assert_eq!(plan.capacity, BASE, "capacity must never drop below base");
    }

    #[test]
    fn higher_device_rate_grows_capacity() {
        // 96k device vs 48k source, stereo → scale = 2.0.
        let plan = RingPlan::compute(BASE, 48_000, 96_000, 2, 2);
        assert_eq!(plan.capacity, BASE * 2);
        assert_eq!(plan.prefill_threshold, plan.capacity / 2);
    }

    #[test]
    fn upmix_multiplies_throughput() {
        // Stereo source upmixed to 4 channels at same SR → scale = 2.0.
        let plan = RingPlan::compute(BASE, 48_000, 48_000, 2, 4);
        assert_eq!(plan.capacity, BASE * 2);
    }

    #[test]
    fn scale_is_clamped_to_four() {
        // 192k device + 2→8 upmix → raw scale = 4 * 4 = 16, clamped to 4.
        let plan = RingPlan::compute(BASE, 48_000, 192_000, 2, 8);
        assert_eq!(plan.capacity, BASE * 4, "scale must plateau at x4");
    }

    #[test]
    fn zero_inputs_do_not_panic() {
        // Defensive: zero SR / channels are floored to 1 internally.
        let plan = RingPlan::compute(BASE, 0, 0, 0, 0);
        assert_eq!(plan.capacity, BASE);
        assert!(plan.prefill_threshold > 0);
    }
}
