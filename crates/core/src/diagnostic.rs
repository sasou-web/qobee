//! Null-test diagnostic orchestration (Phase F of audio-quality-improvements).
//!
//! Glues together the engine-side primitives (test WAV generator,
//! WASAPI loopback capture, Exclusive pre-render sink, FFT-based
//! aligner) into a single end-to-end run that produces a
//! [`NullTestReport`] for the UI.
//!
//! Two capture branches:
//!
//!   * **Loopback** (Shared mode): a background thread runs a WASAPI
//!     loopback capture on the active render device while the engine
//!     plays the deterministic source WAV. Once playback finishes the
//!     capture is aligned with the source via FFT cross-correlation
//!     and a sample-level diff is produced (R11.3).
//!   * **PreRender** (Exclusive mode): loopback is unavailable when
//!     another client holds the device exclusively, so the engine
//!     instead writes its post-DSP stream to a 24-bit WAV on disk.
//!     The "captured" file is exactly what would have been sent to
//!     the DAC; the comparison is purely numerical (R11.3 alternative).
//!
//! The generated source WAV and any capture file are deleted on
//! every exit path (R11.9), success or error.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::RecvTimeoutError;
use serde::{Deserialize, Serialize};

use qobee_engine::diagnostic::{
    align_by_cross_correlation, compute_diff, generate_test_wav, NullTestStats,
    DEFAULT_TEST_WAV_SEED,
};
use qobee_engine::{AudioEngine, EffectiveOutputMode, EngineEvent, OutputMode};

use crate::player::PlayerHandle;

/// Conclusion surfaced by [`NullTestReport`]. The badge in the UI
/// is rendered from this enum directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NullTestConclusion {
    BitPerfect,
    Modified,
    Inconclusive,
}

/// Which capture branch produced the `captured` buffer in
/// [`NullTestReport`]. Exposed so the UI can label the result with
/// the actual mechanism used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NullTestSource {
    Loopback,
    PreRender,
}

/// Final report returned to the UI. Mirrors the `NullTestReport`
/// type listed in the design's Tauri command table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NullTestReport {
    pub conclusion: NullTestConclusion,
    pub mode: EffectiveOutputMode,
    pub captured_via: NullTestSource,
    pub peak_diff_dbfs: f32,
    pub rms_diff_dbfs: f32,
    pub samples_diff_count: u64,
    pub aligned_frames: u64,
    pub error: Option<String>,
}

impl NullTestReport {
    fn inconclusive(
        mode: EffectiveOutputMode,
        captured_via: NullTestSource,
        message: impl Into<String>,
    ) -> Self {
        NullTestReport {
            conclusion: NullTestConclusion::Inconclusive,
            mode,
            captured_via,
            peak_diff_dbfs: f32::NEG_INFINITY,
            rms_diff_dbfs: f32::NEG_INFINITY,
            samples_diff_count: 0,
            aligned_frames: 0,
            error: Some(message.into()),
        }
    }

    fn from_stats(
        mode: EffectiveOutputMode,
        captured_via: NullTestSource,
        stats: NullTestStats,
    ) -> Self {
        let conclusion = if stats.samples_diff_count == 0 {
            NullTestConclusion::BitPerfect
        } else {
            NullTestConclusion::Modified
        };
        NullTestReport {
            conclusion,
            mode,
            captured_via,
            peak_diff_dbfs: stats.peak_diff_dbfs,
            rms_diff_dbfs: stats.rms_diff_dbfs,
            samples_diff_count: stats.samples_diff_count,
            aligned_frames: stats.aligned_frames,
            error: None,
        }
    }
}

/// Cancellation flag shared across the pump and the orchestrator.
/// Lives at module scope so the Tauri `cancel_null_test` command
/// can flip it without piping a handle through `AppState`.
fn cancel_flag() -> &'static Arc<AtomicBool> {
    static CANCEL_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    CANCEL_FLAG.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// Trip the cancellation flag observed by an in-flight
/// [`run_null_test`]. The current run finishes early with an
/// `Inconclusive` report once the flag is observed.
pub fn cancel_null_test() {
    cancel_flag().store(true, Ordering::Release);
}

