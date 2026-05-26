//! Centralised audio settings (snapshot read by the decoder thread).
//!
//! `AudioSettings` is the single source of truth for every `audio.*`
//! parameter exposed by the audio-quality-improvements spec. The
//! struct is meant to be wrapped in an `arc_swap::ArcSwap` on the
//! engine `Shared` state so the decoder thread can lazily reload it
//! at every chunk boundary without contending a lock.
//!
//! The struct itself does not know how to read or write SQLite — that
//! responsibility belongs to the future `AudioSettingsStore` (see
//! `crates/core/src/audio_settings.rs`, task 4). Each `audio.<field>`
//! key in the settings table maps to the matching field below by
//! stripping the `audio.` prefix.
//!
//! ## Defaults and bounds
//!
//! See `design.md` § "Settings keys". `AudioSettings::default()`
//! produces exactly the table's default row. `clamp_in_place` brings
//! out-of-range values back to the boundaries (the store layer calls
//! it after deserialising rows that may have been written by an older
//! build with a wider range).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Enums
// -----------------------------------------------------------------------------

/// Dither shaping profile (R2). Only relevant when the negotiated
/// output format is ≤ 16 bits integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DitherProfile {
    /// Triangular PDF, white noise (legacy behaviour).
    Tpdf,
    /// First/second-order high-pass shaped noise.
    ShapedHp,
    /// F-weighted shaping (Lipshitz simplifié) — default.
    ShapedFWeighted,
}

/// Peak limiter mode (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeakLimiterMode {
    /// No limiter and no soft-clipper.
    Off,
    /// Cubic soft-clip (legacy fallback).
    SoftClip,
    /// Look-ahead true-peak limiter — default.
    LookaheadLimiter,
}

/// Volume curve (R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeCurve {
    /// `gain = v * v` (legacy behaviour, kept for compatibility).
    Quadratic,
    /// `gain_db = lerp(floor_db, 0.0, v)` — default.
    Logarithmic,
}

/// Crossfeed preset (R6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossfeedPreset {
    /// BS2B / Jan Meier moderate — default.
    Bauer,
    /// BS2B strong.
    BauerStrong,
    /// User-defined `crossfeed_delay_us` and `crossfeed_lp_cutoff_hz`.
    Custom,
}

/// Resampler quality preset (R8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResamplerQuality {
    /// Lighter sinc (`sinc_len ≤ 128`).
    Standard,
    /// Existing high-quality sinc — default.
    Best,
}

// -----------------------------------------------------------------------------
// AudioSettings
// -----------------------------------------------------------------------------

/// Snapshot of every `audio.*` setting consumed by the DSP pipeline.
///
/// Field names are the plain (snake_case) equivalent of the settings
/// keys; the `audio.` prefix is the responsibility of the persistence
/// layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    /// Bumped by the store on every successful mutation. The decoder
    /// thread compares this against its last seen version to decide
    /// whether to call `reconfigure(...)` on the DSP chain.
    #[serde(default)]
    pub version: u64,

    // ---- R1 — RG peak protection ----
    #[serde(default = "default_rg_peak_protection")]
    pub rg_peak_protection: bool,
    #[serde(default = "default_rg_safety_headroom_db")]
    pub rg_safety_headroom_db: f32,

    // ---- R2 — dither ----
    #[serde(default = "default_dither_profile")]
    pub dither_profile: DitherProfile,

    // ---- R3 — peak limiter ----
    #[serde(default = "default_peak_limiter_mode")]
    pub peak_limiter_mode: PeakLimiterMode,
    #[serde(default = "default_peak_limiter_ceiling_dbfs")]
    pub peak_limiter_ceiling_dbfs: f32,
    #[serde(default = "default_peak_limiter_lookahead_ms")]
    pub peak_limiter_lookahead_ms: f32,
    #[serde(default = "default_peak_limiter_release_ms")]
    pub peak_limiter_release_ms: f32,

    // ---- R4 — volume curve ----
    #[serde(default = "default_volume_curve")]
    pub volume_curve: VolumeCurve,
    #[serde(default = "default_volume_floor_db")]
    pub volume_floor_db: f32,

    // ---- R6 — crossfeed ----
    #[serde(default)]
    pub crossfeed_enabled: bool,
    #[serde(default = "default_crossfeed_preset")]
    pub crossfeed_preset: CrossfeedPreset,
    #[serde(default = "default_crossfeed_delay_us")]
    pub crossfeed_delay_us: f32,
    #[serde(default = "default_crossfeed_lp_cutoff_hz")]
    pub crossfeed_lp_cutoff_hz: f32,

    // ---- R8 — resampler quality ----
    #[serde(default = "default_resampler_quality")]
    pub resampler_quality: ResamplerQuality,

    // ---- R9 — convolver ----
    #[serde(default)]
    pub convolver_enabled: bool,
    #[serde(default)]
    pub convolver_ir_path: Option<PathBuf>,
    #[serde(default = "default_convolver_gain_db")]
    pub convolver_gain_db: f32,

    // ---- R10 — channel balance ----
    #[serde(default)]
    pub balance: f32,
    #[serde(default)]
    pub trim_db_per_channel: Vec<f32>,
}

