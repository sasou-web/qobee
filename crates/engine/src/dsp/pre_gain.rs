//! `Pre_Gain_Stage` — combined ReplayGain + true-peak protection +
//! volume-curve gain (R1, R4).
//!
//! This stage multiplies every sample by a single scalar `g_linear`
//! recomputed on three triggers:
//!
//!   1. `reconfigure(...)` — when [`AudioSettings::version`] changed
//!      (peak ceiling, headroom, volume curve, floor).
//!   2. [`PreGainStage::set_context`] — when the decoder thread loads
//!      a new track (RG dB and peak from tags) or the user moves the
//!      volume slider.
//!   3. Construction — `PreGainStage::new` runs the same recompute
//!      with the supplied settings and a default context (no RG, no
//!      peak, slider at unity → `g_linear = 1.0`).
//!
//! The four-step compute matches the design's pseudocode literally:
//! demanded RG → true-peak ceiling → reported attenuation → volume
//! curve with mute-exact at `v=0` and unity-exact at `v=1`.
//!
//! ## Bypass
//!
//! `is_bypass()` returns `true` when `|g_linear − 1.0| < 1e-6`, which
//! satisfies R4.3 (slider = 1.0 + RG = 0 dB ⇒ unity-exact ⇒
//! sample-for-sample passthrough).

use crate::audio_settings::{AudioSettings, VolumeCurve};
use crate::dsp::DspStage;

/// Per-load context the decoder thread feeds to the stage.
///
/// `rg_db` and `rg_peak` come from the loaded track's tags via the
/// orchestrator (`Player::start_track`); `slider` is the current
/// volume position in `[0.0, 1.0]` read from `Shared::volume_public`.
///
/// `Default` produces "no RG, slider at unity" — a context that
/// resolves to `g_linear = 1.0` and lets the stage stay bypassed
/// until real values are pushed in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreGainContext {
    pub rg_db: Option<f32>,
    pub rg_peak: Option<f32>,
    /// Volume slider position in `[0.0, 1.0]`.
    pub slider: f32,
}

impl Default for PreGainContext {
    fn default() -> Self {
        Self {
            rg_db: None,
            rg_peak: None,
            slider: 1.0,
        }
    }
}

/// Combined RG + true-peak + volume-curve scalar gain stage.
pub struct PreGainStage {
    g_linear: f32,
    rg_attenuation_db: f32,
    bypass: bool,

    // Cached settings consumed by `recompute_inner`. Mirrors the
    // subset of `AudioSettings` this stage actually reads.
    peak_protection_enabled: bool,
    safety_headroom_db: f32,
    ceiling_dbfs: f32,
    curve: VolumeCurve,
    floor_db: f32,

    /// Last context supplied via `set_context` (or default at
    /// construction time).
    ctx: PreGainContext,
}

impl PreGainStage {
    /// Build a fresh stage from the supplied settings. The initial
    /// context has no RG and a unity slider, so a default
    /// configuration produces `g_linear = 1.0` and `is_bypass() ==
    /// true`.
    pub fn new(settings: &AudioSettings, _sample_rate: u32, _channels: u16) -> Self {
        let mut s = Self {
            g_linear: 1.0,
            rg_attenuation_db: 0.0,
            bypass: true,
            peak_protection_enabled: settings.rg_peak_protection,
            safety_headroom_db: settings.rg_safety_headroom_db,
            ceiling_dbfs: settings.peak_limiter_ceiling_dbfs,
            curve: settings.volume_curve,
            floor_db: settings.volume_floor_db,
            ctx: PreGainContext::default(),
        };
        s.recompute_inner();
        s
    }

    /// Push a new per-load context (RG dB/peak from the track and
    /// the current volume slider). Triggers a `recompute_inner`.
    pub fn set_context(&mut self, ctx: PreGainContext) {
        self.ctx = ctx;
        self.recompute_inner();
    }

    /// Last reported RG attenuation in dB. `0.0` when the demanded
    /// RG gain fits within the true-peak ceiling; negative when the
    /// stage had to reduce it (R1.5).
    pub fn rg_attenuation_db(&self) -> f32 {
        self.rg_attenuation_db
    }

