//! Persistent store for the `audio.*` settings.
//!
//! The store is the single source of truth between the SQLite
//! `settings` table (managed by [`qobee_library::Library`]) and the
//! engine's lock-free [`AudioSettings`] snapshot. The flow is always:
//!
//! 1. The UI calls a Tauri command (`get_audio_setting` /
//!    `set_audio_setting` — wired in task 5).
//! 2. The command delegates to [`AudioSettingsStore`].
//! 3. The store validates type + range, persists the JSON value to
//!    the `settings` table, mutates its in-memory snapshot, and
//!    pushes the new snapshot through the [`AudioSettingsApply`]
//!    handle (the engine-side adapter).
//!
//! Every mutation goes through the store so we can guarantee the
//! atomicity property listed in the spec (Property 29): a `set` that
//! fails (invalid type, out of range, DB error) leaves the in-memory
//! snapshot and the engine state strictly unchanged.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use qobee_engine::{
    AudioSettings, CrossfeedPreset, DitherProfile, PeakLimiterMode, ResamplerQuality,
    VolumeCurve,
};
use qobee_library::Library;
use serde_json::{json, Value};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Trait used by the store to push snapshots into the engine.
// ---------------------------------------------------------------------------

/// Applies an [`AudioSettings`] snapshot to a live engine. Implemented
/// on [`crate::Player`] (see `player.rs`) so the store can stay
/// engine-agnostic and unit-testable with a mock implementation.
pub trait AudioSettingsApply: Send + Sync {
    fn apply(&self, settings: AudioSettings);
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure modes raised by [`AudioSettingsStore`].
///
/// `TypeMismatch` includes a static description of the expected
/// shape (e.g. `"f32 in [0.0, 3.0]"` or `"one of: tpdf,
/// shaped_hp, shaped_f_weighted"`) so the UI layer can format a
/// helpful toast without re-implementing the type table.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum AudioSettingsError {
    #[error("unknown audio setting key: {0}")]
    UnknownKey(String),
    #[error("type mismatch for `{key}`: expected {expected}")]
    TypeMismatch {
        key: String,
        expected: &'static str,
    },
    #[error("value for `{key}` out of range: {reason}")]
    OutOfRange { key: String, reason: String },
    #[error("database error: {0}")]
    DbError(String),
}

// ---------------------------------------------------------------------------
// Settings keys
// ---------------------------------------------------------------------------

/// Every persisted key, in canonical order. Used by `load_or_init` and
/// `write_defaults` to iterate without forgetting one.
pub const ALL_KEYS: &[&str] = &[
    "audio.rg_peak_protection",
    "audio.rg_safety_headroom_db",
    "audio.dither_profile",
    "audio.peak_limiter_mode",
    "audio.peak_limiter_ceiling_dbfs",
    "audio.peak_limiter_lookahead_ms",
    "audio.peak_limiter_release_ms",
    "audio.volume_curve",
    "audio.volume_floor_db",
    "audio.crossfeed_enabled",
    "audio.crossfeed_preset",
    "audio.crossfeed_delay_us",
    "audio.crossfeed_lp_cutoff_hz",
    "audio.resampler_quality",
    "audio.convolver_enabled",
    "audio.convolver_ir_path",
    "audio.convolver_gain_db",
    "audio.balance",
    "audio.trim_db_per_channel",
];

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Persistent, validated bridge between the `settings` SQLite table
/// and the engine's [`AudioSettings`] snapshot.
pub struct AudioSettingsStore {
    library: Library,
    current: Mutex<AudioSettings>,
    engine: Arc<dyn AudioSettingsApply>,
}

impl AudioSettingsStore {
    /// Create a new store. The in-memory snapshot starts at
    /// [`AudioSettings::default`]; the caller is expected to invoke
    /// [`AudioSettingsStore::load_or_init`] before serving the first
    /// request so the persisted values take effect.
    pub fn new(library: Library, engine: Arc<dyn AudioSettingsApply>) -> Self {
        Self {
            library,
            current: Mutex::new(AudioSettings::default()),
            engine,
        }
    }