/// Resolve `%APPDATA%/Qobee/diagnostic/` (or the equivalent on
/// other OSes), creating the directory if missing. Falls back to
/// the system temp dir when no config dir can be derived (very
/// uncommon; we still want the diagnostic to run).
fn diagnostic_dir() -> PathBuf {
    let base = dirs::config_dir()
        .map(|p| p.join("Qobee").join("diagnostic"))
        .unwrap_or_else(|| std::env::temp_dir().join("qobee-diagnostic"));
    let _ = std::fs::create_dir_all(&base);
    base
}

/// Best-effort cleanup helper used by every exit path so the
/// generated WAVs never linger on disk (R11.9).
fn cleanup(paths: &[&Path]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

/// Run the null-test diagnostic end-to-end. Picks Loopback when the
/// engine is in Shared mode and PreRender when it's in Exclusive.
/// Always returns a report; failures surface as `Inconclusive` with
/// `error = Some(...)`. Generated files are deleted before this
/// function returns, even on error.
pub fn run_null_test(player: &PlayerHandle) -> NullTestReport {
    cancel_flag().store(false, Ordering::Release);

    let mode = match player.current_output_mode() {
        OutputMode::Exclusive => EffectiveOutputMode::Exclusive,
        OutputMode::Asio => EffectiveOutputMode::Asio,
        _ => EffectiveOutputMode::Shared,
    };
    let captured_via = match mode {
        EffectiveOutputMode::Shared => NullTestSource::Loopback,
        // Exclusive and ASIO both bypass the OS mixer, so a loopback
        // capture would record silence; use the decoder-side
        // pre-render sink instead.
        EffectiveOutputMode::Exclusive | EffectiveOutputMode::Asio => NullTestSource::PreRender,
    };

    let dir = diagnostic_dir();
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let source_path = dir.join(format!("null_test_{unix}.wav"));
    let capture_path = dir.join(format!("null_test_{unix}_capture.wav"));

    if let Err(e) = generate_test_wav(&source_path, DEFAULT_TEST_WAV_SEED) {
        cleanup(&[&source_path, &capture_path]);
        return NullTestReport::inconclusive(mode, captured_via, format!("WAV gen: {e}"));
    }

    let report = match captured_via {
        NullTestSource::Loopback => run_loopback_branch(player, mode, &source_path, &capture_path),
        NullTestSource::PreRender => {
            run_pre_render_branch(player, mode, &source_path, &capture_path)
        }
    };

    cleanup(&[&source_path, &capture_path]);
    report
}

/// Shared-mode branch: spawn a loopback capture on a worker thread,
/// load + play the source on the engine, wait until the capture
/// thread returns, then align + diff in memory.
fn run_loopback_branch(
    player: &PlayerHandle,
    mode: EffectiveOutputMode,
    source_path: &Path,
    _capture_path: &Path,
) -> NullTestReport {
    let captured_via = NullTestSource::Loopback;
    // Slightly longer than the 5 s source so we always capture the
    // full track plus a bit of post-roll silence. The aligner trims
    // the extra padding via cross-correlation.
    let capture_duration = Duration::from_millis(6_000);

    let cancel = Arc::clone(cancel_flag());
    let capture_handle = thread::spawn(move || {
        qobee_engine::diagnostic::loopback::capture_loopback(None, capture_duration)
            .map_err(|e| e.to_string())
            .map(|samples| {
                if cancel.load(Ordering::Acquire) {
                    Vec::new()
                } else {
                    samples
                }
            })
    });

    // Give the loopback client a beat to spin up before we start
    // playing the source. 80 ms is enough for the WASAPI handshake
    // on every machine we have measured.
    thread::sleep(Duration::from_millis(80));

    let engine = player.shared_engine();
    let engine: Arc<dyn AudioEngine> = engine as Arc<dyn AudioEngine>;
    if let Err(e) = engine.load(source_path) {
        let _ = capture_handle.join();
        return NullTestReport::inconclusive(mode, captured_via, format!("load: {e}"));
    }
    if let Err(e) = engine.play() {
        let _ = capture_handle.join();
        return NullTestReport::inconclusive(mode, captured_via, format!("play: {e}"));
    }

    // Wait for the capture window or an explicit cancel.
    let started = std::time::Instant::now();
    while started.elapsed() < capture_duration {
        if cancel_flag().load(Ordering::Acquire) {
            let _ = engine.stop();
            let _ = capture_handle.join();
            return NullTestReport::inconclusive(mode, captured_via, "cancelled by user");
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = engine.stop();

    let captured = match capture_handle.join() {
        Ok(Ok(samples)) => samples,
        Ok(Err(e)) => {
            return NullTestReport::inconclusive(mode, captured_via, format!("loopback: {e}"));
        }
        Err(_) => {
            return NullTestReport::inconclusive(mode, captured_via, "loopback thread panicked");
        }
    };
    if captured.is_empty() {
        return NullTestReport::inconclusive(mode, captured_via, "no samples captured");
    }

    let source_samples = match read_wav_f32(source_path) {
        Ok(s) => s,
        Err(e) => {
            return NullTestReport::inconclusive(mode, captured_via, format!("read source: {e}"));
        }
    };

    align_and_diff(mode, captured_via, &source_samples, &captured)
}

/// Exclusive-mode branch: open a pre-render sink at `capture_path`,
/// load + play the source, wait for `EndOfTrack`, finalise the
/// sink, then align + diff in memory.
fn run_pre_render_branch(
    player: &PlayerHandle,
    mode: EffectiveOutputMode,
    source_path: &Path,
    capture_path: &Path,
) -> NullTestReport {
    let captured_via = NullTestSource::PreRender;
    let shared_engine = player.shared_engine();

    // Pre-render is implemented on the Shared (CPAL) engine because
    // it is always present and shares the decoder thread that owns
    // the PCM chain. The Exclusive engine's render path goes through
    // WASAPI directly and bypasses the sink; routing the diagnostic
    // through Shared keeps the comparison purely software-side
    // (R11.3 alternative) and matches the design's note on the
    // Exclusive branch.
    let test_spec = qobee_engine::diagnostic::TestWavSpec::default();
    if let Err(e) =
        shared_engine.start_pre_render(capture_path, test_spec.sample_rate, test_spec.channels)
    {
        return NullTestReport::inconclusive(mode, captured_via, format!("pre-render: {e}"));
    }

    let events = player.subscribe_engine_events();

    let engine: Arc<dyn AudioEngine> = Arc::clone(&shared_engine) as Arc<dyn AudioEngine>;
    if let Err(e) = engine.load(source_path) {
        let _ = shared_engine.finalise_pre_render();
        return NullTestReport::inconclusive(mode, captured_via, format!("load: {e}"));
    }
    if let Err(e) = engine.play() {
        let _ = shared_engine.finalise_pre_render();
        return NullTestReport::inconclusive(mode, captured_via, format!("play: {e}"));
    }

    // Pump events until EndOfTrack or a cancel. The 30 s timeout is
    // a safety net: a 5 s WAV should finish within ~5–6 s real time
    // even with a slow DAC.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if cancel_flag().load(Ordering::Acquire) {
            let _ = engine.stop();
            let _ = shared_engine.finalise_pre_render();
            return NullTestReport::inconclusive(mode, captured_via, "cancelled by user");
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            let _ = engine.stop();
            let _ = shared_engine.finalise_pre_render();
            return NullTestReport::inconclusive(mode, captured_via, "pre-render timeout");
        }
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(EngineEvent::EndOfTrack) => break,
            Ok(EngineEvent::Error { message }) => {
                let _ = engine.stop();
                let _ = shared_engine.finalise_pre_render();
                return NullTestReport::inconclusive(mode, captured_via, message);
            }
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                let _ = shared_engine.finalise_pre_render();
                return NullTestReport::inconclusive(
                    mode,
                    captured_via,
                    "engine event channel disconnected",
                );
            }
        }
    }

    if let Err(e) = shared_engine.finalise_pre_render() {
        return NullTestReport::inconclusive(mode, captured_via, format!("finalise: {e}"));
    }

    let source_samples = match read_wav_f32(source_path) {
        Ok(s) => s,
        Err(e) => {
            return NullTestReport::inconclusive(mode, captured_via, format!("read source: {e}"));
        }
    };
    let captured = match read_wav_f32(capture_path) {
        Ok(s) => s,
        Err(e) => {
            return NullTestReport::inconclusive(mode, captured_via, format!("read capture: {e}"));
        }
    };

    align_and_diff(mode, captured_via, &source_samples, &captured)
}

