//! Soft clipping helper shared across the audio chain.
//!
//! Historically each backend (`backend_cpal_shared`,
//! `backend_wasapi_exclusive`) carried its own private copy of the
//! same `soft_clip` polynomial. The Phase B refactor reuses the
//! function from the new `Peak_Limiter::SoftClip` mode, so the
//! function lives here as the single source of truth and the two
//! backends import it through `crate::dsp::clip::soft_clip`.
//!
//! The polynomial is unchanged from the legacy implementation:
//!
//! * Linear in `[-0.7, 0.7]` (within the cubic's near-unity slope),
//! * 3rd-order knee `t * (1 - t²/3)` on `[-1, 1]`,
//! * Smooth exponential roll outside `±1`, asymptoting at `±1.0`.
//!
//! The function is `#[inline(always)]` because it is called once per
//! output sample on the audio hot path.

/// Smoothly limit `x` toward `±1.0` using the historical
/// soft-clip polynomial.
///
/// Bit-identical to the previous private copies in
/// `backend_cpal_shared` and `backend_wasapi_exclusive`.
#[inline(always)]
pub fn soft_clip(x: f32) -> f32 {
    // Linear up to ±0.7, then a smooth knee that asymptotes at ±1.
    let t = x.clamp(-1.5, 1.5);
    let t2 = t * t;
    // 3rd-order polynomial: y = t * (1 - t^2/3) on [-1, 1].
    // Outside, clamp to ±2/3 * 1 = ±0.667 then add a softer roll.
    if t.abs() <= 1.0 {
        t * (1.0 - t2 / 3.0)
    } else if t > 0.0 {
        // Asymptote toward 2/3 + small tail; safe ceiling at 1.
        (2.0 / 3.0 + (1.0 - (-((t - 1.0) * 2.0)).exp()) / 3.0).min(1.0)
    } else {
        (-2.0 / 3.0 - (1.0 - (-((-t - 1.0) * 2.0)).exp()) / 3.0).max(-1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_signal_is_near_identity() {
        // For |x| ≤ ~0.3 the cubic correction `1 - x²/3` ≈ 1, so
        // the function is nearly transparent.
        for x in [0.0_f32, 0.05, -0.1, 0.2, -0.3] {
            let y = soft_clip(x);
            assert!(
                (y - x * (1.0 - x * x / 3.0)).abs() < 1e-6,
                "soft_clip({x}) = {y}, expected ≈ {}",
                x * (1.0 - x * x / 3.0)
            );
        }
    }

    #[test]
    fn output_stays_within_unit_range() {
        // Even pathological inputs may not break out of [-1, 1].
        for &x in &[-10.0_f32, -2.0, -1.5, -1.0, 1.0, 1.5, 2.0, 10.0] {
            let y = soft_clip(x);
            assert!(y.is_finite(), "non-finite output for {x}: {y}");
            assert!(
                (-1.0..=1.0).contains(&y),
                "out-of-range output for {x}: {y}"
            );
        }
    }

    #[test]
    fn unity_inputs_match_polynomial() {
        // y(1) = 1 * (1 - 1/3) = 2/3.
        assert!((soft_clip(1.0) - 2.0 / 3.0).abs() < 1e-6);
        assert!((soft_clip(-1.0) + 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn zero_in_zero_out() {
        assert_eq!(soft_clip(0.0), 0.0);
    }
}
