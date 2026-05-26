//! DSF (DSD Stream File) parser + writer.
//!
//! The DSF container layout used here:
//!
//! ```text
//! DSD chunk  (28 bytes):  "DSD " | chunk_size(8) | file_size(8) | metadata_ptr(8)
//! fmt chunk  (52 bytes):  "fmt " | chunk_size(8) | format_version(4)
//!                          | format_id(4) | channel_type(4) | channel_num(4)
//!                          | sampling_frequency(4) | bits_per_sample(4)
//!                          | sample_count(8) | block_size_per_channel(4)
//!                          | reserved(4)
//! data chunk:             "data" | chunk_size(8) | <interleaved-by-block bytes>
//! ```
//!
//! Audio bytes are stored block-interleaved: a fixed `block_size`
//! window (4 096 bytes per channel) for channel 0, then the same
//! block for channel 1, …, repeating until the whole stream is
//! emitted. DSF uses LSB-first bit order: bit 0 of each byte is the
//! oldest DSD bit in time. The parser deinterleaves the blocks into
//! one `Vec<u8>` per channel and stores `lsb_first = true`.
//!
//! `read_dsf` returns an `EngineError::DsdInvalidFile` on any
//! structural mismatch (wrong magic, truncated chunk, unsupported
//! sampling frequency). `write_dsf` is the matching encoder used by
//! the property tests (Property 14, R7.9): a `DsdStream` written
//! then read back yields an equal `DsdStream` sample-for-sample.

use std::io::{Read, Seek, SeekFrom, Write};

use crate::error::{EngineError, EngineResult};

use super::{DsdRate, DsdStream};

/// Block size used when interleaving channels in the data chunk.
/// The DSF spec fixes this at 4 096 bytes per channel; we honour it
/// in both the reader and the writer.
const BLOCK_SIZE: usize = 4096;

