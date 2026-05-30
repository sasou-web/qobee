//! ASIO output backend (Windows, pro-audio / audiophile).
//!
//! ## What this is
//!
//! ASIO (Audio Stream Input/Output) is Steinberg's low-latency,
//! exclusive audio API. On Windows it is the path audiophiles and
//! pro-audio users expect: it bypasses the OS mixer entirely, talks
//! to the device driver directly, and — like WASAPI Exclusive — can
//! deliver a strictly bit-perfect stream at the source sample rate
//! and bit depth. Many high-end USB DACs and pro interfaces ship an
//! ASIO driver as their preferred path.
//!
//! ## How it is built
//!
//! cpal already implements an ASIO host. Rather than duplicate the
//! ~3000-line decode → DSP → render worker that
//! [`crate::backend_cpal_shared::CpalSharedEngine`] already provides
//! and which is exercised by the whole property-test suite, this
//! backend *reuses* that engine and simply binds it to cpal's ASIO
//! host via [`HostKind::Asio`]. The negotiation, resampling, DSP
//! chain, dither, gapless and crossfade logic are therefore shared
//! verbatim with the Shared backend; only the host the device is
//! enumerated on, and the reported [`EffectiveOutputMode`], differ.
//!
//! The engine reports [`EffectiveOutputMode::Asio`], so the
//! bit-perfect health calculator treats it like Exclusive: it can
//! reach a Green badge when every DSP stage is at unity and the
//! device runs at the native rate (see
//! [`crate::types::BitPerfectHealth::compute`]).
//!
//! ## Build / runtime requirements (the honest part)
//!
//! A working ASIO backend needs two things that are *not* present in
//! a default build:
//!
//!   1. **Build time:** cpal's `asio` feature, which in turn requires
//!      the proprietary Steinberg ASIO SDK and LLVM/`bindgen`. We
//!      surface that through the `engine-asio` Cargo feature so the
//!      default build (and CI on machines without the SDK) stays
//!      green. See `crates/engine/Cargo.toml` and the project README
//!      for the `CPAL_ASIO_DIR` setup.
//!
//!   2. **Runtime:** an installed ASIO driver for the target device.
//!
//! When the `engine-asio` feature is **off**, [`AsioEngine::new`]
//! returns [`EngineError::BackendUnavailable`] with a localised,
//! actionable message. The orchestrator (`qobee-core`) catches this
//! exactly like a failed WASAPI-Exclusive open and falls back to the
//! Shared backend, so selecting ASIO on an unsupported build degrades
//! gracefully instead of breaking playback.
//!
//! ## Availability probe
//!
//! [`asio_available`] lets the orchestrator / UI decide whether to
//! offer ASIO as a selectable mode at all, without trying to open a
//! stream. It is `false` on every build that lacks the feature.

use std::path::Path;

use crossbeam_channel::Receiver;

use crate::backend_cpal_shared::{CpalSharedEngine, HostKind};
use crate::error::EngineResult;
use crate::types::{EngineEvent, OutputMode, PlayerState};
use crate::{AudioEngine, AudioSettings, DsdRate, EngineError, PreGainContext};

/// `true` when this build can actually open an ASIO stream — i.e. it
/// was compiled with the `engine-asio` feature on Windows. Cheap,
/// const-foldable; the UI uses it to decide whether to list ASIO as a
/// selectable output mode.
pub const fn asio_available() -> bool {
    cfg!(all(target_os = "windows", feature = "engine-asio"))
}

/// ASIO output engine.
///
/// A thin wrapper around a [`CpalSharedEngine`] bound to cpal's ASIO
/// host. Every [`AudioEngine`] call is forwarded to the inner engine;
/// the wrapper exists so the orchestrator can hold an `Arc<AsioEngine>`
/// distinct from the Shared engine and so construction can fail fast
/// with a clean [`EngineError::BackendUnavailable`] on builds without
/// ASIO support.
pub struct AsioEngine {
    inner: CpalSharedEngine,
}

impl AsioEngine {
    /// Build an ASIO-backed engine.
    ///
    /// Returns [`EngineError::BackendUnavailable`] when the build does
    /// not include the `engine-asio` feature (or is not Windows). The
    /// inner engine spawns its worker thread immediately, but no ASIO
    /// stream is opened until the first [`AudioEngine::load`] + `play`,
    /// at which point a missing driver surfaces as a stream error and
    /// the orchestrator falls back to Shared.
    pub fn new() -> EngineResult<Self> {
        if !asio_available() {
            return Err(EngineError::BackendUnavailable(
                "Le mode ASIO n'est pas inclus dans cette build \
                 (fonctionnalité « engine-asio » désactivée). \
                 Réinstallez une version compilée avec le SDK ASIO."
                    .to_string(),
            ));
        }
        let inner = CpalSharedEngine::with_host(HostKind::Asio)?;
        Ok(Self { inner })
    }
}

