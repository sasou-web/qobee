//! Shared engine types: PCM buffer enum, output mode, player state, events.

use serde::{Deserialize, Serialize};

/// User-facing output mode. Kept as an enum (not a single variant) so
/// the API can be extended without breaking the IPC layer (e.g. an ASIO
/// backend in the future).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OutputMode {
    /// Pick whatever is available (currently always Shared).
    #[default]
    Auto,
    /// OS-mixer path (CPAL / WASAPI Shared on Windows). Supports
    /// concurrent playback with other applications.
    Shared,
    /// WASAPI Exclusive on Windows. Bit-perfect when the device
    /// natively supports the source sample rate and bit depth.
    /// Other applications cannot play to the same device while
    /// Qobee is active.
    Exclusive,
    /// ASIO on Windows (low-latency, exclusive, bit-perfect path
    /// favoured by audiophiles and pro-audio interfaces). Requires
    /// an ASIO driver for the target device. The backend is only
    /// compiled when the engine is built with the `engine-asio`
    /// feature *and* the Steinberg ASIO SDK is available at build
    /// time; otherwise selecting it surfaces a clean
    /// `BackendUnavailable` and the orchestrator falls back to
    /// Shared.
    Asio,
}

/// Effective output mode reported back to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveOutputMode {
    /// Running in Shared (the OS mixer is in the chain).
    Shared,
    /// Running in WASAPI Exclusive (the OS mixer is bypassed).
    Exclusive,
    /// Running through an ASIO driver (the OS mixer is bypassed).
    Asio,
}

/// Description of an output endpoint the user can pick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputDevice {
    /// Stable identifier (CPAL name on Windows). Matches what the UI
    /// passes back when calling `set_output_device`.
    pub id: String,
    /// Human-readable label. May be the same as `id` on most hosts.
    pub name: String,
    /// True if this is the system default at the time of the call.
    pub is_default: bool,
    /// Native sample rate the device's default config exposes (Hz).
    pub default_sample_rate: u32,
    /// Channel count of the device's default config.
    pub channels: u16,
}

/// High-level playback status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PlaybackStatus {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Stopped,
    Errored,
}

/// Format descriptor for the currently playing track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackFormat {
    pub sample_rate: u32,
    pub channels: u16,
    /// Bit depth as reported by the decoder, when known.
    pub bit_depth: Option<u8>,
}

/// PCM buffer carried through the decode â†’ output pipeline.
///
/// Only [`PcmBuffer::F32Interleaved`] is produced by the MVP, but the
/// integer variants are part of the public API so a future bit-perfect
/// path (WASAPI Exclusive) can plug in without breaking downstream code.
#[derive(Debug, Clone)]
pub enum PcmBuffer {
    /// 32-bit float, interleaved. Used by SharedCpal.
    F32Interleaved(Vec<f32>),
    /// 16-bit signed, interleaved.
    I16Interleaved(Vec<i16>),
    /// 24-bit signed packed in i32 (low 24 bits significant), interleaved.
    /// Reserved for the WASAPI Exclusive integer path.
    I24In32Interleaved(Vec<i32>),
    /// 32-bit signed, interleaved.
    I32Interleaved(Vec<i32>),
}