/// Read a DSF file from `r`, returning a fully-buffered
/// [`DsdStream`]. The reader is consumed up to the end of the data
/// chunk; trailing ID3v2 metadata (if any) is ignored.
pub fn read_dsf<R: Read + Seek>(mut r: R) -> EngineResult<DsdStream> {
    // -------- DSD chunk (28 bytes) --------
    let mut header = [0u8; 28];
    r.read_exact(&mut header)
        .map_err(|e| EngineError::DsdInvalidFile(format!("DSD header: {e}")))?;
    if &header[0..4] != b"DSD " {
        return Err(EngineError::DsdInvalidFile(format!(
            "expected 'DSD ' magic, got {:?}",
            &header[0..4]
        )));
    }
    let dsd_chunk_size = u64::from_le_bytes(header[4..12].try_into().unwrap());
    if dsd_chunk_size != 28 {
        return Err(EngineError::DsdInvalidFile(format!(
            "DSD chunk size = {dsd_chunk_size}, expected 28"
        )));
    }
    // header[12..20] = file size (advisory), header[20..28] = metadata ptr.

    // -------- fmt chunk --------
    let mut fmt_header = [0u8; 12];
    r.read_exact(&mut fmt_header)
        .map_err(|e| EngineError::DsdInvalidFile(format!("fmt header: {e}")))?;
    if &fmt_header[0..4] != b"fmt " {
        return Err(EngineError::DsdInvalidFile(format!(
            "expected 'fmt ' magic, got {:?}",
            &fmt_header[0..4]
        )));
    }
    let fmt_chunk_size = u64::from_le_bytes(fmt_header[4..12].try_into().unwrap());
    if fmt_chunk_size < 40 {
        return Err(EngineError::DsdInvalidFile(format!(
            "fmt chunk too small ({fmt_chunk_size})"
        )));
    }
    // The fmt body is `fmt_chunk_size - 12` bytes; canonical DSF
    // uses 52, leaving 40 bytes after the 12-byte header. We read
    // exactly 40 then skip any padding.
    let mut fmt_body = [0u8; 40];
    r.read_exact(&mut fmt_body)
        .map_err(|e| EngineError::DsdInvalidFile(format!("fmt body: {e}")))?;
    let extra = (fmt_chunk_size as usize).saturating_sub(52);
    if extra > 0 {
        r.seek(SeekFrom::Current(extra as i64))
            .map_err(|e| EngineError::DsdInvalidFile(format!("fmt padding: {e}")))?;
    }

    let _format_version = u32::from_le_bytes(fmt_body[0..4].try_into().unwrap());
    let _format_id = u32::from_le_bytes(fmt_body[4..8].try_into().unwrap());
    let _channel_type = u32::from_le_bytes(fmt_body[8..12].try_into().unwrap());
    let channel_num = u32::from_le_bytes(fmt_body[12..16].try_into().unwrap());
    let sampling_frequency = u32::from_le_bytes(fmt_body[16..20].try_into().unwrap());
    let bits_per_sample = u32::from_le_bytes(fmt_body[20..24].try_into().unwrap());
    let sample_count = u64::from_le_bytes(fmt_body[24..32].try_into().unwrap());
    let block_size_per_channel = u32::from_le_bytes(fmt_body[32..36].try_into().unwrap());
    // fmt_body[36..40] = reserved.

    if channel_num == 0 || channel_num > 32 {
        return Err(EngineError::DsdInvalidFile(format!(
            "unsupported channel_num: {channel_num}"
        )));
    }
    let rate = DsdRate::from_hz_bits(sampling_frequency, 1).ok_or_else(|| {
        EngineError::DsdInvalidFile(format!(
            "unsupported sampling_frequency: {sampling_frequency} Hz"
        ))
    })?;
    let lsb_first = match bits_per_sample {
        1 => true,
        8 => false,
        other => {
            return Err(EngineError::DsdInvalidFile(format!(
                "unsupported bits_per_sample: {other}"
            )))
        }
    };
    let block = block_size_per_channel as usize;
    if block == 0 {
        return Err(EngineError::DsdInvalidFile(
            "block_size_per_channel = 0".into(),
        ));
    }

    // -------- data chunk --------
    let mut data_header = [0u8; 12];
    r.read_exact(&mut data_header)
        .map_err(|e| EngineError::DsdInvalidFile(format!("data header: {e}")))?;
    if &data_header[0..4] != b"data" {
        return Err(EngineError::DsdInvalidFile(format!(
            "expected 'data' magic, got {:?}",
            &data_header[0..4]
        )));
    }
    let data_chunk_size = u64::from_le_bytes(data_header[4..12].try_into().unwrap());
    let payload_len = (data_chunk_size as usize).saturating_sub(12);

    // Bytes per channel = ceil(sample_count / 8) padded up to a
    // multiple of `block`.
    let bytes_per_ch_unpadded = ((sample_count + 7) / 8) as usize;
    let blocks_per_ch = (bytes_per_ch_unpadded + block - 1) / block;
    let bytes_per_ch_padded = blocks_per_ch * block;
    let total_expected = bytes_per_ch_padded * channel_num as usize;
    if payload_len < total_expected {
        return Err(EngineError::DsdInvalidFile(format!(
            "data payload {payload_len} < expected {total_expected}"
        )));
    }

    let mut payload = vec![0u8; total_expected];
    r.read_exact(&mut payload)
        .map_err(|e| EngineError::DsdInvalidFile(format!("data payload: {e}")))?;

    let mut bytes_per_channel: Vec<Vec<u8>> =
        (0..channel_num).map(|_| Vec::with_capacity(bytes_per_ch_unpadded)).collect();
    for blk in 0..blocks_per_ch {
        for c in 0..channel_num as usize {
            let offset = (blk * channel_num as usize + c) * block;
            let take = bytes_per_ch_unpadded.saturating_sub(blk * block).min(block);
            if take > 0 {
                bytes_per_channel[c].extend_from_slice(&payload[offset..offset + take]);
            }
        }
    }

    Ok(DsdStream {
        rate,
        channels: channel_num as u16,
        bytes_per_channel,
        lsb_first,
    })
}