/// Compute the lag, run the diff, and assemble the final report.
fn align_and_diff(
    mode: EffectiveOutputMode,
    captured_via: NullTestSource,
    source: &[f32],
    captured: &[f32],
) -> NullTestReport {
    let lag = align_by_cross_correlation(source, captured);
    let stats = compute_diff(source, captured, lag);
    NullTestReport::from_stats(mode, captured_via, stats)
}

/// Minimal WAV reader: parses the RIFF header, walks the chunks,
/// and decodes PCM 16/24/32-bit and 32-bit float into a single
/// interleaved `Vec<f32>` in `[-1.0, 1.0]`. Used to read both the
/// generated source WAV and the pre-render output without pulling
/// `hound` in.
fn read_wav_f32(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }

    let mut pos = 12;
    let mut bits_per_sample: u16 = 0;
    let mut format_tag: u16 = 0;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        let body_end = body_start + size;
        if body_end > bytes.len() {
            break;
        }
        match id {
            b"fmt " => {
                if size < 16 {
                    return Err("short fmt chunk".to_string());
                }
                format_tag =
                    u16::from_le_bytes(bytes[body_start..body_start + 2].try_into().unwrap());
                bits_per_sample =
                    u16::from_le_bytes(bytes[body_start + 14..body_start + 16].try_into().unwrap());
            }
            b"data" => {
                data = Some(&bytes[body_start..body_end]);
                break;
            }
            _ => {}
        }
        // Chunks are padded to even sizes.
        pos = body_end + (size & 1);
    }
    let data = data.ok_or_else(|| "no data chunk".to_string())?;

    match (format_tag, bits_per_sample) {
        (1, 16) => Ok(data
            .chunks_exact(2)
            .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0)
            .collect()),
        (1, 24) => Ok(data
            .chunks_exact(3)
            .map(|c| {
                let sign = if c[2] & 0x80 != 0 { 0xFF } else { 0x00 };
                let s = i32::from_le_bytes([c[0], c[1], c[2], sign]);
                s as f32 / 8_388_608.0
            })
            .collect()),
        (1, 32) => Ok(data
            .chunks_exact(4)
            .map(|c| {
                let s = i32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                s as f32 / 2_147_483_648.0
            })
            .collect()),
        (3, 32) => Ok(data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        (tag, bps) => Err(format!("unsupported WAV format tag={tag} bps={bps}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_wav_f32_round_trips_through_generator() {
        let dir = std::env::temp_dir().join("qobee-diag-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seed.wav");
        generate_test_wav(&path, 1234).unwrap();
        let samples = read_wav_f32(&path).unwrap();
        // 5 s × 44_100 frames × 2 channels.
        assert_eq!(samples.len(), 5 * 44_100 * 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn null_test_conclusion_matches_diff_count() {
        let stats_zero = NullTestStats {
            peak_diff_dbfs: f32::NEG_INFINITY,
            rms_diff_dbfs: f32::NEG_INFINITY,
            samples_diff_count: 0,
            aligned_frames: 100,
        };
        let report = NullTestReport::from_stats(
            EffectiveOutputMode::Shared,
            NullTestSource::Loopback,
            stats_zero,
        );
        assert_eq!(report.conclusion, NullTestConclusion::BitPerfect);

        let stats_one = NullTestStats {
            peak_diff_dbfs: -90.0,
            rms_diff_dbfs: -120.0,
            samples_diff_count: 1,
            aligned_frames: 100,
        };
        let report = NullTestReport::from_stats(
            EffectiveOutputMode::Shared,
            NullTestSource::Loopback,
            stats_one,
        );
        assert_eq!(report.conclusion, NullTestConclusion::Modified);
    }
}
