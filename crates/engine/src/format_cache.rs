//! Per-device format cache for WASAPI Exclusive negotiation (`R9`).
//!
//! These types are *worker-only* in production: in the WASAPI
//! Exclusive backend they live entirely on the worker thread that owns
//! COM, so they need neither `Send` nor `Sync`, and they are
//! deliberately **not** serialized (they never cross the Tauri / IPC
//! boundary).
//!
//! They are placed in this standalone, non-`cfg`-gated module — rather
//! than inline in the Windows-only [`crate::backend_wasapi_exclusive`]
//! module — so the cross-platform integration property test
//! (`tests/properties/device_format_cache.rs`, task 5.2) can construct
//! and exercise them on *every* platform. Mirrors the
//! [`crate::ring_plan`] placement decision.
//!
//! ## Why generic over the payload `F`
//!
//! The design (`Couche 3 → R9`) writes `CachedFormat { nego:
//! NegotiatedFormat, .. }`. But `NegotiatedFormat` wraps a Windows-only
//! `wasapi::WaveFormat`, so a concrete `NegotiatedFormat` payload would
//! make the whole cache Windows-only and unreachable from the
//! cross-platform property test. We therefore parameterise the cache by
//! the negotiated-format payload type `F`: the Windows backend
//! instantiates it as `DeviceFormatCache<NegotiatedFormat>` (storing the
//! real format it must later apply, including the opaque `WaveFormat`),
//! while the property test instantiates it with a lightweight,
//! comparable stand-in. The cache logic (hit only on matching
//! fingerprint, miss on mismatch / after invalidation) is identical and
//! payload-agnostic.
//!
//! Feature: qobee-beta-feedback-improvements, R9 `Device_Format_Cache`.

use std::collections::HashMap;

/// Fingerprint of a device's configuration at the moment a format was
/// cached: the device mixformat (sample rate + channel count).
///
/// A mismatch on lookup means the device configuration changed
/// (default-format change, reconnection, ...), so the cached entry is
/// stale and must be ignored / invalidated rather than reused (R9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceConfigFingerprint {
    /// Device mixformat sample rate (Hz) at cache time.
    pub mix_sample_rate: u32,
    /// Device mixformat channel count at cache time.
    pub mix_channels: u16,
}

/// A negotiated format memorised for a device (R9.1).
///
/// Generic over the negotiated-format payload `F` (see module docs).
/// `#[derive]`d impls are *conditional* on `F`, so they are available
/// only for payloads that themselves implement the trait — the Windows
/// `NegotiatedFormat` (which is only `Clone`) still works because the
/// backend never needs `PartialEq`/`Eq` on the cached entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFormat<F> {
    /// The negotiated format to reuse on a cache hit (sample rate,
    /// channels, `src_channels`, sample type, valid bits, ...). On
    /// Windows this is the backend's `NegotiatedFormat`.
    pub nego: F,
    /// Whether the negotiated rate equals the source rate (no
    /// resampling). Mirrors the `bool` second element returned by the
    /// backend's `negotiate_with_fallback`.
    pub is_native_rate: bool,
    /// Device-config fingerprint captured when this entry was stored.
    /// Used to detect a configuration change and invalidate (R9.3).
    pub config_fingerprint: DeviceConfigFingerprint,
}

impl<F> CachedFormat<F> {
    /// Convenience constructor.
    pub fn new(nego: F, is_native_rate: bool, config_fingerprint: DeviceConfigFingerprint) -> Self {
        Self {
            nego,
            is_native_rate,
            config_fingerprint,
        }
    }
}

/// Cache mapping a stable device identifier (friendly name + endpoint
/// id) to its negotiated format. Worker-only; not serialized.
///
/// The lookup contract is the heart of R9:
///   * [`get`](Self::get) returns `Some` **only** when an entry exists
///     *and* its stored fingerprint matches the supplied one — this is
///     both the reuse path (R9.2) and the staleness guard (R9.3).
///   * [`put`](Self::put) records a freshly negotiated format (R9.1).
///   * [`invalidate`](Self::invalidate) drops an entry on config change,
///     disconnection, or failure to apply a cached format (R9.3).
#[derive(Debug, Clone)]
pub struct DeviceFormatCache<F> {
    map: HashMap<String, CachedFormat<F>>,
}

impl<F> Default for DeviceFormatCache<F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F> DeviceFormatCache<F> {
    /// An empty cache.
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Look up a cached format for `device_id`.
    ///
    /// Returns `Some` **only** when an entry exists *and* its stored
    /// fingerprint equals `fp`: the negotiated format can be reused
    /// without re-probing every candidate rate (R9.2). A fingerprint
    /// mismatch — the device's configuration changed since caching — is
    /// reported as a miss (`None`) so the caller renegotiates (R9.3).
    pub fn get(&self, device_id: &str, fp: &DeviceConfigFingerprint) -> Option<&CachedFormat<F>> {
        self.map
            .get(device_id)
            .filter(|c| &c.config_fingerprint == fp)
    }