impl AudioEngine for AsioEngine {
    fn load(&self, path: &Path) -> EngineResult<()> {
        self.inner.load(path)
    }

    fn load_dsd(&self, path: &Path, rate: DsdRate) -> EngineResult<()> {
        // DSD over ASIO is a future extension (it would need a DoP or
        // native-DSD packer on the ASIO sample path). For now defer to
        // the inner engine, which rejects DSD with `BackendUnavailable`
        // just like the Shared backend — the orchestrator already
        // routes DSD to WASAPI Exclusive.
        self.inner.load_dsd(path, rate)
    }

    fn play(&self) -> EngineResult<()> {
        self.inner.play()
    }

    fn pause(&self) -> EngineResult<()> {
        self.inner.pause()
    }

    fn resume(&self) -> EngineResult<()> {
        self.inner.resume()
    }

    fn stop(&self) -> EngineResult<()> {
        self.inner.stop()
    }

    fn seek(&self, position_seconds: f64) -> EngineResult<()> {
        self.inner.seek(position_seconds)
    }

    fn set_volume(&self, volume: f32) -> EngineResult<()> {
        self.inner.set_volume(volume)
    }

    fn set_output_mode(&self, mode: OutputMode) -> EngineResult<()> {
        self.inner.set_output_mode(mode)
    }

    fn state(&self) -> PlayerState {
        self.inner.state()
    }

    fn subscribe_events(&self) -> Receiver<EngineEvent> {
        self.inner.subscribe_events()
    }

    fn set_current_track_id(&self, id: Option<String>) {
        self.inner.set_current_track_id(id)
    }

    fn set_eq_gains_db(&self, gains: &[f32]) {
        self.inner.set_eq_gains_db(gains)
    }

    fn eq_gains_db(&self) -> Vec<f32> {
        self.inner.eq_gains_db()
    }

    fn set_pre_gain(&self, linear: f32) {
        self.inner.set_pre_gain(linear)
    }

    fn set_output_device(&self, device_id: Option<String>) {
        self.inner.set_output_device(device_id)
    }

    fn selected_device(&self) -> Option<String> {
        self.inner.selected_device()
    }

    fn set_audio_settings(&self, settings: AudioSettings) {
        self.inner.set_audio_settings(settings)
    }

    fn set_pre_gain_context(&self, ctx: PreGainContext) {
        self.inner.set_pre_gain_context(ctx)
    }

    fn set_convolver_ir(&self, ir_left: Vec<f32>, ir_right: Vec<f32>) {
        self.inner.set_convolver_ir(ir_left, ir_right)
    }

    fn convolver_ir_len(&self) -> usize {
        self.inner.convolver_ir_len()
    }

    fn is_dsd_active(&self) -> bool {
        self.inner.is_dsd_active()
    }

    fn prepare_next(&self, path: &Path, track_id: Option<String>) -> EngineResult<()> {
        self.inner.prepare_next(path, track_id)
    }

    fn clear_pending_next(&self) -> EngineResult<()> {
        self.inner.clear_pending_next()
    }

    fn set_crossfade_ms(&self, ms: u32) {
        self.inner.set_crossfade_ms(ms)
    }
}

/// Enumerate output devices on the ASIO host.
///
/// Returns an empty list (not an error) when the build lacks ASIO
/// support, so callers can merge it with the system device list
/// unconditionally. When ASIO is available, each entry's `id` is the
/// driver/device name cpal reports, suitable to pass back to
/// [`AudioEngine::set_output_device`].
pub fn list_asio_devices() -> EngineResult<Vec<crate::types::OutputDevice>> {
    #[cfg(all(target_os = "windows", feature = "engine-asio"))]
    {
        crate::backend_cpal_shared::list_output_devices_on(HostKind::Asio)
    }
    #[cfg(not(all(target_os = "windows", feature = "engine-asio")))]
    {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_matches_build_config() {
        // The probe is a pure cfg!() fold; assert it tracks the build.
        let expected = cfg!(all(target_os = "windows", feature = "engine-asio"));
        assert_eq!(asio_available(), expected);
    }

    #[cfg(not(all(target_os = "windows", feature = "engine-asio")))]
    #[test]
    fn new_is_backend_unavailable_without_feature() {
        // On every build that lacks the feature, constructing the
        // engine must fail cleanly (never panic) so the orchestrator
        // can fall back to Shared. `AsioEngine` doesn't implement
        // `Debug`, so we match instead of using `expect_err`.
        match AsioEngine::new() {
            Err(EngineError::BackendUnavailable(_)) => {}
            Err(other) => panic!("expected BackendUnavailable, got {other:?}"),
            Ok(_) => panic!("ASIO must be unavailable without the feature"),
        }
    }

    #[cfg(not(all(target_os = "windows", feature = "engine-asio")))]
    #[test]
    fn list_asio_devices_is_empty_without_feature() {
        let devices = list_asio_devices().expect("listing must not error without the feature");
        assert!(devices.is_empty());
    }
}
