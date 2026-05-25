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
}

impl SymphoniaDecoder {
    /// Open `path` and prepare the first decodable audio track for
    /// streaming.
    pub fn open(path: &Path) -> EngineResult<Self> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
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
        let (duration_seconds, time_base_seconds) = match (codec_params.n_frames, codec_params.time_base) {
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

    /// Pull the next decoded packet. Returns `Ok(None)` at end of stream.
    pub fn next_packet(&mut self) -> EngineResult<Option<DecodedPacket>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
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
                    tracing::warn!(target: "qobee::engine", %msg, "decode error, skipping packet");
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