    /// Read every `audio.*` key, validate it (type + range), and
    /// rewrite the corrected value to the table when needed.
    ///
    /// Behaviour per key:
    ///   * Missing -> write the default to the table.
    ///   * Present but invalid JSON / wrong type -> overwrite with
    ///     the default.
    ///   * Present and valid but out of range -> clamp and rewrite.
    ///
    /// Returns the resulting snapshot. The new snapshot is also
    /// pushed to the engine via [`AudioSettingsApply::apply`].
    pub fn load_or_init(&self) -> AudioSettings {
        let mut s = AudioSettings::default();

        // For each key, read from SQLite and either update `s` from
        // the persisted value or fall through to the default already
        // sitting on `s`. `dirty` tracks whether any key needs to be
        // rewritten so we can persist the corrected payload back.
        for key in ALL_KEYS {
            let raw = self
                .library
                .get_setting(key)
                .ok()
                .flatten();
            match raw {
                None => {
                    // Missing: persist the default.
                    let default_value = read_field(&s, key)
                        .expect("ALL_KEYS only contains known keys");
                    let _ = self.write_raw(key, &default_value);
                }
                Some(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(value) => {
                        if apply_field(&mut s, key, &value).is_err() {
                            // Wrong type or unparseable variant: revert
                            // to the default and rewrite.
                            let default_value = read_field(&s, key)
                                .expect("ALL_KEYS only contains known keys");
                            let _ = self.write_raw(key, &default_value);
                        }
                    }
                    Err(_) => {
                        // Garbage in the column: rewrite the default.
                        let default_value = read_field(&s, key)
                            .expect("ALL_KEYS only contains known keys");
                        let _ = self.write_raw(key, &default_value);
                    }
                },
            }
        }

        // Bring numeric fields back into the canonical bounds. The
        // store rewrites every clamped field unconditionally because
        // detecting per-field changes would require a full diff.
        let before = s.clone();
        s.clamp_in_place();
        if !audio_numeric_eq(&before, &s) {
            for key in ALL_KEYS {
                if let Some(v) = read_field(&s, key) {
                    let _ = self.write_raw(key, &v);
                }
            }
        }

        // Publish.
        *self.current.lock() = s.clone();
        self.engine.apply(s.clone());
        s
    }

    /// Read a single setting from the in-memory snapshot.
    pub fn get(&self, key: &str) -> Option<Value> {
        let snap = self.current.lock();
        read_field(&snap, key)
    }

    /// Atomic snapshot of the published settings.
    pub fn snapshot(&self) -> AudioSettings {
        self.current.lock().clone()
    }

    /// Validate, persist, update the in-memory snapshot, and push
    /// the new snapshot to the engine. Atomic: a failure leaves
    /// `current` and the engine state strictly unchanged (Property
    /// 29).
    pub fn set(&self, key: &str, value: Value) -> Result<(), AudioSettingsError> {
        if !ALL_KEYS.contains(&key) {
            return Err(AudioSettingsError::UnknownKey(key.to_string()));
        }

        let mut next = self.current.lock().clone();
        apply_field(&mut next, key, &value)?;
        // Validate range before touching the DB; clamp would silently
        // accept whatever the caller sent, which is the wrong
        // contract for `set` (R12.6).
        validate_field(&next, key)?;

        // Persist first. If SQLite rejects the row we leave the
        // in-memory snapshot intact.
        let canonical = read_field(&next, key)
            .expect("apply_field updated a known key");
        self.write_raw(key, &canonical)
            .map_err(|e| AudioSettingsError::DbError(e.to_string()))?;

        *self.current.lock() = next.clone();
        self.engine.apply(next);
        Ok(())
    }

    /// Reset every persisted key to its default value and push the
    /// fresh snapshot to the engine. Used by the
    /// `reset_audio_settings` Tauri command (task 5).
    pub fn write_defaults(&self) -> Result<(), AudioSettingsError> {
        let defaults = AudioSettings::default();
        for key in ALL_KEYS {
            let v = read_field(&defaults, key)
                .expect("ALL_KEYS only contains known keys");
            self.write_raw(key, &v)
                .map_err(|e| AudioSettingsError::DbError(e.to_string()))?;
        }
        *self.current.lock() = defaults.clone();
        self.engine.apply(defaults);
        Ok(())
    }

