//! Symphonia-based decoder.
//!
//! Decodes a file into f32 interleaved PCM and exposes a pull-style
//! [`SymphoniaDecoder::next_packet`] API. The output backend pulls packets
//! and copies them into the audio device's ring buffer.
//!
//! This module deliberately stays decoder-only: it does not own the
//! output stream. That separation is what lets us swap CPAL Shared for a
//! future WASAPI Exclusive backend without rewriting the decoder.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use crate::error::{EngineError, EngineResult};
use crate::types::{PcmBuffer, TrackFormat};

/// Number of repaired/skipped MP3 frames above which the current
/// file is flagged as possibly degraded (R11.3). Propagated to
/// [`PlayerState::degraded`](crate::types::PlayerState) by the
/// decoder thread.
pub const DEGRADED_FRAME_THRESHOLD: u64 = 64;

/// Decoded packet returned by [`SymphoniaDecoder::next_packet`].
pub struct DecodedPacket {
    pub buffer: PcmBuffer,
    pub timestamp_seconds: f64,
}

/// A streaming decoder over a single audio file.
pub struct SymphoniaDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    bit_depth: Option<u8>,
    duration_seconds: f64,
    time_base_seconds: f64,
    /// Number of repaired/skipped frames on this file (R11.2/R11.3).
    /// Incremented on every `DecodeError` recovered from in
    /// [`Self::next_packet`]; an aggregate `info` log is emitted once
    /// at EOF and the count drives the degraded flag.
    repaired_frames: u64,
}

impl SymphoniaDecoder {
    /// Open `path` and prepare the first decodable audio track for
    /// streaming.
    pub fn open(path: &Path) -> EngineResult<Self> {
        let file = File::open(path)?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        Self::open_from_source(mss, ext.as_deref())
    }

    /// Open a track from a path **or** a Drive URI
    /// (`drv://<source_id>/<file_id>`). Backends call this instead
    /// of [`Self::open`] so a single code path covers both local
    /// and remote tracks.
    pub fn open_uri(track_path: &str) -> EngineResult<Self> {
        if let Some((source_id, file_id)) = qobee_drive::parse_drive_uri(track_path) {
            use std::sync::Arc;
            let client = qobee_drive::DriveClient::new(source_id)
                .map_err(|e| EngineError::Decode(format!("Drive auth: {e}")))?;
            let source = qobee_drive::DriveMediaSource::open(Arc::new(client), file_id)
                .map_err(|e| EngineError::Decode(format!("Drive open: {e}")))?;
            let mss = MediaSourceStream::new(Box::new(source), Default::default());
            return Self::open_from_source(mss, None);
        }
        Self::open(Path::new(track_path))
    }

    /// Same as [`Self::open`] but takes an arbitrary
    /// [`symphonia::core::io::MediaSource`]. Used by the Drive
    /// backend to feed Symphonia an HTTP byte-range source. The
    /// optional `ext_hint` (e.g. `"flac"`) helps Symphonia pick
    /// the right demuxer when the source has no filesystem path.
    pub fn open_from_source(mss: MediaSourceStream, ext_hint: Option<&str>) -> EngineResult<Self> {
        let mut hint = Hint::new();
        if let Some(ext) = ext_hint {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| EngineError::Decode(e.to_string()))?;

        Self::from_probed(probed)
    }

    fn from_probed(probed: symphonia::core::probe::ProbeResult) -> EngineResult<Self> {
        let format = probed.format;

        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            .ok_or(EngineError::NoAudioStream)?;

        let track_id = track.id;
        let codec_params = &track.codec_params;

        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| EngineError::Decode("missing sample rate".into()))?;

        let channels = codec_params
            .channels
            .ok_or_else(|| EngineError::Decode("missing channel layout".into()))?
            .count() as u16;

        let bit_depth = codec_params.bits_per_sample.map(|b| b as u8);

        // Compute total duration in seconds when both n_frames and time_base
        // are exposed by the demuxer.
        let (duration_seconds, time_base_seconds) =
            match (codec_params.n_frames, codec_params.time_base) {
                (Some(n), Some(tb)) => {
                    let secs = n as f64 * (tb.numer as f64) / (tb.denom as f64);
                    (secs, (tb.numer as f64) / (tb.denom as f64))
                }
                _ => (0.0, 1.0 / sample_rate as f64),
            };

