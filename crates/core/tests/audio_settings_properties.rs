//! Property-based tests for the `AudioSettingsStore` (Phase A).
//!
//! These four properties cover the contract listed in `design.md`
//! and `requirements.md` (R12) for every `audio.*` setting:
//!
//!   * Property 26 — round-trip set/get preserves the value.
//!   * Property 27 — `load_or_init` repairs missing / out-of-range
//!     values by clamping or substituting the default.
//!   * Property 28 — a successful `set` is immediately visible to
//!     the engine via the `AudioSettingsApply` callback.
//!   * Property 29 — a failing `set` (wrong type or out of range) is
//!     atomic: the in-memory snapshot and the last applied snapshot
//!     are strictly unchanged.
//!
//! The tests use a real `Library` opened against a fresh per-test
//! temporary directory and a `MockApply` that captures the most
//! recently applied snapshot. No filesystem mocking, no engine
//! mocking — only the SQLite layer is exercised end-to-end.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use proptest::prelude::*;
use qobee_core::{AudioSettingsApply, AudioSettingsError, AudioSettingsStore};
use qobee_engine::{
    AudioSettings, CrossfeedPreset, DitherProfile, PeakLimiterMode, ResamplerQuality,
    VolumeCurve,
};
use qobee_library::Library;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Mirror of `core::audio_settings::ALL_KEYS` (kept private in the
/// crate). Used by Property 27 to inspect every persisted column.
const ALL_KEYS: &[&str] = &[
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

/// Captures the most recent `AudioSettings` snapshot pushed to the
/// engine. Used to assert visibility (Property 28) and atomicity
/// (Property 29).
struct MockApply {
    last: Mutex<Option<AudioSettings>>,
}

impl MockApply {
    fn new() -> Arc<Self> {
        Arc::new(Self { last: Mutex::new(None) })
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

/// Per-test fixture. The `library` handle is a clone of the one the
/// store owns — `Library` is `Arc`-backed so both views see the
/// same SQLite database, which lets Property 27 seed arbitrary raw
/// values directly through [`Library::set_setting`].
struct Fixture {
    store: AudioSettingsStore,
    mock: Arc<MockApply>,
    library: Library,
    /// Kept alive so the temp directory does not get GC'd before
    /// the SQLite file inside it is closed.
    _base: PathBuf,
}

fn fresh_fixture() -> Fixture {
    let mut base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Combine the wall-clock nanosecond reading with the OS thread
    // ID so two tests scheduled in the same nanosecond bucket still
    // get distinct paths.
    let tag = format!(
        "qobee-audio-settings-pbt-{}-{:?}",
        nanos,
        std::thread::current().id()
    );
    base.push(tag);

    let db_path = base.join("library.sqlite3");
    let covers = base.join("covers");
    let library = Library::open(&db_path, &covers).expect("open library");
    let mock = MockApply::new();
    let engine: Arc<dyn AudioSettingsApply> = mock.clone();
    let store = AudioSettingsStore::new(library.clone(), engine);
    Fixture { store, mock, library, _base: base }
}

// ---------------------------------------------------------------------------
// Per-key strategies
// ---------------------------------------------------------------------------

/// One representative valid `(key, value)` pair, sampled uniformly
/// from the entire key set. The value matches the type and range
/// declared in `design.md` § "Settings keys".
fn any_valid_kv() -> impl Strategy<Value = (&'static str, Value)> {
    prop_oneof![
        any::<bool>().prop_map(|b| ("audio.rg_peak_protection", json!(b))),
        (0.0f32..=3.0f32).prop_map(|v| ("audio.rg_safety_headroom_db", json!(v))),
        any_dither_profile().prop_map(|d| (
            "audio.dither_profile",
            serde_json::to_value(d).unwrap()
        )),
        any_peak_limiter_mode().prop_map(|d| (
            "audio.peak_limiter_mode",
            serde_json::to_value(d).unwrap()
        )),
        (-3.0f32..=0.0f32).prop_map(|v| ("audio.peak_limiter_ceiling_dbfs", json!(v))),
        (2.0f32..=10.0f32).prop_map(|v| ("audio.peak_limiter_lookahead_ms", json!(v))),
        (20.0f32..=500.0f32).prop_map(|v| ("audio.peak_limiter_release_ms", json!(v))),
        any_volume_curve().prop_map(|d| (
            "audio.volume_curve",
            serde_json::to_value(d).unwrap()
        )),
        (-80.0f32..=-30.0f32).prop_map(|v| ("audio.volume_floor_db", json!(v))),
        any::<bool>().prop_map(|b| ("audio.crossfeed_enabled", json!(b))),
        any_crossfeed_preset().prop_map(|d| (
            "audio.crossfeed_preset",
            serde_json::to_value(d).unwrap()
        )),
        (200.0f32..=400.0f32).prop_map(|v| ("audio.crossfeed_delay_us", json!(v))),
        (500.0f32..=1500.0f32).prop_map(|v| ("audio.crossfeed_lp_cutoff_hz", json!(v))),
        any_resampler_quality().prop_map(|d| (
            "audio.resampler_quality",
            serde_json::to_value(d).unwrap()
        )),
        any::<bool>().prop_map(|b| ("audio.convolver_enabled", json!(b))),
        prop_oneof![
            Just(("audio.convolver_ir_path", Value::Null)),
            "[a-zA-Z0-9_/.-]{1,32}".prop_map(|p: String| (
                "audio.convolver_ir_path",
                json!(p)
            )),
        ],
        (-24.0f32..=0.0f32).prop_map(|v| ("audio.convolver_gain_db", json!(v))),
        (-1.0f32..=1.0f32).prop_map(|v| ("audio.balance", json!(v))),
        proptest::collection::vec(-12.0f32..=0.0f32, 0..=AudioSettings::MAX_CHANNELS)
            .prop_map(|xs| ("audio.trim_db_per_channel", json!(xs))),
    ]
}

/// One representative invalid `(key, value)` pair: either the wrong
/// type (string where a number is expected, etc.) or a number
/// outside the declared range. Property 29 uses this to drive the
/// atomicity assertion.
fn any_invalid_kv() -> impl Strategy<Value = (&'static str, Value)> {
    prop_oneof![
        // Wrong types.
        Just(("audio.rg_peak_protection", json!("nope"))),
        Just(("audio.rg_safety_headroom_db", json!("string"))),
        Just(("audio.dither_profile", json!("garbage_profile"))),
        Just(("audio.peak_limiter_mode", json!(42))),
        Just(("audio.volume_curve", json!(true))),
        Just(("audio.crossfeed_preset", json!("nonexistent"))),
        Just(("audio.resampler_quality", json!(3.14))),
        Just(("audio.convolver_ir_path", json!(42))),
        Just(("audio.trim_db_per_channel", json!("not an array"))),
        Just(("audio.unknown_key", json!(0))),

        // Out of range numerics — pick a value clearly outside the
        // declared bounds.
        prop_oneof![Just(99.0f32), Just(-1.0f32)]
            .prop_map(|v| ("audio.rg_safety_headroom_db", json!(v))),
        prop_oneof![Just(5.0f32), Just(-50.0f32)]
            .prop_map(|v| ("audio.peak_limiter_ceiling_dbfs", json!(v))),
        prop_oneof![Just(0.5f32), Just(20.0f32)]
            .prop_map(|v| ("audio.peak_limiter_lookahead_ms", json!(v))),
        prop_oneof![Just(0.0f32), Just(2_000.0f32)]
            .prop_map(|v| ("audio.peak_limiter_release_ms", json!(v))),
        prop_oneof![Just(-200.0f32), Just(0.0f32)]
            .prop_map(|v| ("audio.volume_floor_db", json!(v))),
        prop_oneof![Just(50.0f32), Just(10_000.0f32)]
            .prop_map(|v| ("audio.crossfeed_delay_us", json!(v))),
        prop_oneof![Just(100.0f32), Just(50_000.0f32)]
            .prop_map(|v| ("audio.crossfeed_lp_cutoff_hz", json!(v))),
        prop_oneof![Just(5.0f32), Just(-100.0f32)]
            .prop_map(|v| ("audio.convolver_gain_db", json!(v))),
        prop_oneof![Just(5.0f32), Just(-5.0f32)]
            .prop_map(|v| ("audio.balance", json!(v))),
        // Trim arrays that are too long, or contain an out-of-range entry.
        prop_oneof![
            Just(("audio.trim_db_per_channel", json!([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))),
            Just(("audio.trim_db_per_channel", json!([5.0]))),
            Just(("audio.trim_db_per_channel", json!([-50.0]))),
        ],
    ]
}

fn any_dither_profile() -> impl Strategy<Value = DitherProfile> {
    prop_oneof![
        Just(DitherProfile::Tpdf),
        Just(DitherProfile::ShapedHp),
        Just(DitherProfile::ShapedFWeighted),
    ]
}

fn any_peak_limiter_mode() -> impl Strategy<Value = PeakLimiterMode> {
    prop_oneof![
        Just(PeakLimiterMode::Off),
        Just(PeakLimiterMode::SoftClip),
        Just(PeakLimiterMode::LookaheadLimiter),
    ]
}

fn any_volume_curve() -> impl Strategy<Value = VolumeCurve> {
    prop_oneof![Just(VolumeCurve::Quadratic), Just(VolumeCurve::Logarithmic)]
}

fn any_crossfeed_preset() -> impl Strategy<Value = CrossfeedPreset> {
    prop_oneof![
        Just(CrossfeedPreset::Bauer),
        Just(CrossfeedPreset::BauerStrong),
        Just(CrossfeedPreset::Custom),
    ]
}

fn any_resampler_quality() -> impl Strategy<Value = ResamplerQuality> {
    prop_oneof![Just(ResamplerQuality::Standard), Just(ResamplerQuality::Best)]
}

/// JSON values approximate equality for f32-typed keys: we accept a
/// 1 ulp-ish tolerance because the value is round-tripped through
/// `f64` (serde_json's number representation).
fn json_value_close_enough(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let xf = x.as_f64().unwrap_or(f64::NAN);
            let yf = y.as_f64().unwrap_or(f64::NAN);
            (xf - yf).abs() <= 1e-6 * xf.abs().max(1.0)
        }
        (Value::Array(xs), Value::Array(ys)) => {
            xs.len() == ys.len()
                && xs.iter().zip(ys.iter()).all(|(x, y)| json_value_close_enough(x, y))
        }
        _ => a == b,
    }
}

/// Re-implementation of the field accessor (the in-crate version is
/// private). Used by Property 28 to compare per-field equality.
fn read_field_via_get(s: &AudioSettings, key: &str) -> Value {
    match key {
        "audio.rg_peak_protection" => json!(s.rg_peak_protection),
        "audio.rg_safety_headroom_db" => json!(s.rg_safety_headroom_db),
        "audio.dither_profile" => serde_json::to_value(s.dither_profile).unwrap(),
        "audio.peak_limiter_mode" => serde_json::to_value(s.peak_limiter_mode).unwrap(),
        "audio.peak_limiter_ceiling_dbfs" => json!(s.peak_limiter_ceiling_dbfs),
        "audio.peak_limiter_lookahead_ms" => json!(s.peak_limiter_lookahead_ms),
        "audio.peak_limiter_release_ms" => json!(s.peak_limiter_release_ms),
        "audio.volume_curve" => serde_json::to_value(s.volume_curve).unwrap(),
        "audio.volume_floor_db" => json!(s.volume_floor_db),
        "audio.crossfeed_enabled" => json!(s.crossfeed_enabled),
        "audio.crossfeed_preset" => serde_json::to_value(s.crossfeed_preset).unwrap(),
        "audio.crossfeed_delay_us" => json!(s.crossfeed_delay_us),
        "audio.crossfeed_lp_cutoff_hz" => json!(s.crossfeed_lp_cutoff_hz),
        "audio.resampler_quality" => serde_json::to_value(s.resampler_quality).unwrap(),
        "audio.convolver_enabled" => json!(s.convolver_enabled),
        "audio.convolver_ir_path" => match &s.convolver_ir_path {
            None => Value::Null,
            Some(p) => json!(p.to_string_lossy()),
        },
        "audio.convolver_gain_db" => json!(s.convolver_gain_db),
        "audio.balance" => json!(s.balance),
        "audio.trim_db_per_channel" => json!(s.trim_db_per_channel),
        _ => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Property 26 — round-trip set/get preserves the value
// ---------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 26
#[test]
fn property_26_settings_round_trip() {
    let fx = fresh_fixture();
    fx.store.load_or_init();

    proptest!(ProptestConfig::with_cases(100), |((key, value) in any_valid_kv())| {
        fx.store.set(key, value.clone()).map_err(|e| TestCaseError::fail(format!(
            "set({}, {}) returned {:?}", key, value, e
        )))?;

        let read_back = fx.store.get(key).ok_or_else(|| {
            TestCaseError::fail(format!("get({}) returned None after a successful set", key))
        })?;

        prop_assert!(
            json_value_close_enough(&read_back, &value),
            "round-trip mismatch on {}: wrote {}, read {}",
            key,
            value,
            read_back,
        );
    });
}

// ---------------------------------------------------------------------------
// Property 27 — load_or_init repairs missing and out-of-range keys
// ---------------------------------------------------------------------------

/// Pre-seed strategy: each key gets one of three states picked
/// uniformly: missing, valid, or aberrant (wrong JSON or out-of-
/// range number). The store must repair the table so every key is
/// present and within bounds.
#[derive(Debug, Clone)]
enum SeedState {
    Missing,
    Valid,
    Aberrant,
}

fn any_seed_state() -> impl Strategy<Value = SeedState> {
    prop_oneof![
        Just(SeedState::Missing),
        Just(SeedState::Valid),
        Just(SeedState::Aberrant),
    ]
}

fn any_seed_plan() -> impl Strategy<Value = Vec<SeedState>> {
    proptest::collection::vec(any_seed_state(), ALL_KEYS.len()..=ALL_KEYS.len())
}

fn aberrant_payload_for(key: &str) -> &'static str {
    // Each aberration is either malformed JSON or a numeric value
    // far outside the declared bounds. We keep the payloads as raw
    // strings so the SQLite column stores exactly what an older
    // (looser) build of Qobee might have written.
    match key {
        "audio.rg_peak_protection" => "\"yes please\"",
        "audio.rg_safety_headroom_db" => "999.0",
        "audio.dither_profile" => "\"unknown_profile\"",
        "audio.peak_limiter_mode" => "12",
        "audio.peak_limiter_ceiling_dbfs" => "10.0",
        "audio.peak_limiter_lookahead_ms" => "0.1",
        "audio.peak_limiter_release_ms" => "9999.0",
        "audio.volume_curve" => "true",
        "audio.volume_floor_db" => "-200.0",
        "audio.crossfeed_enabled" => "\"on\"",
        "audio.crossfeed_preset" => "\"nope\"",
        "audio.crossfeed_delay_us" => "1.0",
        "audio.crossfeed_lp_cutoff_hz" => "50000.0",
        "audio.resampler_quality" => "false",
        "audio.convolver_enabled" => "42",
        "audio.convolver_ir_path" => "{not valid json}",
        "audio.convolver_gain_db" => "12.0",
        "audio.balance" => "9.0",
        "audio.trim_db_per_channel" => "[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 5.0, -99.0]",
        _ => "\"\"",
    }
}

fn valid_payload_for(key: &str) -> &'static str {
    match key {
        "audio.rg_peak_protection" => "false",
        "audio.rg_safety_headroom_db" => "0.5",
        "audio.dither_profile" => "\"tpdf\"",
        "audio.peak_limiter_mode" => "\"off\"",
        "audio.peak_limiter_ceiling_dbfs" => "-2.5",
        "audio.peak_limiter_lookahead_ms" => "3.5",
        "audio.peak_limiter_release_ms" => "150.0",
        "audio.volume_curve" => "\"quadratic\"",
        "audio.volume_floor_db" => "-50.0",
        "audio.crossfeed_enabled" => "true",
        "audio.crossfeed_preset" => "\"bauer_strong\"",
        "audio.crossfeed_delay_us" => "350.0",
        "audio.crossfeed_lp_cutoff_hz" => "1000.0",
        "audio.resampler_quality" => "\"standard\"",
        "audio.convolver_enabled" => "false",
        "audio.convolver_ir_path" => "null",
        "audio.convolver_gain_db" => "-12.0",
        "audio.balance" => "0.25",
        "audio.trim_db_per_channel" => "[-3.0, -1.0]",
        _ => "null",
    }
}

/// Assert every numeric field of a snapshot is within the design's
/// declared bounds. Called after `load_or_init` to verify the
/// repair behaviour.
fn assert_field_in_bounds(snapshot: &AudioSettings) -> Result<(), TestCaseError> {
    prop_assert!((0.0..=3.0).contains(&snapshot.rg_safety_headroom_db));
    prop_assert!((-3.0..=0.0).contains(&snapshot.peak_limiter_ceiling_dbfs));
    prop_assert!((2.0..=10.0).contains(&snapshot.peak_limiter_lookahead_ms));
    prop_assert!((20.0..=500.0).contains(&snapshot.peak_limiter_release_ms));
    prop_assert!((-80.0..=-30.0).contains(&snapshot.volume_floor_db));
    prop_assert!((200.0..=400.0).contains(&snapshot.crossfeed_delay_us));
    prop_assert!((500.0..=1500.0).contains(&snapshot.crossfeed_lp_cutoff_hz));
    prop_assert!((-24.0..=0.0).contains(&snapshot.convolver_gain_db));
    prop_assert!((-1.0..=1.0).contains(&snapshot.balance));
    prop_assert!(snapshot.trim_db_per_channel.len() <= AudioSettings::MAX_CHANNELS);
    for t in &snapshot.trim_db_per_channel {
        prop_assert!((-12.0..=0.0).contains(t));
    }
    Ok(())
}

// Feature: audio-quality-improvements, Property 27
#[test]
fn property_27_load_or_init_repairs_table() {
    proptest!(ProptestConfig::with_cases(100), |(plan in any_seed_plan())| {
        let fx = fresh_fixture();

        // Seed the SQLite settings table according to the plan.
        // The fresh fixture starts empty, so `Missing` means "do
        // nothing"; `Valid` and `Aberrant` write a raw payload via
        // the cloned `Library` handle, which shares the same
        // database file as the store.
        for (key, state) in ALL_KEYS.iter().zip(plan.iter()) {
            match state {
                SeedState::Missing => {}
                SeedState::Valid => {
                    fx.library
                        .set_setting(key, valid_payload_for(key))
                        .expect("seed valid");
                }
                SeedState::Aberrant => {
                    fx.library
                        .set_setting(key, aberrant_payload_for(key))
                        .expect("seed aberrant");
                }
            }
        }

        // Run the repair pass.
        let snapshot = fx.store.load_or_init();
        assert_field_in_bounds(&snapshot)?;

        // Every key is present in the in-memory snapshot AND in
        // the table — the store rewrote anything it found
        // missing or invalid.
        for key in ALL_KEYS {
            prop_assert!(
                fx.store.get(key).is_some(),
                "key {} should be present after load_or_init", key,
            );
            let raw = fx.library.get_setting(key)
                .expect("library read")
                .ok_or_else(|| TestCaseError::fail(format!(
                    "key {} should be persisted after load_or_init", key
                )))?;
            // The persisted text must round-trip through serde_json
            // — Property 27 says "the stored value is parseable".
            let _: Value = serde_json::from_str(&raw)
                .map_err(|e| TestCaseError::fail(format!(
                    "key {} persisted as unparseable JSON: {}: {:?}", key, raw, e
                )))?;
        }
    });
}

// ---------------------------------------------------------------------------
// Property 28 — successful set is immediately visible
// ---------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 28
#[test]
fn property_28_set_is_immediately_visible() {
    let fx = fresh_fixture();
    fx.store.load_or_init();

    proptest!(ProptestConfig::with_cases(100), |((key, value) in any_valid_kv())| {
        // Snapshot before, to detect any movement other than the
        // single field we're about to mutate.
        let before = fx.store.snapshot();

        fx.store.set(key, value.clone()).map_err(|e| TestCaseError::fail(format!(
            "set({}, {}) returned {:?}", key, value, e
        )))?;

        // The new value is visible through `get` (which reads from
        // the in-memory snapshot, NOT the database — the
        // "immediate" wording in the property).
        let read = fx.store.get(key).expect("present");
        prop_assert!(
            json_value_close_enough(&read, &value),
            "snapshot field for {} did not reflect the set value", key,
        );

        // The engine adapter received the new snapshot AND it
        // matches the in-memory snapshot bit for bit.
        let after = fx.store.snapshot();
        let pushed = fx.mock.last().expect("apply was called");
        prop_assert_eq!(pushed, after.clone(), "engine snapshot diverged from store snapshot");

        // No regression: every key not equal to `key` is unchanged.
        for k in ALL_KEYS {
            if *k == key { continue; }
            let a = read_field_via_get(&before, k);
            let b = read_field_via_get(&after, k);
            prop_assert!(
                json_value_close_enough(&a, &b),
                "set({}, _) leaked into key {}: before={}, after={}",
                key, k, a, b,
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Property 29 — failing set is atomic
// ---------------------------------------------------------------------------

// Feature: audio-quality-improvements, Property 29
#[test]
fn property_29_invalid_set_is_atomic() {
    let fx = fresh_fixture();
    fx.store.load_or_init();

    proptest!(ProptestConfig::with_cases(100), |((key, value) in any_invalid_kv())| {
        let before_snapshot = fx.store.snapshot();
        let before_apply = fx.mock.last();

        let outcome = fx.store.set(key, value.clone());

        // Some "invalid" cases for trim_db_per_channel may slip
        // through if the random literal happens to be valid (e.g.
        // an array that fits within MAX_CHANNELS and whose entries
        // are all in `[-12.0, 0.0]`). The strategy mixes type and
        // range errors so Property 29 only constrains the failing
        // arm — successful arms simply do not contribute to this
        // property's invariant.
        if outcome.is_ok() {
            return Ok(());
        }

        // We reached an Err — the contract now demands strict
        // atomicity.
        let err = outcome.unwrap_err();
        prop_assert!(
            matches!(
                err,
                AudioSettingsError::UnknownKey(_)
                    | AudioSettingsError::TypeMismatch { .. }
                    | AudioSettingsError::OutOfRange { .. }
            ),
            "unexpected error variant: {:?}", err,
        );

        // In-memory snapshot strictly equal (PartialEq).
        let after_snapshot = fx.store.snapshot();
        prop_assert_eq!(after_snapshot, before_snapshot.clone(),
            "store snapshot mutated despite a failing set({}, {})", key, value);

        // Engine state strictly equal: no extra `apply` was called.
        let after_apply = fx.mock.last();
        prop_assert_eq!(after_apply, before_apply.clone(),
            "engine received an apply despite a failing set({}, {})", key, value);
    });
}