    fn write_raw(&self, key: &str, value: &Value) -> Result<(), qobee_library::LibraryError> {
        let s = serde_json::to_string(value).expect("serde_json never fails on Value");
        self.library.set_setting(key, &s)
    }
}

// ---------------------------------------------------------------------------
// Field <-> JSON helpers
// ---------------------------------------------------------------------------

/// Read a single field from `settings` and return its JSON
/// representation. Returns `None` for unknown keys.
fn read_field(s: &AudioSettings, key: &str) -> Option<Value> {
    Some(match key {
        "audio.rg_peak_protection" => Value::Bool(s.rg_peak_protection),
        "audio.rg_safety_headroom_db" => json!(s.rg_safety_headroom_db),
        "audio.dither_profile" => serde_json::to_value(s.dither_profile).ok()?,
        "audio.peak_limiter_mode" => serde_json::to_value(s.peak_limiter_mode).ok()?,
        "audio.peak_limiter_ceiling_dbfs" => json!(s.peak_limiter_ceiling_dbfs),
        "audio.peak_limiter_lookahead_ms" => json!(s.peak_limiter_lookahead_ms),
        "audio.peak_limiter_release_ms" => json!(s.peak_limiter_release_ms),
        "audio.volume_curve" => serde_json::to_value(s.volume_curve).ok()?,
        "audio.volume_floor_db" => json!(s.volume_floor_db),
        "audio.crossfeed_enabled" => Value::Bool(s.crossfeed_enabled),
        "audio.crossfeed_preset" => serde_json::to_value(s.crossfeed_preset).ok()?,
        "audio.crossfeed_delay_us" => json!(s.crossfeed_delay_us),
        "audio.crossfeed_lp_cutoff_hz" => json!(s.crossfeed_lp_cutoff_hz),
        "audio.resampler_quality" => serde_json::to_value(s.resampler_quality).ok()?,
        "audio.convolver_enabled" => Value::Bool(s.convolver_enabled),
        "audio.convolver_ir_path" => match &s.convolver_ir_path {
            None => Value::Null,
            Some(p) => Value::String(p.to_string_lossy().into_owned()),
        },
        "audio.convolver_gain_db" => json!(s.convolver_gain_db),
        "audio.balance" => json!(s.balance),
        "audio.trim_db_per_channel" => json!(s.trim_db_per_channel),
        _ => return None,
    })
}