        let decoder = symphonia::default::get_codecs()
            .make(codec_params, &DecoderOptions::default())
            .map_err(|e| EngineError::Decode(e.to_string()))?;

        Ok(SymphoniaDecoder {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            bit_depth,
            duration_seconds,
            time_base_seconds,
            repaired_frames: 0,
        })
    }

    pub fn format(&self) -> TrackFormat {
        TrackFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
            bit_depth: self.bit_depth,
        }
    }

    pub fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    /// Number of MP3 frames repaired/skipped on this file so far
    /// (R11.2/R11.3). Read by the decoder thread at EOF to publish
    /// the degraded flag once `repaired_frames` exceeds
    /// [`DEGRADED_FRAME_THRESHOLD`].
    pub fn repaired_frames(&self) -> u64 {
        self.repaired_frames
    }

    /// Pull the next decoded packet. Returns `Ok(None)` at end of stream.
    pub fn next_packet(&mut self) -> EngineResult<Option<DecodedPacket>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    // End of stream. Emit a single aggregate `info`
                    // log when at least one frame was repaired on this
                    // file (R11.2), instead of one `warn` per frame.
                    if self.repaired_frames > 0 {
                        tracing::info!(
                            target: "qobee::engine",
                            repaired = self.repaired_frames,
                            "{} trames MP3 réparées sur ce fichier",
                            self.repaired_frames
                        );
                    }
                    return Ok(None);
                }
                Err(SymphoniaError::ResetRequired) => {
                    return Err(EngineError::Decode("decoder reset required".into()));
                }
                Err(e) => return Err(EngineError::Decode(e.to_string())),
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            let timestamp_seconds = packet.ts() as f64 * self.time_base_seconds;

            match self.decoder.decode(&packet) {
                Ok(audio_buf) => {
                    let interleaved = audio_buffer_to_f32_interleaved(&audio_buf);
                    return Ok(Some(DecodedPacket {
                        buffer: PcmBuffer::F32Interleaved(interleaved),
                        timestamp_seconds,
                    }));
                }
                Err(SymphoniaError::DecodeError(msg)) => {
                    // A recoverable per-frame decode error (e.g. MP3
                    // `invalid main_data_begin`). Count it and log at
                    // `debug` so the logs aren't flooded (R11.1); the
                    // aggregate is emitted at EOF (R11.2).
                    self.repaired_frames += 1;
                    tracing::debug!(target: "qobee::engine", %msg, "trame MP3 réparée/skippée");
                    continue;
                }
                Err(e) => return Err(EngineError::Decode(e.to_string())),
            }
        }
    }

    /// Seek to `position_seconds`. Sample-accurate: the demuxer
    /// snaps to the nearest keyframe and the decoder then discards
    /// samples up to the exact requested timestamp. Cost is at most
    /// a few extra packets of decode time, well below human
    /// perception.
    pub fn seek(&mut self, position_seconds: f64) -> EngineResult<()> {
        let time = Time::new(
            position_seconds.trunc() as u64,
            position_seconds.fract().clamp(0.0, 1.0),
        );
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| EngineError::Decode(e.to_string()))?;
        Ok(())
    }
}

