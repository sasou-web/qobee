//! Property test for the per-device format cache
//! (`Device_Format_Cache`, task 5.2).
//!
//! The cache lives in `qobee_engine::format_cache` and is re-exported
//! (doc-hidden) as [`qobee_engine::DeviceFormatCache`] /
//! [`qobee_engine::CachedFormat`] / [`qobee_engine::DeviceConfigFingerprint`].
//! It is generic over the negotiated-format payload `F`. The production
//! payload `NegotiatedFormat` wraps a Windows-only `wasapi::WaveFormat`,
//! so here we instantiate the cache with a lightweight, comparable
//! stand-in payload ([`StandInFormat`]) — the generic
//! `DeviceFormatCache<F>` exercises the exact same hit / miss logic on
//! every platform.
//!
//! Implements **Property 5: Cohérence du Device_Format_Cache** from the
//! design document.

use proptest::prelude::*;
use qobee_engine::{CachedFormat, DeviceConfigFingerprint, DeviceFormatCache};

/// Lightweight, cross-platform stand-in for the Windows-only
/// `NegotiatedFormat` payload. It carries just enough plain data
/// (sample rate + channel count) to be meaningfully compared, so a
/// cache hit can assert the *same* format comes back. `PartialEq`/`Eq`
/// let the property compare the round-tripped payload exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StandInFormat {
    sample_rate: u32,
    channels: u16,
}

/// Non-empty device identifier (mirrors the production "friendly name +
/// endpoint id" string key). Constrained to a small printable alphabet
/// so failing cases shrink to something readable.
fn any_device_id() -> impl Strategy<Value = String> {
    "[A-Za-z0-9 _:-]{1,24}"
}

/// A device-config fingerprint over realistic mixformat values: a common
/// sample rate and a channel count in `1..=8`.
fn any_fingerprint() -> impl Strategy<Value = DeviceConfigFingerprint> {
    (
        prop_oneof![
            Just(44_100u32),
            Just(48_000u32),
            Just(88_200u32),
            Just(96_000u32),
            Just(176_400u32),
            Just(192_000u32),
        ],
        1u16..=8u16,
    )
        .prop_map(|(mix_sample_rate, mix_channels)| DeviceConfigFingerprint {
            mix_sample_rate,
            mix_channels,
        })
}

/// A stand-in negotiated-format payload.
fn any_format() -> impl Strategy<Value = StandInFormat> {
    (
        prop_oneof![
            Just(44_100u32),
            Just(48_000u32),
            Just(96_000u32),
            Just(192_000u32),
        ],
        1u16..=8u16,
    )
        .prop_map(|(sample_rate, channels)| StandInFormat {
            sample_rate,
            channels,
        })
}

/// Produce a fingerprint guaranteed to differ from `fp`. The generators
/// only ever sample sample rates from a fixed set whose maximum is
/// `192_000`, so `mix_sample_rate + 1` can never collide with a
/// generated fingerprint and never overflows.
fn different_fingerprint(fp: DeviceConfigFingerprint) -> DeviceConfigFingerprint {
    DeviceConfigFingerprint {
        mix_sample_rate: fp.mix_sample_rate + 1,
        mix_channels: fp.mix_channels,
    }
}

// Feature: qobee-beta-feedback-improvements, Property 5: Cohérence du Device_Format_Cache
//
// For an arbitrary `(device_id, fingerprint, payload)`:
//   * after `put`, a `get` at the *same* fingerprint is a hit that
//     returns an equal format — the negotiated format is reused without
//     re-probing (R9.1 store, R9.2 reuse);
//   * a `get` at a *different* fingerprint is a miss (R9.3 staleness
//     guard — the device configuration changed);
//   * after `invalidate`, a `get` is a miss even at the original
//     fingerprint (R9.3 — config change / disconnection / failed apply).
//
// **Validates: Requirements 9.1, 9.2, 9.3**
#[test]
fn property_5_device_format_cache_coherence() {
    proptest!(
        ProptestConfig::with_cases(256),
        |(device_id in any_device_id(),
          fp in any_fingerprint(),
          is_native in any::<bool>(),
          nego in any_format())| {

            let mut cache: DeviceFormatCache<StandInFormat> = DeviceFormatCache::new();
            prop_assert!(cache.is_empty(), "a fresh cache must be empty");

            // A clean cache misses for every device id (no false hit).
            prop_assert!(
                cache.get(&device_id, &fp).is_none(),
                "empty cache must miss before any put"
            );

            // --- put, then get at the SAME fingerprint => hit (R9.1 / R9.2) ---
            let cached = CachedFormat::new(nego.clone(), is_native, fp);
            cache.put(device_id.clone(), cached.clone());

            let hit = cache.get(&device_id, &fp);
            prop_assert!(
                hit.is_some(),
                "expected a hit after put at the same fingerprint"
            );
            let hit = hit.unwrap();
            // The hit returns the *same* format — no renegotiation.
            prop_assert_eq!(&hit.nego, &nego, "hit must return the stored format");
            prop_assert_eq!(hit.is_native_rate, is_native);
            prop_assert_eq!(hit.config_fingerprint, fp);
            prop_assert_eq!(hit, &cached, "hit must equal the cached entry");

            // --- get at a DIFFERENT fingerprint => miss (R9.3) ---
            let other = different_fingerprint(fp);
            prop_assert_ne!(other, fp, "the alternate fingerprint must differ");
            prop_assert!(
                cache.get(&device_id, &other).is_none(),
                "a fingerprint mismatch (changed device config) must be a miss"
            );
            // The mismatch is a non-destructive read: the original entry
            // is still reusable at its own fingerprint.
            prop_assert!(
                cache.get(&device_id, &fp).is_some(),
                "a mismatch lookup must not evict the existing entry"
            );

            // --- after invalidate => miss even at the original fp (R9.3) ---
            cache.invalidate(&device_id);
            prop_assert!(
                cache.get(&device_id, &fp).is_none(),
                "after invalidate the device must miss at its own fingerprint"
            );
            prop_assert!(cache.is_empty(), "invalidating the only entry empties the cache");
        }
    );
}