// ---- default helpers (used both by Default and serde(default = "...")) ----

fn default_rg_peak_protection() -> bool {
    true
}
fn default_rg_safety_headroom_db() -> f32 {
    1.0
}
fn default_dither_profile() -> DitherProfile {
    DitherProfile::ShapedFWeighted
}
fn default_peak_limiter_mode() -> PeakLimiterMode {
    PeakLimiterMode::LookaheadLimiter
}
fn default_peak_limiter_ceiling_dbfs() -> f32 {
    -1.0
}
fn default_peak_limiter_lookahead_ms() -> f32 {
    5.0
}
fn default_peak_limiter_release_ms() -> f32 {
    100.0
}
fn default_volume_curve() -> VolumeCurve {
    VolumeCurve::Logarithmic
}
fn default_volume_floor_db() -> f32 {
    -60.0
}
fn default_crossfeed_preset() -> CrossfeedPreset {
    CrossfeedPreset::Bauer
}
fn default_crossfeed_delay_us() -> f32 {
    300.0
}
fn default_crossfeed_lp_cutoff_hz() -> f32 {
    700.0
}
fn default_resampler_quality() -> ResamplerQuality {
    ResamplerQuality::Best
}
fn default_convolver_gain_db() -> f32 {
    -6.0
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            version: 0,
            rg_peak_protection: default_rg_peak_protection(),
            rg_safety_headroom_db: default_rg_safety_headroom_db(),
            dither_profile: default_dither_profile(),
            peak_limiter_mode: default_peak_limiter_mode(),
            peak_limiter_ceiling_dbfs: default_peak_limiter_ceiling_dbfs(),
            peak_limiter_lookahead_ms: default_peak_limiter_lookahead_ms(),
            peak_limiter_release_ms: default_peak_limiter_release_ms(),
            volume_curve: default_volume_curve(),
            volume_floor_db: default_volume_floor_db(),
            crossfeed_enabled: false,
            crossfeed_preset: default_crossfeed_preset(),
            crossfeed_delay_us: default_crossfeed_delay_us(),
            crossfeed_lp_cutoff_hz: default_crossfeed_lp_cutoff_hz(),
            resampler_quality: default_resampler_quality(),
            convolver_enabled: false,
            convolver_ir_path: None,
            convolver_gain_db: default_convolver_gain_db(),
            balance: 0.0,
            trim_db_per_channel: Vec::new(),
        }
    }
}

impl AudioSettings {
    /// Maximum number of trim entries the engine keeps. Excess
    /// entries are truncated by `clamp_in_place`.
    pub const MAX_CHANNELS: usize = 8;