/// Encode `stream` into a DSF byte stream on `w`. The output is
/// parser-symmetric: `read_dsf(write_dsf(s)) == s` for any
/// well-formed `DsdStream`.
pub fn write_dsf<W: Write>(stream: &DsdStream, w: &mut W) -> EngineResult<()> {
    if stream.channels == 0 || stream.bytes_per_channel.len() != stream.channels as usize {
        return Err(EngineError::DsdInvalidFile(
            "channels mismatch in DsdStream".into(),
        ));
    }
    let bytes_per_ch_unpadded = stream.bytes_per_channel[0].len();
    if stream
        .bytes_per_channel
        .iter()
        .any(|c| c.len() != bytes_per_ch_unpadded)
    {
        return Err(EngineError::DsdInvalidFile(
            "channels have differing lengths".into(),
        ));
    }
    let block = BLOCK_SIZE;
    let blocks_per_ch = (bytes_per_ch_unpadded + block - 1) / block;
    let bytes_per_ch_padded = blocks_per_ch * block;
    let total_payload = bytes_per_ch_padded * stream.channels as usize;
    let data_chunk_size = (12 + total_payload) as u64;
    let fmt_chunk_size: u64 = 52;
    let dsd_chunk_size: u64 = 28;
    let file_size = dsd_chunk_size + fmt_chunk_size + data_chunk_size;

    // -------- DSD chunk --------
    w.write_all(b"DSD ")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write DSD: {e}")))?;
    w.write_all(&dsd_chunk_size.to_le_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write DSD size: {e}")))?;
    w.write_all(&file_size.to_le_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write file size: {e}")))?;
    // metadata pointer = 0 (no ID3v2 trailer).
    w.write_all(&0u64.to_le_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write meta ptr: {e}")))?;

    // -------- fmt chunk --------
    w.write_all(b"fmt ")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write fmt: {e}")))?;
    w.write_all(&fmt_chunk_size.to_le_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write fmt size: {e}")))?;
    let format_version: u32 = 1;
    let format_id: u32 = 0; // DSD raw
    let channel_type: u32 = if stream.channels == 1 { 1 } else { 2 };
    let channel_num: u32 = stream.channels as u32;
    let sampling_frequency: u32 = stream.rate.hz();
    let bits_per_sample: u32 = if stream.lsb_first { 1 } else { 8 };
    let sample_count: u64 = (bytes_per_ch_unpadded as u64) * 8;
    let block_size_per_channel: u32 = block as u32;
    let reserved: u32 = 0;
    let mut fmt_body = [0u8; 40];
    fmt_body[0..4].copy_from_slice(&format_version.to_le_bytes());
    fmt_body[4..8].copy_from_slice(&format_id.to_le_bytes());
    fmt_body[8..12].copy_from_slice(&channel_type.to_le_bytes());
    fmt_body[12..16].copy_from_slice(&channel_num.to_le_bytes());
    fmt_body[16..20].copy_from_slice(&sampling_frequency.to_le_bytes());
    fmt_body[20..24].copy_from_slice(&bits_per_sample.to_le_bytes());
    fmt_body[24..32].copy_from_slice(&sample_count.to_le_bytes());
    fmt_body[32..36].copy_from_slice(&block_size_per_channel.to_le_bytes());
    fmt_body[36..40].copy_from_slice(&reserved.to_le_bytes());
    w.write_all(&fmt_body)
        .map_err(|e| EngineError::DsdInvalidFile(format!("write fmt body: {e}")))?;

    // -------- data chunk --------
    w.write_all(b"data")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write data: {e}")))?;
    w.write_all(&data_chunk_size.to_le_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write data size: {e}")))?;

    let zero_pad = vec![0u8; block];
    for blk in 0..blocks_per_ch {
        for c in 0..stream.channels as usize {
            let start = blk * block;
            let take = bytes_per_ch_unpadded.saturating_sub(start).min(block);
            if take > 0 {
                w.write_all(&stream.bytes_per_channel[c][start..start + take])
                    .map_err(|e| EngineError::DsdInvalidFile(format!("write block: {e}")))?;
            }
            if take < block {
                w.write_all(&zero_pad[..block - take]).map_err(|e| {
                    EngineError::DsdInvalidFile(format!("write block pad: {e}"))
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_stream(channels: u16, frames_per_channel: usize) -> DsdStream {
        let bytes: Vec<Vec<u8>> = (0..channels as usize)
            .map(|c| {
                (0..frames_per_channel)
                    .map(|i| ((c * 31 + i * 17) & 0xFF) as u8)
                    .collect()
            })
            .collect();
        DsdStream {
            rate: DsdRate::Dsd64,
            channels,
            bytes_per_channel: bytes,
            lsb_first: true,
        }
    }

    #[test]
    fn dsf_roundtrip_stereo() {
        let s = sample_stream(2, 4096 + 1234);
        let mut buf = Vec::<u8>::new();
        write_dsf(&s, &mut buf).expect("write");
        let parsed = read_dsf(Cursor::new(buf)).expect("read");
        assert_eq!(parsed, s);
    }

    #[test]
    fn dsf_roundtrip_mono_short() {
        let s = sample_stream(1, 100);
        let mut buf = Vec::<u8>::new();
        write_dsf(&s, &mut buf).expect("write");
        let parsed = read_dsf(Cursor::new(buf)).expect("read");
        assert_eq!(parsed, s);
    }

    #[test]
    fn dsf_rejects_bad_magic() {
        let mut buf = vec![0u8; 28];
        buf[0..4].copy_from_slice(b"NOPE");
        let err = read_dsf(Cursor::new(buf)).unwrap_err();
        match err {
            EngineError::DsdInvalidFile(_) => {}
            other => panic!("expected DsdInvalidFile, got {other:?}"),
        }
    }
}