impl PcmBuffer {
    /// Number of interleaved samples (frames * channels) currently held.
    pub fn len(&self) -> usize {
        match self {
            PcmBuffer::F32Interleaved(v) => v.len(),
            PcmBuffer::I16Interleaved(v) => v.len(),
            PcmBuffer::I24In32Interleaved(v) => v.len(),
            PcmBuffer::I32Interleaved(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Source → destination channel layout recorded when the engine had
/// to upmix a stereo (or otherwise narrower) source onto a device
/// configured for a wider channel layout (R9.5).
///
/// Only present on [`PlayerState::upmix`] when an upmix is actually
/// active: a `None` upmix means the device played the source's own
/// channel layout (or a native stereo configuration) with no upmix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpmixInfo {
    /// Source channel count (e.g. `2` for a stereo file).
    pub src_channels: u16,
    /// Destination channel count the device was configured for
    /// (e.g. `8` for a 7.1 surround layout).
    pub dst_channels: u16,
}

/// Snapshot of player state, suitable for serialization to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    /// Identifier of the currently loaded track, if any. Opaque to the
    /// engine; assigned by [`qobee-core`](../qobee_core/index.html).
    pub current_track_id: Option<String>,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    /// Current software volume in `[0.0, 1.0]`.
    pub volume: f32,
    /// Mode the engine is *actually* running in.
    pub output_mode: EffectiveOutputMode,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u16>,
    /// True only on a strictly bit-perfect chain. Always `false` in
    /// Shared mode (the OS mixer is in the path); kept on the state
    /// for forward-compatibility and so the UI can render the truth.
    pub is_bit_perfect: bool,
    /// Reported RG attenuation in dB (R1.5). `Some(neg_db)` when the
    /// `Pre_Gain_Stage` had to clamp the demanded RG gain to fit the
    /// configured true-peak ceiling; `Some(0.0)` or `None` otherwise.
    /// Surfaced for diagnostics and for the BitPerfectHealth panel
    /// to show the user *why* their RG offset was reduced.
    #[serde(default)]
    pub rg_attenuation_db: Option<f32>,
    /// Aggregated bit-perfect health snapshot (R5). Always
    /// `Some(...)` when a device is currently open (i.e. a track is
    /// loaded or playing); `None` while idle/stopped. Skipped on the
    /// wire when absent so the IPC payload stays compact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_perfect: Option<BitPerfectHealth>,
    /// Active DSD rate label (e.g. `"DSD64"`) when the engine is
    /// playing a DSD track via WASAPI Exclusive (R7.7). `None` for
    /// PCM playback or while the DSD pipeline is still loading.
    /// Skipped on the wire when absent so PCM `PlayerState`
    /// payloads stay compact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsd_rate_label: Option<String>,
    /// `true` when the engine is currently rendering a DSD stream
    /// (R7.8). Mirrors `Shared::is_dsd_active` so the UI can grey
    /// out volume / EQ / pre-gain controls for the duration of
    /// the track without polling a separate command.
    #[serde(default)]
    pub is_dsd: bool,
    /// Active upmix layout (R9.5). `Some(UpmixInfo { src, dst })`
    /// when no native stereo configuration was available and the
    /// engine had to upmix the source onto a wider device layout;
    /// `None` when the device played the source's channel layout
    /// natively (no upmix). `#[serde(default)]` keeps older IPC
    /// payloads that predate the field deserialising as `None`.
    #[serde(default)]
    pub upmix: Option<UpmixInfo>,
    /// `true` when the current file has exceeded the MP3 frame-repair
    /// threshold (R11.3), i.e. so many `invalid main_data_begin`
    /// frames were repaired that the file is likely degraded. Reset
    /// to `false` on the next `Load`. `#[serde(default)]` keeps older
    /// IPC payloads that predate the field deserialising as `false`.
    #[serde(default)]
    pub degraded: bool,
    /// Last error message, when [`PlayerState::status`] is
    /// [`PlaybackStatus::Errored`].
    pub error: Option<String>,
}

impl Default for PlayerState {
    fn default() -> Self {
        PlayerState {
            status: PlaybackStatus::Idle,
            current_track_id: None,
            position_seconds: 0.0,
            duration_seconds: 0.0,
            volume: 1.0,
            output_mode: EffectiveOutputMode::Shared,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            is_bit_perfect: false,
            rg_attenuation_db: None,
            bit_perfect: None,
            dsd_rate_label: None,
            is_dsd: false,
            upmix: None,
            degraded: false,
            error: None,
        }
    }
}

// -----------------------------------------------------------------------------
// Bit-Perfect Health (R5)
// -----------------------------------------------------------------------------

/// Tri-state status of the audio chain reported to the user. A green
/// badge means the chain is strictly bit-perfect (Exclusive + every
/// stage at unity + native rate + no upmix). Amber is a "Shared but
/// clean" intermediate (no double-resample suspect, every PCM stage
/// bypassed). Red covers everything else, including any active stage
/// or a sample-rate mismatch in Shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitPerfectStatus {
    Green,
    Amber,
    Red,
}

/// Aggregated snapshot of the playback chain published to the UI for
/// the Bit-Perfect Health panel (R5).
///
/// Each `*_off` / `*_bypass` flag is `true` when the corresponding
/// stage is strictly no-op, i.e. it does not contribute any sample
/// modification. The struct is computed by
/// [`BitPerfectHealth::compute`] from a [`PlayerState`] plus the
/// per-stage bypass booleans collected upstream (the caller is
/// responsible for reading them from the `PcmChain` bypass snapshot
/// and from the negotiated output format).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BitPerfectHealth {
    pub status: BitPerfectStatus,