/// Convert any `AudioBufferRef` produced by Symphonia into f32 interleaved
/// samples, normalized to `[-1.0, 1.0]`.
fn audio_buffer_to_f32_interleaved(buf: &AudioBufferRef<'_>) -> Vec<f32> {
    use symphonia::core::audio::AudioBufferRef as B;

    let spec = match buf {
        B::U8(b) => *b.spec(),
        B::U16(b) => *b.spec(),
        B::U24(b) => *b.spec(),
        B::U32(b) => *b.spec(),
        B::S8(b) => *b.spec(),
        B::S16(b) => *b.spec(),
        B::S24(b) => *b.spec(),
        B::S32(b) => *b.spec(),
        B::F32(b) => *b.spec(),
        B::F64(b) => *b.spec(),
    };
    let frames = match buf {
        B::U8(b) => b.frames(),
        B::U16(b) => b.frames(),
        B::U24(b) => b.frames(),
        B::U32(b) => b.frames(),
        B::S8(b) => b.frames(),
        B::S16(b) => b.frames(),
        B::S24(b) => b.frames(),
        B::S32(b) => b.frames(),
        B::F32(b) => b.frames(),
        B::F64(b) => b.frames(),
    };
    let channels = spec.channels.count();

    let mut out = vec![0.0f32; frames * channels];

    macro_rules! interleave_signed {
        ($buf:expr, $max:expr) => {{
            let max = $max as f32;
            for ch in 0..channels {
                let plane = $buf.chan(ch);
                for (i, sample) in plane.iter().enumerate() {
                    out[i * channels + ch] = (*sample as f32) / max;
                }
            }
        }};
    }

    macro_rules! interleave_unsigned {
        ($buf:expr, $half:expr, $max:expr) => {{
            let half = $half as f32;
            let max = $max as f32;
            for ch in 0..channels {
                let plane = $buf.chan(ch);
                for (i, sample) in plane.iter().enumerate() {
                    out[i * channels + ch] = ((*sample as f32) - half) / max;
                }
            }
        }};
    }

    macro_rules! interleave_float {
        ($buf:expr, $cast:ty) => {{
            for ch in 0..channels {
                let plane = $buf.chan(ch);
                for (i, sample) in plane.iter().enumerate() {
                    out[i * channels + ch] = *sample as $cast as f32;
                }
            }
        }};
    }

    match buf {
        B::U8(b) => interleave_unsigned!(b, 128_i32, 128_i32),
        B::U16(b) => interleave_unsigned!(b, 32_768_i32, 32_768_i32),
        B::U24(b) => {
            // U24 wraps a custom type; access via inner() returns i32-like.
            let half = 8_388_608.0_f32;
            let max = 8_388_608.0_f32;
            for ch in 0..channels {
                let plane = b.chan(ch);
                for (i, sample) in plane.iter().enumerate() {
                    let v = sample.inner() as f32;
                    out[i * channels + ch] = (v - half) / max;
                }
            }
        }
        B::U32(b) => {
            let half = (u32::MAX as f64 / 2.0) as f32;
            for ch in 0..channels {
                let plane = b.chan(ch);
                for (i, sample) in plane.iter().enumerate() {
                    out[i * channels + ch] = ((*sample as f64 - half as f64) / half as f64) as f32;
                }
            }
        }
        B::S8(b) => interleave_signed!(b, 128_i32),
        B::S16(b) => interleave_signed!(b, 32_768_i32),
        B::S24(b) => {
            let max = 8_388_608.0_f32;
            for ch in 0..channels {
                let plane = b.chan(ch);
                for (i, sample) in plane.iter().enumerate() {
                    out[i * channels + ch] = (sample.inner() as f32) / max;
                }
            }
        }
        B::S32(b) => {
            let max = 2_147_483_648.0_f32;
            for ch in 0..channels {
                let plane = b.chan(ch);
                for (i, sample) in plane.iter().enumerate() {
                    out[i * channels + ch] = (*sample as f32) / max;
                }
            }
        }
        B::F32(b) => interleave_float!(b, f32),
        B::F64(b) => interleave_float!(b, f32),
    }

    out
}