/// Validate the JSON value's *type* and apply it to the matching
/// field. Range checks live in [`validate_field`] so `load_or_init`
/// can re-clamp without rejecting persisted-but-stale values.
fn apply_field(
    s: &mut AudioSettings,
    key: &str,
    value: &Value,
) -> Result<(), AudioSettingsError> {
    let mismatch = |expected: &'static str| AudioSettingsError::TypeMismatch {
        key: key.to_string(),
        expected,
    };

    match key {
        "audio.rg_peak_protection" => {
            s.rg_peak_protection = value.as_bool().ok_or_else(|| mismatch("bool"))?;
        }
        "audio.rg_safety_headroom_db" => {
            s.rg_safety_headroom_db = parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.dither_profile" => {
            s.dither_profile = serde_json::from_value::<DitherProfile>(value.clone())
                .map_err(|_| mismatch("\"tpdf\" | \"shaped_hp\" | \"shaped_f_weighted\""))?;
        }
        "audio.peak_limiter_mode" => {
            s.peak_limiter_mode = serde_json::from_value::<PeakLimiterMode>(value.clone())
                .map_err(|_| mismatch("\"off\" | \"soft_clip\" | \"lookahead_limiter\""))?;
        }
        "audio.peak_limiter_ceiling_dbfs" => {
            s.peak_limiter_ceiling_dbfs =
                parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.peak_limiter_lookahead_ms" => {
            s.peak_limiter_lookahead_ms =
                parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.peak_limiter_release_ms" => {
            s.peak_limiter_release_ms =
                parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.volume_curve" => {
            s.volume_curve = serde_json::from_value::<VolumeCurve>(value.clone())
                .map_err(|_| mismatch("\"quadratic\" | \"logarithmic\""))?;
        }
        "audio.volume_floor_db" => {
            s.volume_floor_db = parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.crossfeed_enabled" => {
            s.crossfeed_enabled = value.as_bool().ok_or_else(|| mismatch("bool"))?;
        }
        "audio.crossfeed_preset" => {
            s.crossfeed_preset = serde_json::from_value::<CrossfeedPreset>(value.clone())
                .map_err(|_| mismatch("\"bauer\" | \"bauer_strong\" | \"custom\""))?;
        }
        "audio.crossfeed_delay_us" => {
            s.crossfeed_delay_us = parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.crossfeed_lp_cutoff_hz" => {
            s.crossfeed_lp_cutoff_hz = parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.resampler_quality" => {
            s.resampler_quality = serde_json::from_value::<ResamplerQuality>(value.clone())
                .map_err(|_| mismatch("\"standard\" | \"best\""))?;
        }
        "audio.convolver_enabled" => {
            s.convolver_enabled = value.as_bool().ok_or_else(|| mismatch("bool"))?;
        }
        "audio.convolver_ir_path" => {
            s.convolver_ir_path = match value {
                Value::Null => None,
                Value::String(p) if p.is_empty() => None,
                Value::String(p) => Some(PathBuf::from(p)),
                _ => return Err(mismatch("string | null")),
            };
        }
        "audio.convolver_gain_db" => {
            s.convolver_gain_db = parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.balance" => {
            s.balance = parse_finite_f32(value).ok_or_else(|| mismatch("f32"))?;
        }
        "audio.trim_db_per_channel" => {
            let arr = value
                .as_array()
                .ok_or_else(|| mismatch("array of f32"))?;
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                out.push(parse_finite_f32(v).ok_or_else(|| mismatch("array of f32"))?);
            }
            s.trim_db_per_channel = out;
        }
        _ => {
            return Err(AudioSettingsError::UnknownKey(key.to_string()));
        }
    }

    Ok(())
}

/// Validate the per-field bounds defined by the settings table.
///
/// Called by `set` after `apply_field`: type errors are reported
/// before this function ever runs, so we only deal with range
/// problems here. `load_or_init` skips this check and relies on
/// `clamp_in_place` instead, which is the correct behaviour for
/// recovering from older builds that may have written wider ranges.
fn validate_field(s: &AudioSettings, key: &str) -> Result<(), AudioSettingsError> {
    let oor = |reason: String| AudioSettingsError::OutOfRange {
        key: key.to_string(),
        reason,
    };

    match key {
        "audio.rg_peak_protection"
        | "audio.dither_profile"
        | "audio.peak_limiter_mode"
        | "audio.volume_curve"
        | "audio.crossfeed_enabled"
        | "audio.crossfeed_preset"
        | "audio.resampler_quality"
        | "audio.convolver_enabled"
        | "audio.convolver_ir_path" => {} // type-only checks
        "audio.rg_safety_headroom_db" => {
            range(s.rg_safety_headroom_db, 0.0, 3.0, "[0.0, 3.0]").map_err(oor)?
        }
        "audio.peak_limiter_ceiling_dbfs" => {
            range(s.peak_limiter_ceiling_dbfs, -3.0, 0.0, "[-3.0, 0.0]").map_err(oor)?
        }
        "audio.peak_limiter_lookahead_ms" => {
            range(s.peak_limiter_lookahead_ms, 2.0, 10.0, "[2.0, 10.0]").map_err(oor)?
        }
        "audio.peak_limiter_release_ms" => {
            range(s.peak_limiter_release_ms, 20.0, 500.0, "[20.0, 500.0]").map_err(oor)?
        }
        "audio.volume_floor_db" => {
            range(s.volume_floor_db, -80.0, -30.0, "[-80.0, -30.0]").map_err(oor)?
        }
        "audio.crossfeed_delay_us" => {
            range(s.crossfeed_delay_us, 200.0, 400.0, "[200.0, 400.0]").map_err(oor)?
        }
        "audio.crossfeed_lp_cutoff_hz" => {
            range(s.crossfeed_lp_cutoff_hz, 500.0, 1500.0, "[500.0, 1500.0]").map_err(oor)?
        }
        "audio.convolver_gain_db" => {
            range(s.convolver_gain_db, -24.0, 0.0, "[-24.0, 0.0]").map_err(oor)?
        }
        "audio.balance" => range(s.balance, -1.0, 1.0, "[-1.0, 1.0]").map_err(oor)?,
        "audio.trim_db_per_channel" => {
            if s.trim_db_per_channel.len() > AudioSettings::MAX_CHANNELS {
                return Err(AudioSettingsError::OutOfRange {
                    key: key.to_string(),
                    reason: format!(
                        "length {} exceeds max channels {}",
                        s.trim_db_per_channel.len(),
                        AudioSettings::MAX_CHANNELS
                    ),
                });
            }
            for (i, t) in s.trim_db_per_channel.iter().enumerate() {
                range(*t, -12.0, 0.0, "[-12.0, 0.0]")
                    .map_err(|reason| AudioSettingsError::OutOfRange {
                        key: key.to_string(),
                        reason: format!("index {}: {}", i, reason),
                    })?;
            }
        }
        _ => return Err(AudioSettingsError::UnknownKey(key.to_string())),
    }

    Ok(())
}