    pub source_sample_rate: Option<u32>,
    pub device_sample_rate: Option<u32>,
    pub source_bit_depth: Option<u8>,
    pub effective_output_mode: EffectiveOutputMode,

    /// `true` when `device_sample_rate == source_sample_rate`. A
    /// mismatch means the OS mixer (Shared) or the device firmware
    /// is doing an extra resample on top of the engine.
    pub is_native_rate: bool,
    pub unity_volume: bool,
    pub unity_pregain: bool,
    pub eq_bypass: bool,
    pub crossfeed_off: bool,
    pub convolver_off: bool,
    pub limiter_off: bool,
    pub dither_bypass: bool,
    pub balance_off: bool,
    /// `true` when the source channel count differs from the device
    /// channel count (the engine had to upmix or downmix).
    pub upmix_active: bool,

    /// Localised human-readable messages that explain *why* the
    /// status is not green. Currently in French, per the panel
    /// surface (`BitPerfectHealthPanel.svelte`).
    pub messages: Vec<String>,
}

impl BitPerfectHealth {
    /// Aggregate the chain state into a single tri-state badge plus
    /// localised messages.
    ///
    /// All per-stage flags are passed in by the caller â€” the function
    /// itself is pure and does no IO. Typical caller (`Player::state`
    /// or the engine `state()` snapshot) reads them from:
    ///   * `state.volume` and the `Pre_Gain_Stage::is_bypass()` for
    ///     `unity_volume` / `unity_pregain` (here folded into
    ///     `state.volume` and the optional `pre_gain_unity` flag).
    ///   * `PcmChain::bypass_snapshot()` for `eq_bypass`,
    ///     `convolver_off`, `dither_bypass`, etc.
    ///
    /// The signature mirrors the contract documented in the design's
    /// Property 9 list.
    #[allow(clippy::too_many_arguments)]
    pub fn compute(
        state: &PlayerState,
        settings: &crate::audio_settings::AudioSettings,
        device_sr: Option<u32>,
        src_ch: u16,
        device_ch: u16,
        eq_bypass: bool,
        convolver_off: bool,
        dither_bypass: bool,
    ) -> Self {
        // ----- Drop source-side fields when no track is loaded -----
        // R5.7: while idle/paused/stopped the panel hides the track
        // block and only renders device fields. The struct still
        // carries device_sample_rate so the UI can show the device
        // SR even with no playback in progress.
        let track_active = matches!(
            state.status,
            PlaybackStatus::Playing | PlaybackStatus::Loading
        );
        let source_sample_rate = if track_active {
            state.sample_rate
        } else {
            None
        };
        let source_bit_depth = if track_active { state.bit_depth } else { None };

        // ----- Per-stage flags (Property 9) -----
        let is_native_rate = match (device_sr, state.sample_rate) {
            (Some(d), Some(s)) => d == s,
            // No information available: treat as native (we cannot
            // claim a mismatch we did not measure).
            _ => true,
        };

        let unity_volume = (state.volume - 1.0).abs() < 1e-4;
        // The pre-gain unity flag is implicit in `rg_attenuation_db`
        // being 0/None *and* the volume slider being at unity. A more
        // direct check could be passed in, but the design records the
        // state purely in terms of volume; we surface it as
        // `unity_volume` and additionally consider the RG pre-gain
        // landed at unity when no attenuation was reported.
        let unity_pregain = unity_volume
            && state
                .rg_attenuation_db
                .map(|db| db.abs() < 1e-4)
                .unwrap_or(true);

        let crossfeed_off = !settings.crossfeed_enabled || src_ch != 2;
        let limiter_off = matches!(
            settings.peak_limiter_mode,
            crate::audio_settings::PeakLimiterMode::Off
        );
        let balance_off = settings.balance.abs() < 1e-9
            && settings.trim_db_per_channel.iter().all(|t| t.abs() < 1e-6);
        let upmix_active = src_ch != 0 && device_ch != 0 && src_ch != device_ch;

        // ----- Tri-state aggregation -----
        let all_stages_clean = unity_volume
            && unity_pregain
            && eq_bypass
            && crossfeed_off
            && convolver_off
            && limiter_off
            && dither_bypass
            && balance_off;

        let status = match state.output_mode {
            EffectiveOutputMode::Exclusive | EffectiveOutputMode::Asio
                if all_stages_clean && is_native_rate && !upmix_active =>
            {
                BitPerfectStatus::Green
            }
            EffectiveOutputMode::Shared if all_stages_clean && is_native_rate => {
                BitPerfectStatus::Amber
            }
            _ => BitPerfectStatus::Red,
        };

        // ----- Localised messages (FR) -----
        let mut messages = Vec::new();

        // R5.3: explicit warning on hidden double-resample in Shared.
        if matches!(state.output_mode, EffectiveOutputMode::Shared) {
            if let (Some(dev), Some(src)) = (device_sr, source_sample_rate) {
                if dev != src {
                    messages.push(format!(
                        "Double rééchantillonnage caché : appareil {} Hz vs source {} Hz",
                        dev, src
                    ));
                }
            }
        }

        if upmix_active {
            messages.push(format!(
                "Upmix actif : source {} canaux vs appareil {} canaux",
                src_ch, device_ch
            ));
        }
        if !eq_bypass {
            messages.push("EQ actif".to_string());
        }
        if !crossfeed_off {
            messages.push("Crossfeed actif".to_string());
        }
        if !convolver_off {
            messages.push("Convolveur actif".to_string());
        }
        if !limiter_off {
            messages.push("Limiteur actif".to_string());
        }
        if !dither_bypass {
            messages.push("Dither actif".to_string());
        }
        if !balance_off {
            messages.push("Balance/trim non unitaire".to_string());
        }
        if !unity_volume {
            messages.push("Volume non unitaire".to_string());
        }

        BitPerfectHealth {
            status,
            source_sample_rate,
            device_sample_rate: device_sr,
            source_bit_depth,
            effective_output_mode: state.output_mode,
            is_native_rate,
            unity_volume,
            unity_pregain,
            eq_bypass,
            crossfeed_off,
            convolver_off,
            limiter_off,
            dither_bypass,
            balance_off,
            upmix_active,
            messages,
        }
    }
}