    /// Bring every numeric field back into the bounds defined by the
    /// design's "Settings keys" table. Enums are constrained by the
    /// type system, so they are not touched here.
    ///
    /// Behaviour:
    ///   * Each `f32` is clamped to its `[min, max]` range.
    ///   * `trim_db_per_channel` is truncated to at most
    ///     [`AudioSettings::MAX_CHANNELS`] entries; each remaining
    ///     entry is clamped to `[-12.0, 0.0]`.
    pub fn clamp_in_place(&mut self) {
        self.rg_safety_headroom_db = self.rg_safety_headroom_db.clamp(0.0, 3.0);

        self.peak_limiter_ceiling_dbfs = self.peak_limiter_ceiling_dbfs.clamp(-3.0, 0.0);
        self.peak_limiter_lookahead_ms = self.peak_limiter_lookahead_ms.clamp(2.0, 10.0);
        self.peak_limiter_release_ms = self.peak_limiter_release_ms.clamp(20.0, 500.0);

        self.volume_floor_db = self.volume_floor_db.clamp(-80.0, -30.0);

        self.crossfeed_delay_us = self.crossfeed_delay_us.clamp(200.0, 400.0);
        self.crossfeed_lp_cutoff_hz = self.crossfeed_lp_cutoff_hz.clamp(500.0, 1500.0);

        self.convolver_gain_db = self.convolver_gain_db.clamp(-24.0, 0.0);

        self.balance = self.balance.clamp(-1.0, 1.0);

        if self.trim_db_per_channel.len() > Self::MAX_CHANNELS {
            self.trim_db_per_channel.truncate(Self::MAX_CHANNELS);
        }
        for trim in &mut self.trim_db_per_channel {
            *trim = trim.clamp(-12.0, 0.0);
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_settings_keys_table() {
        let s = AudioSettings::default();

        assert_eq!(s.version, 0);

        // R1
        assert!(s.rg_peak_protection);
        assert_eq!(s.rg_safety_headroom_db, 1.0);

        // R2
        assert_eq!(s.dither_profile, DitherProfile::ShapedFWeighted);

        // R3
        assert_eq!(s.peak_limiter_mode, PeakLimiterMode::LookaheadLimiter);
        assert_eq!(s.peak_limiter_ceiling_dbfs, -1.0);
        assert_eq!(s.peak_limiter_lookahead_ms, 5.0);
        assert_eq!(s.peak_limiter_release_ms, 100.0);

        // R4
        assert_eq!(s.volume_curve, VolumeCurve::Logarithmic);
        assert_eq!(s.volume_floor_db, -60.0);

        // R6
        assert!(!s.crossfeed_enabled);
        assert_eq!(s.crossfeed_preset, CrossfeedPreset::Bauer);
        assert_eq!(s.crossfeed_delay_us, 300.0);
        assert_eq!(s.crossfeed_lp_cutoff_hz, 700.0);

        // R8
        assert_eq!(s.resampler_quality, ResamplerQuality::Best);

        // R9
        assert!(!s.convolver_enabled);
        assert!(s.convolver_ir_path.is_none());
        assert_eq!(s.convolver_gain_db, -6.0);

        // R10
        assert_eq!(s.balance, 0.0);
        assert!(s.trim_db_per_channel.is_empty());
    }

    #[test]
    fn clamp_in_place_brings_out_of_range_values_back() {
        let mut s = AudioSettings {
            rg_safety_headroom_db: 99.0,
            peak_limiter_ceiling_dbfs: 5.0,
            peak_limiter_lookahead_ms: 0.5,
            peak_limiter_release_ms: 9_999.0,
            volume_floor_db: -200.0,
            crossfeed_delay_us: 10.0,
            crossfeed_lp_cutoff_hz: 50_000.0,
            convolver_gain_db: 12.0,
            balance: -7.5,
            ..AudioSettings::default()
        };

        s.clamp_in_place();

        assert_eq!(s.rg_safety_headroom_db, 3.0);
        assert_eq!(s.peak_limiter_ceiling_dbfs, 0.0);
        assert_eq!(s.peak_limiter_lookahead_ms, 2.0);
        assert_eq!(s.peak_limiter_release_ms, 500.0);
        assert_eq!(s.volume_floor_db, -80.0);
        assert_eq!(s.crossfeed_delay_us, 200.0);
        assert_eq!(s.crossfeed_lp_cutoff_hz, 1500.0);
        assert_eq!(s.convolver_gain_db, 0.0);
        assert_eq!(s.balance, -1.0);

        // Clamp also handles the lower side.
        let mut t = AudioSettings {
            rg_safety_headroom_db: -10.0,
            peak_limiter_ceiling_dbfs: -10.0,
            balance: 7.5,
            convolver_gain_db: -100.0,
            ..AudioSettings::default()
        };
        t.clamp_in_place();
        assert_eq!(t.rg_safety_headroom_db, 0.0);
        assert_eq!(t.peak_limiter_ceiling_dbfs, -3.0);
        assert_eq!(t.balance, 1.0);
        assert_eq!(t.convolver_gain_db, -24.0);
    }

    #[test]
    fn clamp_in_place_truncates_trim_to_eight_channels() {
        let mut s = AudioSettings {
            // 12 entries, mix of in-range and out-of-range values.
            trim_db_per_channel: vec![
                0.0, -3.0, -6.0, -9.0, -12.0, -15.0, 5.0, -1.0, // 8 valid + first OOB
                -100.0, 0.5, -7.0, -2.5, // these must be truncated
            ],
            ..AudioSettings::default()
        };

        s.clamp_in_place();

        assert_eq!(s.trim_db_per_channel.len(), AudioSettings::MAX_CHANNELS);
        assert_eq!(
            s.trim_db_per_channel,
            vec![0.0, -3.0, -6.0, -9.0, -12.0, -12.0, 0.0, -1.0]
        );
    }

    #[test]
    fn dither_profile_serializes_as_snake_case() {
        let json = serde_json::to_string(&DitherProfile::ShapedFWeighted).unwrap();
        assert_eq!(json, "\"shaped_f_weighted\"");

        let back: DitherProfile = serde_json::from_str("\"shaped_f_weighted\"").unwrap();
        assert_eq!(back, DitherProfile::ShapedFWeighted);

        // Round-trip on the rest of the variants too, to make sure
        // the snake_case rename applies consistently.
        for v in [
            DitherProfile::Tpdf,
            DitherProfile::ShapedHp,
            DitherProfile::ShapedFWeighted,
        ] {
            let s = serde_json::to_string(&v).unwrap();
            let r: DitherProfile = serde_json::from_str(&s).unwrap();
            assert_eq!(v, r);
        }
    }
}
