//! Pre-render mode for the Exclusive-mode null-test branch (R11.3).
//!
//! Loopback over WASAPI is only available in Shared. When the user
//! is on Exclusive, the diagnostic instead asks the engine to render
//! the post-DSP stream into a 24-bit WAV file on disk and compares
//! that file with the source. The output bytes are exactly what the
//! engine would have written to the DAC, minus the device round-trip.
//!
//! Concretely the engine exposes two primitives here:
//!
//!   * [`PreRenderSink`] — a thread-safe handle owned by `Shared`
//!     that the decoder thread writes interleaved post-DSP chunks
//!     to. While the sink is `Some(...)`, the device side is
//!     short-circuited (the orchestrator must not also be playing
//!     to the DAC).
//!   * [`open_pre_render`] / [`finalise_pre_render`] — open / close
//!     a sink at `output_wav` (RIFF, stereo, the source's native SR,
//!     24-bit). The header's `data` size is patched on close.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use crate::error::{EngineError, EngineResult};

/// Thread-safe write sink installed on `Shared` while a pre-render
/// session is active. The decoder thread takes the lock for every
/// chunk it produces (a contented mutex is acceptable here because
/// pre-render is mutually exclusive with normal device playback).
pub struct PreRenderSink {
    inner: Mutex<Option<PreRenderInner>>,
}

struct PreRenderInner {
    writer: BufWriter<File>,
    /// Bytes written into the `data` chunk so far (excluding header).
    /// Used to back-patch the RIFF and `data` sizes in
    /// [`finalise_pre_render`].
    data_bytes: u32,
}

impl PreRenderSink {
    /// Empty sink — `is_active` returns false, every write is a
    /// no-op. The decoder thread checks this at chunk boundaries
    /// before writing.
    pub fn new() -> Self {
        PreRenderSink {
            inner: Mutex::new(None),
        }
    }

    /// True while a pre-render session is in flight. Cheap; lock
    /// contention is limited to chunk boundaries.
    pub fn is_active(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Convert each f32 sample into a 24-bit signed PCM frame and
    /// append to the file. Called from the decoder thread on every
    /// post-DSP chunk while the sink is open.
    pub fn write_chunk(&self, samples: &[f32]) {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(inner) = guard.as_mut() else { return };
        for s in samples {
            // Symmetric clamp + scale to i24, little-endian 3-byte.
            let v = s.clamp(-1.0, 1.0);
            let q = (v * 8_388_607.0).round() as i32;
            let bytes = q.to_le_bytes();
            if inner.writer.write_all(&bytes[..3]).is_err() {
                break;
            }
            inner.data_bytes = inner.data_bytes.saturating_add(3);
        }
    }
}

impl Default for PreRenderSink {
    fn default() -> Self {
        Self::new()
    }
}

/// Open `output_wav` for writing and install the writer on `sink`.
/// The header is written eagerly with placeholder sizes; the real
/// sizes are patched in [`finalise_pre_render`].
///
/// Errors out when a session is already in flight, so the caller
/// (orchestrator) can refuse to start two pre-render sessions in
/// parallel and avoid clobbering each other's output file.
pub fn open_pre_render(
    sink: &PreRenderSink,
    output_wav: &Path,
    sample_rate: u32,
    channels: u16,
) -> EngineResult<()> {
    if let Some(parent) = output_wav.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut guard = sink.inner.lock().map_err(|_| {
        EngineError::Internal("pre-render sink mutex poisoned".to_string())
    })?;
    if guard.is_some() {
        return Err(EngineError::InvalidState(
            "pre-render session already active",
        ));
    }
    let f = File::create(output_wav)?;
    let mut w = BufWriter::new(f);
    write_wav_header_24bit(&mut w, sample_rate, channels, 0)?;
    *guard = Some(PreRenderInner {
        writer: w,
        data_bytes: 0,
    });
    Ok(())
}

/// Close the active pre-render session: flush, patch the RIFF
/// header sizes with the actual byte count, and drop the writer.
/// Returns `Ok(())` even when no session is active so callers can
/// invoke this unconditionally on cleanup.
pub fn finalise_pre_render(sink: &PreRenderSink) -> EngineResult<()> {
    let Some(mut inner) = sink
        .inner
        .lock()
        .map_err(|_| EngineError::Internal("pre-render sink mutex poisoned".to_string()))?
        .take()
    else {
        return Ok(());
    };
    inner.writer.flush()?;
    let mut file = inner.writer.into_inner().map_err(|e| {
        EngineError::Internal(format!("BufWriter::into_inner: {}", e.error()))
    })?;
    // Patch RIFF and data sizes.
    let data_bytes = inner.data_bytes;
    let riff_size = 36u32.saturating_add(data_bytes);
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.seek(SeekFrom::Start(40))?;
    file.write_all(&data_bytes.to_le_bytes())?;
    file.flush()?;
    Ok(())
}

fn write_wav_header_24bit(
    w: &mut BufWriter<File>,
    sample_rate: u32,
    channels: u16,
    data_bytes: u32,
) -> EngineResult<()> {
    let bits_per_sample: u16 = 24;
    let byte_rate: u32 =
        sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let block_align: u16 = channels * (bits_per_sample / 8);
    let riff_size: u32 = 36u32.saturating_add(data_bytes);

    w.write_all(b"RIFF")?;
    w.write_all(&riff_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits_per_sample.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_sink_is_inactive_and_swallows_writes() {
        let sink = PreRenderSink::new();
        assert!(!sink.is_active());
        sink.write_chunk(&[0.1, -0.2, 0.3, -0.4]);
        // Still inactive after a no-op write.
        assert!(!sink.is_active());
    }

    #[test]
    fn open_and_finalise_writes_a_well_formed_wav() {
        let dir = std::env::temp_dir().join("qobee-pre-render-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.wav");

        let sink = PreRenderSink::new();
        open_pre_render(&sink, &path, 44_100, 2).unwrap();
        sink.write_chunk(&[0.5, -0.5, 0.25, -0.25]); // 4 samples × 3 bytes
        finalise_pre_render(&sink).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        // 44 byte header + 4 × 3 byte samples = 56 bytes.
        assert_eq!(bytes.len(), 44 + 12);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        // `data` size patched correctly.
        let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data_size, 12);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cannot_open_two_sessions_in_parallel() {
        let dir = std::env::temp_dir().join("qobee-pre-render-test-2");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.wav");
        let sink = PreRenderSink::new();
        open_pre_render(&sink, &path, 44_100, 2).unwrap();
        let err = open_pre_render(&sink, &path, 44_100, 2);
        assert!(err.is_err());
        finalise_pre_render(&sink).unwrap();
        let _ = std::fs::remove_file(&path);
    }
}