    /// Current combined linear gain. Useful for tests and for the
    /// `BitPerfectHealth` calculator (task 24).
    pub fn linear_gain(&self) -> f32 {
        self.g_linear
    }

    /// Snapshot of the last context, for diagnostics.
    pub fn context(&self) -> PreGainContext {
        self.ctx
    }

    /// Implement the design's four-step pseudocode against the
    /// cached settings + ctx.
    ///
    /// Note on the no-RG path: when `rg_db.is_none()` we skip the
    /// true-peak protection entirely — the user has not asked for
    /// any RG correction, so there is no boost to protect against
    /// and the stage stays at unity (modulo the volume curve). Peak
    /// protection only kicks in when `rg_db.is_some()`; if its
    /// matching `rg_peak` tag is absent we fall back to the
    /// conservative `1.0` (R1.3).
    fn recompute_inner(&mut self) {
        let (rg_linear_demanded, rg_linear_applied, attenuation_db) = match self.ctx.rg_db {
            None => {
                // No RG requested: unity gain, no clamp, no
                // attenuation reported.
                (1.0_f32, 1.0_f32, 0.0_f32)
            }
            Some(db) => {
                // 1) demanded RG gain (clamped to the same range used
                //    by the legacy `Player::start_track` math).
                let rg_target = db.clamp(-24.0, 12.0);
                let demanded = 10f32.powf(rg_target / 20.0);

                // 2) true-peak ceiling: g × peak ≤ ceiling_linear / safety.
                //    Missing peak ⇒ assume 1.0 (R1.3 — full-scale fallback).
                let peak = self.ctx.rg_peak.unwrap_or(1.0).max(1e-6);
                let ceiling_linear = 10f32.powf(self.ceiling_dbfs / 20.0);
                let headroom = if self.peak_protection_enabled {
                    10f32.powf(-self.safety_headroom_db / 20.0)
                } else {
                    1.0
                };
                let max_rg_linear = (ceiling_linear * headroom) / peak;
                let applied = demanded.min(max_rg_linear);

                // 3) reported attenuation (R1.5 surface for `PlayerState`).
                let attn = if applied < demanded - 1e-9 {
                    20.0 * (applied / demanded).log10()
                } else {
                    0.0
                };
                (demanded, applied, attn)
            }
        };
        let _ = rg_linear_demanded; // kept for clarity / future logging

        // 4) volume curve with mute-exact at v=0 and unity-exact at v=1.
        let slider = self.ctx.slider;
        let v_linear = if slider <= 0.0 {
            // R4.2 — mute-exact, no `10^(floor/20)` floor leak.
            0.0
        } else if (slider - 1.0).abs() < 1e-9 {
            // R4.3 — unity-exact regardless of the active curve.
            1.0
        } else {
            match self.curve {
                VolumeCurve::Quadratic => slider.clamp(0.0, 1.0).powi(2),
                VolumeCurve::Logarithmic => {
                    let db = self.floor_db
                        + (0.0 - self.floor_db) * slider.clamp(0.0, 1.0);
                    10f32.powf(db / 20.0)
                }
            }
        };

        self.g_linear = rg_linear_applied * v_linear;
        self.rg_attenuation_db = attenuation_db;
        self.bypass = (self.g_linear - 1.0).abs() < 1e-6;
    }
}

impl DspStage for PreGainStage {
    fn reconfigure(
        &mut self,
        settings: &AudioSettings,
        _sample_rate: u32,
        _channels: u16,
    ) {
        self.peak_protection_enabled = settings.rg_peak_protection;
        self.safety_headroom_db = settings.rg_safety_headroom_db;
        self.ceiling_dbfs = settings.peak_limiter_ceiling_dbfs;
        self.curve = settings.volume_curve;
        self.floor_db = settings.volume_floor_db;
        self.recompute_inner();
    }

    fn is_bypass(&self) -> bool {
        self.bypass
    }

    fn process_inplace(&mut self, samples: &mut [f32]) {
        if self.bypass {
            return;
        }
        let g = self.g_linear;
        for s in samples.iter_mut() {
            *s *= g;
        }
    }