fn range(v: f32, min: f32, max: f32, label: &str) -> Result<(), String> {
    if v.is_nan() || v.is_infinite() {
        return Err(format!("value {} not finite (expected {})", v, label));
    }
    if v < min || v > max {
        return Err(format!("{} not in {}", v, label));
    }
    Ok(())
}

fn parse_finite_f32(value: &Value) -> Option<f32> {
    let n = value.as_f64()?;
    if !n.is_finite() {
        return None;
    }
    Some(n as f32)
}

/// Compare every numeric field of two `AudioSettings` snapshots; used
/// by `load_or_init` to detect whether `clamp_in_place` mutated
/// anything and we therefore need to rewrite the persisted values.
fn audio_numeric_eq(a: &AudioSettings, b: &AudioSettings) -> bool {
    a.rg_safety_headroom_db == b.rg_safety_headroom_db
        && a.peak_limiter_ceiling_dbfs == b.peak_limiter_ceiling_dbfs
        && a.peak_limiter_lookahead_ms == b.peak_limiter_lookahead_ms
        && a.peak_limiter_release_ms == b.peak_limiter_release_ms
        && a.volume_floor_db == b.volume_floor_db
        && a.crossfeed_delay_us == b.crossfeed_delay_us
        && a.crossfeed_lp_cutoff_hz == b.crossfeed_lp_cutoff_hz
        && a.convolver_gain_db == b.convolver_gain_db
        && a.balance == b.balance
        && a.trim_db_per_channel == b.trim_db_per_channel
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mock engine adapter that captures the latest applied snapshot.
    struct MockApply {
        last: Mutex<Option<AudioSettings>>,
    }

    impl MockApply {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last: Mutex::new(None),
            })
        }
        fn last(&self) -> Option<AudioSettings> {
            self.last.lock().clone()
        }
    }

    impl AudioSettingsApply for MockApply {
        fn apply(&self, settings: AudioSettings) {
            *self.last.lock() = Some(settings);
        }
    }

    fn fresh_store() -> (AudioSettingsStore, Arc<MockApply>) {
        // Use the default `Library` open path with a unique temp dir
        // so each test gets a fresh in-process database. We avoid
        // adding `tempfile` as a new dependency by carving out a
        // per-test directory under the system tmp.
        let mut base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // The randomness here doesn't need to be cryptographic; we
        // just want unique paths even when several tests run in
        // parallel inside the same nanosecond bucket.
        let tag = format!(
            "qobee-audio-settings-{}-{:?}",
            nanos,
            std::thread::current().id()
        );
        base.push(tag);

        let db_path = base.join("library.sqlite3");
        let covers = base.join("covers");
        let library = Library::open(&db_path, &covers).expect("open library");
        let mock = MockApply::new();
        let engine: Arc<dyn AudioSettingsApply> = mock.clone();
        let store = AudioSettingsStore::new(library, engine);
        (store, mock)
    }

    #[test]
    fn load_or_init_writes_defaults_on_empty_db() {
        let (store, mock) = fresh_store();

        let snap = store.load_or_init();

        assert_eq!(snap, AudioSettings::default());

        // Every key is now in the table.
        for key in ALL_KEYS {
            let raw = store.library.get_setting(key).expect("read setting");
            assert!(raw.is_some(), "key {} should have been written", key);
        }

        // Engine received the snapshot.
        let pushed = mock.last().expect("apply was called");
        assert_eq!(pushed, AudioSettings::default());
    }

    #[test]
    fn load_or_init_clamps_out_of_range_values() {
        let (store, _) = fresh_store();

        store
            .library
            .set_setting("audio.rg_safety_headroom_db", "999.0")
            .expect("seed");

        let snap = store.load_or_init();

        assert_eq!(snap.rg_safety_headroom_db, 3.0);

        // The corrected value is back in the DB.
        let raw = store
            .library
            .get_setting("audio.rg_safety_headroom_db")
            .expect("read")
            .expect("present");
        let v: f32 = serde_json::from_str(&raw).expect("number");
        assert_eq!(v, 3.0);
    }

    #[test]
    fn load_or_init_replaces_invalid_json_with_default() {
        let (store, _) = fresh_store();

        store
            .library
            .set_setting("audio.dither_profile", "\"garbage\"")
            .expect("seed");

        let snap = store.load_or_init();

        assert_eq!(snap.dither_profile, DitherProfile::ShapedFWeighted);

        let raw = store
            .library
            .get_setting("audio.dither_profile")
            .expect("read")
            .expect("present");
        assert_eq!(raw, "\"shaped_f_weighted\"");
    }

    #[test]
    fn set_with_valid_value_round_trips() {
        let (store, mock) = fresh_store();
        store.load_or_init();

        store
            .set("audio.rg_safety_headroom_db", json!(2.0))
            .expect("set");

        assert_eq!(
            store.get("audio.rg_safety_headroom_db"),
            Some(json!(2.0))
        );

        let pushed = mock.last().expect("apply called at least once");
        assert_eq!(pushed.rg_safety_headroom_db, 2.0);
    }

    #[test]
    fn set_with_invalid_type_returns_error_and_does_not_mutate() {
        let (store, mock) = fresh_store();
        store.load_or_init();
        let before = store.snapshot();
        let pushed_before = mock.last();

        let err = store
            .set("audio.rg_safety_headroom_db", json!("nope"))
            .expect_err("must reject string");
        assert!(matches!(err, AudioSettingsError::TypeMismatch { .. }));

        assert_eq!(store.snapshot(), before);
        // No additional `apply` happened: the snapshot received last
        // is the same one published by `load_or_init`.
        assert_eq!(mock.last(), pushed_before);
    }

    #[test]
    fn set_with_out_of_range_returns_error_and_does_not_mutate() {
        let (store, mock) = fresh_store();
        store.load_or_init();
        let before = store.snapshot();
        let pushed_before = mock.last();

        let err = store
            .set("audio.balance", json!(99.0))
            .expect_err("must reject 99.0");
        assert!(matches!(err, AudioSettingsError::OutOfRange { .. }));

        assert_eq!(store.snapshot(), before);
        assert_eq!(mock.last(), pushed_before);
    }

    #[test]
    fn set_unknown_key_returns_error() {
        let (store, _) = fresh_store();
        store.load_or_init();

        let err = store
            .set("audio.bogus", json!(1.0))
            .expect_err("must reject unknown key");
        assert!(matches!(err, AudioSettingsError::UnknownKey(_)));
    }
}