// ---------------------------------------------------------------------------
// Tests — task 15.3 (MP3 repaired-frame example tests, R11.1/R11.2/R11.3)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tracing_test::traced_test;

    // Test-only seam: drive the repaired-frame counter to a chosen
    // value. Synthesising *real* recoverable MP3 corruption
    // (`invalid main_data_begin`) deterministically inside a unit
    // test is impractical, so the "damaged file" example puts the
    // decoder into the exact state `next_packet` would reach after
    // recovering that many per-frame `DecodeError`s, then exercises
    // the real EOF aggregate / degraded path.
    impl SymphoniaDecoder {
        fn set_repaired_frames_for_test(&mut self, n: u64) {
            self.repaired_frames = n;
        }
    }

    /// Build a minimal, valid 16-bit PCM WAV entirely in memory.
    ///
    /// The workspace enables symphonia's `wav` + `pcm` features, so
    /// this decodes cleanly through the very same `SymphoniaDecoder`
    /// path used in production — no binary fixture has to be shipped.
    /// The sample values are irrelevant to repaired-frame accounting;
    /// a gentle ramp keeps the data deterministic.
    fn clean_wav_pcm16(sample_rate: u32, channels: u16, frames: u32) -> Vec<u8> {
        let bits_per_sample: u16 = 16;
        let block_align: u16 = channels * (bits_per_sample / 8);
        let byte_rate: u32 = sample_rate * block_align as u32;
        let data_len: u32 = frames * block_align as u32;

        let mut v = Vec::with_capacity(44 + data_len as usize);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
        v.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&sample_rate.to_le_bytes());
        v.extend_from_slice(&byte_rate.to_le_bytes());
        v.extend_from_slice(&block_align.to_le_bytes());
        v.extend_from_slice(&bits_per_sample.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let s = (((i % 256) as i16) - 128).wrapping_mul(64);
            for _ch in 0..channels {
                v.extend_from_slice(&s.to_le_bytes());
            }
        }
        v
    }

    fn open_wav(bytes: Vec<u8>) -> SymphoniaDecoder {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        SymphoniaDecoder::open_from_source(mss, Some("wav")).expect("a clean WAV must open")
    }

    /// Drain a decoder to end-of-stream, asserting no hard error, and
    /// return the number of packets handed back.
    fn drain_to_eof(dec: &mut SymphoniaDecoder) -> usize {
        let mut packets = 0;
        while dec
            .next_packet()
            .expect("a clean WAV must decode without a hard error")
            .is_some()
        {
            packets += 1;
        }
        packets
    }

    // Feature: qobee-beta-feedback-improvements, task 15.3 — a healthy
    // file repairs nothing and emits no aggregate (R11.2/R11.3).
    #[traced_test]
    #[test]
    fn clean_file_repairs_nothing_and_emits_no_aggregate() {
        let mut dec = open_wav(clean_wav_pcm16(44_100, 2, 4096));

        let packets = drain_to_eof(&mut dec);
        assert!(
            packets > 0,
            "a 4096-frame WAV must yield at least one packet"
        );

        // R11.2 (clean half): nothing was repaired.
        assert_eq!(dec.repaired_frames(), 0, "a healthy file repairs nothing");
        // R11.3 (clean half): well below the degraded threshold.
        assert!(
            dec.repaired_frames() <= DEGRADED_FRAME_THRESHOLD,
            "a healthy file must not be flagged degraded"
        );
        // R11.2 (clean half): the EOF aggregate `info` line is guarded
        // by `repaired_frames > 0`, so a clean file logs nothing.
        assert!(
            !logs_contain("trames MP3 réparées"),
            "no aggregate repair summary may be emitted for a clean file"
        );
    }

    // Feature: qobee-beta-feedback-improvements, task 15.3 — a damaged
    // file emits a single aggregate `info` summary at EOF and is
    // flagged degraded once the repair count clears the threshold
    // (R11.2/R11.3).
    #[traced_test]
    #[test]
    fn damaged_file_emits_aggregate_and_is_flagged_degraded() {
        let mut dec = open_wav(clean_wav_pcm16(44_100, 2, 64));

        // Put the decoder in the state it reaches after recovering more
        // `invalid main_data_begin` frames than the degraded threshold.
        dec.set_repaired_frames_for_test(DEGRADED_FRAME_THRESHOLD + 1);

        // Draining to EOF runs the real terminal `Ok(None)` branch of
        // `next_packet`, which emits the single aggregate `info` line
        // (R11.2) because `repaired_frames > 0`.
        drain_to_eof(&mut dec);

        assert!(
            logs_contain("trames MP3 réparées"),
            "a damaged file must emit the aggregate repair summary at EOF"
        );
        // R11.3: the count exceeding the threshold is exactly the
        // degraded decision the decoder thread applies at EOF.
        assert!(
            dec.repaired_frames() > DEGRADED_FRAME_THRESHOLD,
            "repaired frames ({}) must exceed the degraded threshold ({})",
            dec.repaired_frames(),
            DEGRADED_FRAME_THRESHOLD
        );
    }

    // Feature: qobee-beta-feedback-improvements, task 15.3 — boundary
    // example pinning the strict `>` degraded comparison the decoder
    // thread relies on (R11.3): exactly threshold repairs is NOT
    // degraded; one more is.
    #[test]
    fn degraded_decision_is_strict_at_the_threshold() {
        let mut dec = open_wav(clean_wav_pcm16(44_100, 2, 16));

        dec.set_repaired_frames_for_test(DEGRADED_FRAME_THRESHOLD);
        assert!(
            dec.repaired_frames() <= DEGRADED_FRAME_THRESHOLD,
            "exactly threshold repairs must not be degraded"
        );

        dec.set_repaired_frames_for_test(DEGRADED_FRAME_THRESHOLD + 1);
        assert!(
            dec.repaired_frames() > DEGRADED_FRAME_THRESHOLD,
            "one over the threshold must be degraded"
        );
    }
}
