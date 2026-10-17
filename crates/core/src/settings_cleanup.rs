//! One-time cleanup of obsolete `audio.*` settings rows.
//!
//! The audiophile feature surface (EQ, ReplayGain, crossfeed, convolver,
//! dither, limiter, balance/trim, resampler quality, output modes,
//! volume curve/floor) was removed when the engine moved to `rodio`.
//! The SQLite `settings` table can still hold rows from a previous
//! build; this module deletes the obsolete keys on startup so stale
//! preferences neither linger nor confuse the simplified player.
//!
//! The cleanup is idempotent: deleting a missing key is a no-op that
//! returns `Ok`, so running it twice (or on an already-clean database)
//! changes nothing and never errors. Non-audio keys (UI, playback
//! device, integrations, session) are never named here and are
//! therefore preserved.

use qobee_library::{Library, LibraryError};

/// Every obsolete `audio.*` settings key removed with the audiophile
/// stack. The output-device selection lives under
/// `playback.output_device_id` (not `audio.*`) and is preserved.
pub const OBSOLETE_AUDIO_KEYS: &[&str] = &[
    // ReplayGain (R1)
    "audio.replaygain_mode",
    "audio.rg_peak_protection",
    "audio.rg_safety_headroom_db",
    // Dither (R2)
    "audio.dither_profile",
    // Peak limiter (R3)
    "audio.peak_limiter_mode",
    "audio.peak_limiter_ceiling_dbfs",
    "audio.peak_limiter_lookahead_ms",
    "audio.peak_limiter_release_ms",
    // Volume curve (R4)
    "audio.volume_curve",
    "audio.volume_floor_db",
    // Crossfeed (R6)
    "audio.crossfeed_enabled",
    "audio.crossfeed_preset",
    "audio.crossfeed_delay_us",
    "audio.crossfeed_lp_cutoff_hz",
    // Resampler quality (R8)
    "audio.resampler_quality",
    // Convolver (R9)
    "audio.convolver_enabled",
    "audio.convolver_ir_path",
    "audio.convolver_gain_db",
    // Balance / trim (R10)
    "audio.balance",
    "audio.trim_db_per_channel",
    // EQ + output mode
    "audio.eq_gains",
    "audio.eq_enabled",
    "audio.output_mode",
];

/// Remove every obsolete `audio.*` key from the settings table.
/// Idempotent and tolerant of absent keys (Requirements 8.1, 8.4, 8.5).
pub fn cleanup_obsolete_audio_settings(library: &Library) -> Result<(), LibraryError> {
    for key in OBSOLETE_AUDIO_KEYS {
        library.delete_setting(key)?;
    }
    Ok(())
}