/// Events emitted by the engine to subscribers (the orchestration layer
/// re-publishes them to the UI as Tauri events).
///
/// Tagged with `serde(tag = "type")` so the UI receives a discriminated
/// union it can pattern-match on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    StateChanged {
        state: PlayerState,
    },
    Position {
        position_seconds: f64,
    },
    EndOfTrack,
    /// Emitted when the decoder thread swaps a prepared next track in
    /// place mid-stream (gapless transition). The audio callback never
    /// stopped: the user heard one continuous output. The orchestrator
    /// advances the queue without issuing a fresh `Load`.
    GaplessTransition,
    /// Aggregated [`BitPerfectHealth`] snapshot pushed when one of the
    /// chain's flags changes (R5). The decoder thread debounces these
    /// to at most one event per 200 ms so the IPC stream stays light
    /// even during slider drags.
    BitPerfectChanged {
        health: BitPerfectHealth,
    },
    /// Convolver IR load failed. Carried up from the worker thread
    /// that decoded the WAV (R9.8). The previously-loaded IR (if
    /// any) stays in place; the UI is expected to show the message
    /// and let the user pick another file.
    IrLoadError {
        message: String,
    },
    /// WASAPI Exclusive refused the DoP carrier format negotiation
    /// (R7.5). Emitted once per failed DSD load attempt; paired
    /// with an `Error` event carrying the user-facing French
    /// message so the UI can surface the failure as a toast.
    DopUnsupported,
    /// User toggled volume / EQ / pre-gain while a DSD track was
    /// playing (R7.8). The engine accepts the requested value
    /// silently for the next PCM track but does not forward it to
    /// the DSD pipeline (which bypasses every PCM stage). The UI
    /// surfaces this as an informational toast.
    DsdReadOnlyDsp,
    /// WASAPI Exclusive failed format negotiation for the current
    /// device and the engine is falling back to the Shared backend
    /// (R9.6). `reason` is a short message describing why Exclusive
    /// was unavailable (e.g. no compatible format). `qobee-core`
    /// republishes this as a [`crate::types::PlayerState`]-adjacent
    /// `PlayerEvent::Error` carrying an FR-localised message so the
    /// UI can surface the switch to Shared as a toast.
    ExclusiveFallback {
        reason: String,
    },
    Error {
        message: String,
    },
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod bit_perfect_tests {
    use super::*;
    use crate::audio_settings::{AudioSettings, PeakLimiterMode};

    /// Build a "clean" PlayerState that, when paired with a default
    /// `AudioSettings` and bypassed PCM stages, would produce a Green
    /// badge in Exclusive mode.
    fn clean_state(mode: EffectiveOutputMode) -> PlayerState {
        PlayerState {
            status: PlaybackStatus::Playing,
            current_track_id: Some("t".into()),
            position_seconds: 1.0,
            duration_seconds: 60.0,
            volume: 1.0,
            output_mode: mode,
            sample_rate: Some(44_100),
            bit_depth: Some(24),
            channels: Some(2),
            is_bit_perfect: matches!(mode, EffectiveOutputMode::Exclusive),
            rg_attenuation_db: None,
            bit_perfect: None,
            dsd_rate_label: None,
            is_dsd: false,
            upmix: None,
            degraded: false,
            error: None,
        }
    }

    /// All PCM stages reported as bypassed by the caller (limiter
    /// bypass is encoded in `settings.peak_limiter_mode = Off`).
    fn clean_settings() -> AudioSettings {
        AudioSettings {
            peak_limiter_mode: PeakLimiterMode::Off,
            crossfeed_enabled: false,
            convolver_enabled: false,
            balance: 0.0,
            trim_db_per_channel: Vec::new(),
            ..AudioSettings::default()
        }
    }

    #[test]
    fn ideal_exclusive_chain_is_green() {
        let state = clean_state(EffectiveOutputMode::Exclusive);
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(
            &state,
            &settings,
            Some(44_100), // device SR matches
            2,            // src_ch
            2,            // device_ch
            true,         // eq_bypass
            true,         // convolver_off
            true,         // dither_bypass (24-bit output)
        );

        assert_eq!(h.status, BitPerfectStatus::Green);
        assert!(h.is_native_rate);
        assert!(h.unity_volume);
        assert!(h.unity_pregain);
        assert!(h.eq_bypass);
        assert!(h.crossfeed_off);
        assert!(h.convolver_off);
        assert!(h.limiter_off);
        assert!(h.dither_bypass);
        assert!(h.balance_off);
        assert!(!h.upmix_active);
        assert!(h.messages.is_empty(), "no warnings, got {:?}", h.messages);
        assert_eq!(h.source_sample_rate, Some(44_100));
        assert_eq!(h.device_sample_rate, Some(44_100));
    }

    #[test]
    fn shared_with_native_rate_and_all_bypass_is_amber() {
        let state = clean_state(EffectiveOutputMode::Shared);
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(&state, &settings, Some(44_100), 2, 2, true, true, true);

        assert_eq!(h.status, BitPerfectStatus::Amber);
        assert!(h.is_native_rate);
        assert!(h.messages.is_empty());
    }

    #[test]
    fn shared_with_sr_mismatch_is_red_and_emits_french_message() {
        let state = clean_state(EffectiveOutputMode::Shared);
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(
            &state,
            &settings,
            Some(48_000), // device SR
            2,
            2,
            true,
            true,
            true,
        );

        assert_eq!(h.status, BitPerfectStatus::Red);
        assert!(!h.is_native_rate);
        assert!(
            h.messages
                .iter()
                .any(|m| m.contains("Double rééchantillonnage caché")
                    && m.contains("48000")
                    && m.contains("44100")),
            "expected a French double-resample warning containing both rates, got {:?}",
            h.messages
        );
    }

    #[test]
    fn exclusive_with_eq_active_is_red() {
        let state = clean_state(EffectiveOutputMode::Exclusive);
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(
            &state,
            &settings,
            Some(44_100),
            2,
            2,
            false, // eq active
            true,
            true,
        );

        assert_eq!(h.status, BitPerfectStatus::Red);
        assert!(h.messages.iter().any(|m| m.contains("EQ actif")));
    }

    #[test]
    fn exclusive_with_limiter_on_is_red() {
        let state = clean_state(EffectiveOutputMode::Exclusive);
        let mut settings = clean_settings();
        settings.peak_limiter_mode = PeakLimiterMode::LookaheadLimiter;

        let h = BitPerfectHealth::compute(&state, &settings, Some(44_100), 2, 2, true, true, true);

        assert_eq!(h.status, BitPerfectStatus::Red);
        assert!(!h.limiter_off);
        assert!(h.messages.iter().any(|m| m.contains("Limiteur actif")));
    }

    #[test]
    fn exclusive_with_upmix_is_red() {
        let state = clean_state(EffectiveOutputMode::Exclusive);
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(
            &state,
            &settings,
            Some(44_100),
            2, // source stereo
            6, // device 5.1 â†’ upmix
            true,
            true,
            true,
        );

        assert_eq!(h.status, BitPerfectStatus::Red);
        assert!(h.upmix_active);
        assert!(h
            .messages
            .iter()
            .any(|m| m.contains("Upmix actif") && m.contains('2') && m.contains('6')));
    }

    #[test]
    fn exclusive_with_non_unity_volume_is_red() {
        let mut state = clean_state(EffectiveOutputMode::Exclusive);
        state.volume = 0.5;
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(&state, &settings, Some(44_100), 2, 2, true, true, true);

        assert_eq!(h.status, BitPerfectStatus::Red);
        assert!(!h.unity_volume);
    }

    #[test]
    fn exclusive_with_balance_active_is_red() {
        let state = clean_state(EffectiveOutputMode::Exclusive);
        let mut settings = clean_settings();
        settings.balance = -0.25;

        let h = BitPerfectHealth::compute(&state, &settings, Some(44_100), 2, 2, true, true, true);

        assert_eq!(h.status, BitPerfectStatus::Red);
        assert!(!h.balance_off);
        assert!(h.messages.iter().any(|m| m.contains("Balance/trim")));
    }

    #[test]
    fn idle_status_drops_source_fields() {
        let mut state = clean_state(EffectiveOutputMode::Shared);
        state.status = PlaybackStatus::Stopped;
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(&state, &settings, Some(48_000), 2, 2, true, true, true);

        // Source SR/depth dropped (R5.7).
        assert!(h.source_sample_rate.is_none());
        assert!(h.source_bit_depth.is_none());
        // Device SR is still reported.
        assert_eq!(h.device_sample_rate, Some(48_000));
        // No double-resample message because source SR is None now.
        assert!(
            h.messages
                .iter()
                .all(|m| !m.contains("Double rééchantillonnage")),
            "messages={:?}",
            h.messages
        );
    }

    #[test]
    fn dither_active_on_16_bit_output_is_red() {
        let state = clean_state(EffectiveOutputMode::Exclusive);
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(
            &state,
            &settings,
            Some(44_100),
            2,
            2,
            true,
            true,
            false, // dither stage active (16-bit out)
        );

        assert_eq!(h.status, BitPerfectStatus::Red);
        assert!(!h.dither_bypass);
        assert!(h.messages.iter().any(|m| m.contains("Dither actif")));
    }

    #[test]
    fn json_round_trip_preserves_all_fields() {
        let state = clean_state(EffectiveOutputMode::Exclusive);
        let settings = clean_settings();

        let h = BitPerfectHealth::compute(&state, &settings, Some(44_100), 2, 2, true, true, true);

        let json = serde_json::to_string(&h).unwrap();
        let parsed: BitPerfectHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(h, parsed);

        // Status is serialised in snake_case (UI contract).
        assert!(
            json.contains("\"status\":\"green\""),
            "status should serialise as snake_case, got json={json}"
        );
    }
}