    fn reset(&mut self) {
        // Stage has no temporal state; nothing to reset.
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AudioSettings {
        AudioSettings::default()
    }

    #[test]
    fn default_construction_is_unity() {
        let s = PreGainStage::new(&settings(), 48_000, 2);
        assert!(s.is_bypass(), "default stage must be bypassed");
        assert!(
            (s.linear_gain() - 1.0).abs() < 1e-6,
            "default linear gain must be ~1.0, got {}",
            s.linear_gain()
        );
        assert_eq!(s.rg_attenuation_db(), 0.0);
    }

    #[test]
    fn process_inplace_passthrough_when_bypass() {
        let mut s = PreGainStage::new(&settings(), 48_000, 2);
        let original: Vec<f32> = vec![
            0.0, 1.0, -1.0, 0.5, -0.5, 0.123_456_7, -0.987_654_3, 1e-9, -1e-9,
            0.999_999_94,
        ];
        let mut buf = original.clone();
        s.process_inplace(&mut buf);
        assert_eq!(
            buf, original,
            "bypass path must not touch the buffer (sample-for-sample passthrough)"
        );
    }

    #[test]
    fn mute_exact_at_slider_zero() {
        // Setting slider to 0.0 must produce a strict zero gain (R4.2),
        // even with the logarithmic curve whose `10^(floor/20)` would
        // otherwise leak a tiny non-zero value.
        let mut s = PreGainStage::new(&settings(), 48_000, 2);
        s.set_context(PreGainContext {
            rg_db: None,
            rg_peak: None,
            slider: 0.0,
        });

        assert_eq!(s.linear_gain(), 0.0, "mute must be exact zero");
        assert!(!s.is_bypass(), "mute is not a bypass — output must be silenced");

        let mut buf = vec![1.0_f32; 256];
        s.process_inplace(&mut buf);
        assert!(
            buf.iter().all(|&v| v == 0.0),
            "mute output must be all zeros"
        );
    }

    #[test]
    fn unity_at_slider_one_no_rg() {
        // R4.3 — slider=1.0 with no RG gain must yield linear_gain=1.0
        // and is_bypass()==true so the buffer flows through unchanged.
        let mut s = PreGainStage::new(&settings(), 48_000, 2);
        s.set_context(PreGainContext {
            rg_db: None,
            rg_peak: None,
            slider: 1.0,
        });
        assert!(
            (s.linear_gain() - 1.0).abs() < 1e-9,
            "unity-exact at v=1 (got {})",
            s.linear_gain()
        );
        assert!(s.is_bypass());
    }

    #[test]
    fn true_peak_protection_clamps_gain() {
        // Demanded RG = +6 dB. Track peak = 0.95 (full-scale-ish).
        // Ceiling = -1 dBFS, headroom = 1 dB ⇒ effective allowed
        // peak after gain ≤ 10^(-2/20) ≈ 0.7943.
        let mut audio = settings();
        audio.rg_peak_protection = true;
        audio.rg_safety_headroom_db = 1.0;
        audio.peak_limiter_ceiling_dbfs = -1.0;

        let mut s = PreGainStage::new(&audio, 48_000, 2);
        s.set_context(PreGainContext {
            rg_db: Some(6.0),
            rg_peak: Some(0.95),
            slider: 1.0,
        });

        let g = s.linear_gain();
        let post_peak = g * 0.95;
        let ceiling_after_safety = 10f32.powf(-2.0 / 20.0);
        assert!(
            post_peak <= ceiling_after_safety + 1e-6,
            "post-gain peak {post_peak} must not exceed ceiling*safety {ceiling_after_safety}"
        );

        // The stage had to clamp the demanded gain ⇒ negative
        // attenuation reported (R1.5).
        assert!(
            s.rg_attenuation_db() < 0.0,
            "expected negative rg_attenuation_db when ceiling clamped, got {}",
            s.rg_attenuation_db()
        );

        // And the stage is no longer bypassed because g != 1.0.
        assert!(!s.is_bypass());
    }

    #[test]
    fn attenuation_zero_when_no_clamp() {
        // Demanded RG = -6 dB. Track peak = 0.5. Even with safety
        // headroom, peak * gain = 0.5 * 10^(-6/20) ≈ 0.25, well below
        // any reasonable ceiling — no clamp must fire.
        let mut s = PreGainStage::new(&settings(), 48_000, 2);
        s.set_context(PreGainContext {
            rg_db: Some(-6.0),
            rg_peak: Some(0.5),
            slider: 1.0,
        });
        assert_eq!(
            s.rg_attenuation_db(),
            0.0,
            "no clamp ⇒ attenuation must be exactly zero, got {}",
            s.rg_attenuation_db()
        );
        // Demanded gain is below 1.0, so the stage is *not* bypassed.
        assert!(s.linear_gain() < 1.0);
        assert!(!s.is_bypass());
    }

    #[test]
    fn missing_peak_falls_back_to_full_scale() {
        // R1.3: when the file has no peak tag, the stage must use a
        // conservative full-scale (1.0) peak. With +6 dB demanded and
        // a -1 dBFS ceiling + 1 dB safety, the clamp must fire.
        let mut audio = settings();
        audio.rg_peak_protection = true;
        audio.rg_safety_headroom_db = 1.0;
        audio.peak_limiter_ceiling_dbfs = -1.0;

        let mut s = PreGainStage::new(&audio, 48_000, 2);
        s.set_context(PreGainContext {
            rg_db: Some(6.0),
            rg_peak: None,
            slider: 1.0,
        });

        // ceiling*safety = 10^(-2/20) ≈ 0.7943; demanded = 10^(6/20)
        // ≈ 1.995. The stage must apply the ceiling, not the request.
        let expected = 10f32.powf(-2.0 / 20.0);
        assert!(
            (s.linear_gain() - expected).abs() < 1e-5,
            "expected {expected}, got {}",
            s.linear_gain()
        );
        assert!(s.rg_attenuation_db() < 0.0);
    }

    #[test]
    fn process_inplace_applies_gain_when_active() {
        // RG = -6 dB ⇒ g_linear ≈ 0.5012. Multiply a known buffer
        // and check the output matches sample-by-sample.
        let mut s = PreGainStage::new(&settings(), 48_000, 2);
        s.set_context(PreGainContext {
            rg_db: Some(-6.0),
            rg_peak: Some(0.5),
            slider: 1.0,
        });
        let g = s.linear_gain();
        assert!(!s.is_bypass());

        let input: Vec<f32> = vec![0.1, -0.2, 0.3, -0.4, 0.5, -0.5, 1.0, -1.0];
        let mut buf = input.clone();
        s.process_inplace(&mut buf);

        for (i, (got, src)) in buf.iter().zip(input.iter()).enumerate() {
            let want = src * g;
            assert!(
                (got - want).abs() < 1e-7,
                "sample {i}: got {got}, expected {want}"
            );
        }
    }

    #[test]
    fn reconfigure_picks_up_new_settings() {
        // Start with peak protection ON, then reconfigure with it OFF
        // and verify the stage returns to unity (no clamp, no
        // attenuation reported).
        let mut audio = settings();
        audio.rg_peak_protection = true;
        audio.rg_safety_headroom_db = 1.0;
        audio.peak_limiter_ceiling_dbfs = -1.0;

        let mut s = PreGainStage::new(&audio, 48_000, 2);
        s.set_context(PreGainContext {
            rg_db: Some(6.0),
            rg_peak: Some(0.95),
            slider: 1.0,
        });
        assert!(s.rg_attenuation_db() < 0.0);

        // Reconfigure with the safety net disabled — no clamp.
        let mut audio2 = audio.clone();
        audio2.rg_peak_protection = false;
        s.reconfigure(&audio2, 48_000, 2);

        let demanded = 10f32.powf(6.0 / 20.0);
        // With peak protection off, headroom = 1.0, so the only cap
        // is the ceiling itself: 10^(-1/20) / 0.95 ≈ 0.939, which is
        // below the demanded 1.995 — clamp still fires, but the
        // attenuation differs.
        let cap = 10f32.powf(-1.0 / 20.0) / 0.95;
        assert!(
            (s.linear_gain() - demanded.min(cap)).abs() < 1e-5,
            "after reconfigure: got {}",
            s.linear_gain()
        );
    }
}