    /// Insert (or replace) the cached format for `device_id`, recording
    /// the result of a successful negotiation (R9.1).
    pub fn put(&mut self, device_id: String, c: CachedFormat<F>) {
        self.map.insert(device_id, c);
    }

    /// Drop the cached entry for `device_id` (R9.3): the device
    /// configuration changed, it was disconnected, or applying the
    /// cached format failed. A subsequent `get` is a guaranteed miss
    /// until the next `put`.
    pub fn invalidate(&mut self, device_id: &str) {
        self.map.remove(device_id);
    }

    /// Number of devices currently cached (diagnostic / test helper).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache holds no entries (diagnostic / test helper).
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal cross-platform stand-in for a negotiated format. Mirrors
    /// the plain-data fields of the backend's `NegotiatedFormat` without
    /// the Windows-only `WaveFormat`, so the cache logic can be tested
    /// on any platform.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestFormat {
        sample_rate: u32,
        channels: u16,
        src_channels: u16,
    }

    fn fp(sr: u32, ch: u16) -> DeviceConfigFingerprint {
        DeviceConfigFingerprint {
            mix_sample_rate: sr,
            mix_channels: ch,
        }
    }

    fn fmt(sr: u32, ch: u16) -> TestFormat {
        TestFormat {
            sample_rate: sr,
            channels: ch,
            src_channels: 2,
        }
    }

    #[test]
    fn empty_cache_misses() {
        let cache: DeviceFormatCache<TestFormat> = DeviceFormatCache::new();
        assert!(cache.is_empty());
        assert!(cache.get("dev-A", &fp(48_000, 2)).is_none());
    }

    #[test]
    fn put_then_get_same_fingerprint_hits_with_same_format() {
        let mut cache = DeviceFormatCache::new();
        let f = fp(48_000, 2);
        let cached = CachedFormat::new(fmt(48_000, 2), true, f);
        cache.put("dev-A".to_string(), cached.clone());

        let got = cache.get("dev-A", &f).expect("expected a hit");
        // Reuse path (R9.2): same format comes back, no renegotiation.
        assert_eq!(got, &cached);
        assert_eq!(got.nego, fmt(48_000, 2));
        assert!(got.is_native_rate);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn get_with_different_fingerprint_misses() {
        let mut cache = DeviceFormatCache::new();
        cache.put(
            "dev-A".to_string(),
            CachedFormat::new(fmt(48_000, 2), true, fp(48_000, 2)),
        );
        // Device config changed (mixformat now 96k): staleness guard
        // (R9.3) → miss even though an entry exists for the id.
        assert!(cache.get("dev-A", &fp(96_000, 2)).is_none());
        // ...and a channel-count change is also a miss.
        assert!(cache.get("dev-A", &fp(48_000, 8)).is_none());
    }

    #[test]
    fn invalidate_drops_entry() {
        let mut cache = DeviceFormatCache::new();
        let f = fp(44_100, 2);
        cache.put(
            "dev-A".to_string(),
            CachedFormat::new(fmt(44_100, 2), false, f),
        );
        assert!(cache.get("dev-A", &f).is_some());

        cache.invalidate("dev-A"); // R9.3
        assert!(cache.get("dev-A", &f).is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn put_replaces_existing_entry_for_same_device() {
        let mut cache = DeviceFormatCache::new();
        cache.put(
            "dev-A".to_string(),
            CachedFormat::new(fmt(48_000, 2), true, fp(48_000, 2)),
        );
        // Renegotiation after a config change overwrites the entry.
        let new_fp = fp(96_000, 2);
        cache.put(
            "dev-A".to_string(),
            CachedFormat::new(fmt(96_000, 2), false, new_fp),
        );
        assert_eq!(cache.len(), 1);
        let got = cache.get("dev-A", &new_fp).expect("expected a hit");
        assert_eq!(got.nego, fmt(96_000, 2));
        assert!(!got.is_native_rate);
    }

    #[test]
    fn entries_are_keyed_per_device() {
        let mut cache = DeviceFormatCache::new();
        cache.put(
            "dev-A".to_string(),
            CachedFormat::new(fmt(48_000, 2), true, fp(48_000, 2)),
        );
        cache.put(
            "dev-B".to_string(),
            CachedFormat::new(fmt(44_100, 2), true, fp(44_100, 2)),
        );
        assert_eq!(cache.len(), 2);
        assert!(cache.get("dev-A", &fp(48_000, 2)).is_some());
        assert!(cache.get("dev-B", &fp(44_100, 2)).is_some());
        // Invalidating one leaves the other intact.
        cache.invalidate("dev-A");
        assert!(cache.get("dev-A", &fp(48_000, 2)).is_none());
        assert!(cache.get("dev-B", &fp(44_100, 2)).is_some());
    }
}
