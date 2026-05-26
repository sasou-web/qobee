//! DFF / DSDIFF (DSD Interchange File Format) parser + writer.
//!
//! DFF wraps DSD payloads in an IFF/FORM container (big-endian
//! 32-bit chunk sizes; the outer chunk is `FRM8` with a 64-bit size
//! to lift the 4 GiB ceiling). This parser handles the subset Qobee
//! needs to play SACD rips:
//!
//! ```text
//! FRM8 / size(8) / "DSD "                  ;; outer DSDIFF form
//!   FVER / size(4) / version(4)            ;; format version
//!   PROP / size(4) / "SND "                ;; property container
//!     FS   / size(4) / sample_rate(4)
//!     CHNL / size(4) / num_channels(2) | ids…
//!   DSD  / size(8) / <interleaved bytes>   ;; raw audio
//!   DST  / size(8) / …                     ;; (refused: compression)
//! ```
//!
//! DFF stores DSD bits MSB-first within each byte, and audio bytes
//! are interleaved frame-by-frame across channels (one byte per
//! channel per group, no block-windowing). The parser deinterleaves
//! the byte stream into one `Vec<u8>` per channel and stores
//! `lsb_first = false`.
//!
//! `DST`-compressed payloads are not supported and are rejected with
//! `EngineError::Decode("DST compression non supporté")` — matching
//! the design's explicit refusal rule.

use std::io::{Read, Seek, SeekFrom, Write};

use crate::error::{EngineError, EngineResult};

use super::{DsdRate, DsdStream};

/// Read a DFF file from `r` into an in-memory [`DsdStream`].
pub fn read_dff<R: Read + Seek>(mut r: R) -> EngineResult<DsdStream> {
    // -------- FRM8 outer chunk --------
    let mut frm8_id = [0u8; 4];
    r.read_exact(&mut frm8_id)
        .map_err(|e| EngineError::DsdInvalidFile(format!("FRM8: {e}")))?;
    if &frm8_id != b"FRM8" {
        return Err(EngineError::DsdInvalidFile(format!(
            "expected FRM8 magic, got {:?}",
            frm8_id
        )));
    }
    let mut frm8_size_buf = [0u8; 8];
    r.read_exact(&mut frm8_size_buf)
        .map_err(|e| EngineError::DsdInvalidFile(format!("FRM8 size: {e}")))?;
    let _frm8_size = u64::from_be_bytes(frm8_size_buf);
    let mut form_type = [0u8; 4];
    r.read_exact(&mut form_type)
        .map_err(|e| EngineError::DsdInvalidFile(format!("FRM8 form type: {e}")))?;
    if &form_type != b"DSD " {
        return Err(EngineError::DsdInvalidFile(format!(
            "expected DSD  form type, got {:?}",
            form_type
        )));
    }

    let mut sample_rate: Option<u32> = None;
    let mut channels: Option<u16> = None;
    let mut audio: Option<Vec<u8>> = None;

    // -------- inner chunks (read until the reader runs out) --------
    loop {
        let mut header = [0u8; 4];
        match r.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                return Err(EngineError::DsdInvalidFile(format!("chunk id: {e}")))
            }
        }
        // FRM8/DSD/DST carry a 64-bit size; everything else uses 32-bit.
        let size = if &header == b"FRM8" || &header == b"DSD " || &header == b"DST " {
            let mut s8 = [0u8; 8];
            r.read_exact(&mut s8)
                .map_err(|e| EngineError::DsdInvalidFile(format!("chunk size 8: {e}")))?;
            u64::from_be_bytes(s8)
        } else {
            let mut s4 = [0u8; 4];
            r.read_exact(&mut s4)
                .map_err(|e| EngineError::DsdInvalidFile(format!("chunk size 4: {e}")))?;
            u32::from_be_bytes(s4) as u64
        };

        match &header {
            b"FVER" => {
                r.seek(SeekFrom::Current(size as i64))
                    .map_err(|e| EngineError::DsdInvalidFile(format!("FVER skip: {e}")))?;
            }
            b"PROP" => {
                let mut prop_type = [0u8; 4];
                r.read_exact(&mut prop_type)
                    .map_err(|e| EngineError::DsdInvalidFile(format!("PROP type: {e}")))?;
                if &prop_type != b"SND " {
                    return Err(EngineError::DsdInvalidFile(format!(
                        "PROP type != SND : {:?}",
                        prop_type
                    )));
                }
                let mut remaining = (size as i64).saturating_sub(4);
                while remaining > 0 {
                    let mut sub_id = [0u8; 4];
                    r.read_exact(&mut sub_id).map_err(|e| {
                        EngineError::DsdInvalidFile(format!("PROP sub id: {e}"))
                    })?;
                    let mut sub_sz_buf = [0u8; 4];
                    r.read_exact(&mut sub_sz_buf).map_err(|e| {
                        EngineError::DsdInvalidFile(format!("PROP sub size: {e}"))
                    })?;
                    let sub_sz = u32::from_be_bytes(sub_sz_buf);
                    match &sub_id {
                        b"FS  " => {
                            let mut fs_buf = [0u8; 4];
                            r.read_exact(&mut fs_buf).map_err(|e| {
                                EngineError::DsdInvalidFile(format!("FS: {e}"))
                            })?;
                            sample_rate = Some(u32::from_be_bytes(fs_buf));
                            // Skip any trailing padding inside the sub-chunk.
                            let pad = (sub_sz as i64).saturating_sub(4);
                            if pad > 0 {
                                r.seek(SeekFrom::Current(pad)).map_err(|e| {
                                    EngineError::DsdInvalidFile(format!("FS pad: {e}"))
                                })?;
                            }
                        }
                        b"CHNL" => {
                            let mut nc_buf = [0u8; 2];
                            r.read_exact(&mut nc_buf).map_err(|e| {
                                EngineError::DsdInvalidFile(format!("CHNL: {e}"))
                            })?;
                            let nc = u16::from_be_bytes(nc_buf);
                            channels = Some(nc);
                            // Each channel id is 4 bytes; skip the
                            // remainder of the CHNL chunk.
                            let pad = (sub_sz as i64).saturating_sub(2);
                            if pad > 0 {
                                r.seek(SeekFrom::Current(pad)).map_err(|e| {
                                    EngineError::DsdInvalidFile(format!("CHNL pad: {e}"))
                                })?;
                            }
                        }
                        _ => {
                            r.seek(SeekFrom::Current(sub_sz as i64)).map_err(|e| {
                                EngineError::DsdInvalidFile(format!("PROP sub skip: {e}"))
                            })?;
                        }
                    }
                    // IFF chunks pad to even size.
                    let pad = sub_sz as i64 + (sub_sz as i64 & 1);
                    remaining -= 8 + pad;
                }
            }
            b"DSD " => {
                let mut buf = vec![0u8; size as usize];
                r.read_exact(&mut buf).map_err(|e| {
                    EngineError::DsdInvalidFile(format!("DSD payload: {e}"))
                })?;
                audio = Some(buf);
            }
            b"DST " => {
                return Err(EngineError::Decode(
                    "DST compression non supporté".to_string(),
                ));
            }
            _ => {
                let pad = size as i64 + (size as i64 & 1);
                r.seek(SeekFrom::Current(pad)).map_err(|e| {
                    EngineError::DsdInvalidFile(format!("unknown chunk skip: {e}"))
                })?;
                continue;
            }
        }
        // FRM8/DSD/DST round to even via their 64-bit size, but the
        // 32-bit chunks above already accounted for padding inline.
        if (&header == b"DSD " || &header == b"DST ") && (size & 1 != 0) {
            r.seek(SeekFrom::Current(1))
                .map_err(|e| EngineError::DsdInvalidFile(format!("pad: {e}")))?;
        }
    }

    let sample_rate = sample_rate
        .ok_or_else(|| EngineError::DsdInvalidFile("missing FS chunk".into()))?;
    let channels = channels
        .ok_or_else(|| EngineError::DsdInvalidFile("missing CHNL chunk".into()))?;
    let audio = audio
        .ok_or_else(|| EngineError::DsdInvalidFile("missing DSD payload".into()))?;
    let rate = DsdRate::from_hz_bits(sample_rate, 1).ok_or_else(|| {
        EngineError::DsdInvalidFile(format!("unsupported FS: {sample_rate}"))
    })?;
    if channels == 0 {
        return Err(EngineError::DsdInvalidFile("channels = 0".into()));
    }
    if audio.len() % channels as usize != 0 {
        return Err(EngineError::DsdInvalidFile(format!(
            "DSD payload {} not divisible by channels {}",
            audio.len(),
            channels
        )));
    }
    let frames_per_ch = audio.len() / channels as usize;
    let mut bytes_per_channel: Vec<Vec<u8>> = (0..channels as usize)
        .map(|_| Vec::with_capacity(frames_per_ch))
        .collect();
    for f in 0..frames_per_ch {
        for c in 0..channels as usize {
            bytes_per_channel[c].push(audio[f * channels as usize + c]);
        }
    }

    Ok(DsdStream {
        rate,
        channels,
        bytes_per_channel,
        lsb_first: false,
    })
}

/// Encode `stream` as a DFF byte stream on `w`. Symmetric to
/// `read_dff`: a round-trip yields an equal stream.
pub fn write_dff<W: Write>(stream: &DsdStream, w: &mut W) -> EngineResult<()> {
    if stream.channels == 0 || stream.bytes_per_channel.len() != stream.channels as usize {
        return Err(EngineError::DsdInvalidFile(
            "channels mismatch in DsdStream".into(),
        ));
    }
    let frames_per_ch = stream.bytes_per_channel[0].len();
    if stream
        .bytes_per_channel
        .iter()
        .any(|c| c.len() != frames_per_ch)
    {
        return Err(EngineError::DsdInvalidFile(
            "channels have differing lengths".into(),
        ));
    }

    // Inner chunks --------------------------------------------------
    // FVER (4-byte size + 4-byte version)
    let fver_body: [u8; 4] = [0x01, 0x05, 0x00, 0x00];

    // PROP/SND : "SND " + FS chunk + CHNL chunk
    //   FS   chunk: id(4) + size(4=4) + value(4)
    //   CHNL chunk: id(4) + size(4=2 + 4*nc) + nc(2) + ids(4*nc)
    let nc = stream.channels as usize;
    let chnl_body_len = 2 + 4 * nc;
    let chnl_body_len_padded = chnl_body_len + (chnl_body_len & 1);
    let prop_body_len = 4    // "SND "
        + (8 + 4)            // FS chunk header + 4-byte payload
        + (8 + chnl_body_len_padded); // CHNL chunk

    // DSD audio (size = nc * frames_per_ch).
    let audio_len = nc * frames_per_ch;
    let audio_pad = audio_len & 1;

    // FRM8 inner content: type(4) + FVER(8 + 4) + PROP(8 + body) + DSD(12 + audio + pad)
    let frm8_body_len = 4
        + (8 + 4)
        + (8 + prop_body_len)
        + (12 + audio_len + audio_pad);

    // -------- FRM8 outer chunk --------
    w.write_all(b"FRM8")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FRM8: {e}")))?;
    w.write_all(&(frm8_body_len as u64).to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FRM8 size: {e}")))?;
    w.write_all(b"DSD ")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FRM8 type: {e}")))?;

    // FVER
    w.write_all(b"FVER")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FVER: {e}")))?;
    w.write_all(&4u32.to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FVER size: {e}")))?;
    w.write_all(&fver_body)
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FVER body: {e}")))?;

    // PROP
    w.write_all(b"PROP")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write PROP: {e}")))?;
    w.write_all(&(prop_body_len as u32).to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write PROP size: {e}")))?;
    w.write_all(b"SND ")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write PROP type: {e}")))?;

    // FS sub-chunk
    w.write_all(b"FS  ")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FS: {e}")))?;
    w.write_all(&4u32.to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FS size: {e}")))?;
    w.write_all(&stream.rate.hz().to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write FS body: {e}")))?;

    // CHNL sub-chunk
    w.write_all(b"CHNL")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write CHNL: {e}")))?;
    w.write_all(&(chnl_body_len as u32).to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write CHNL size: {e}")))?;
    w.write_all(&(stream.channels).to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write CHNL nc: {e}")))?;
    // Channel IDs: padded zeros, four bytes each.
    for _ in 0..nc {
        w.write_all(&[0u8; 4]).map_err(|e| {
            EngineError::DsdInvalidFile(format!("write CHNL id: {e}"))
        })?;
    }
    if chnl_body_len_padded != chnl_body_len {
        w.write_all(&[0u8])
            .map_err(|e| EngineError::DsdInvalidFile(format!("write CHNL pad: {e}")))?;
    }

    // DSD audio
    w.write_all(b"DSD ")
        .map_err(|e| EngineError::DsdInvalidFile(format!("write DSD: {e}")))?;
    w.write_all(&(audio_len as u64).to_be_bytes())
        .map_err(|e| EngineError::DsdInvalidFile(format!("write DSD size: {e}")))?;
    for f in 0..frames_per_ch {
        for c in 0..nc {
            w.write_all(&[stream.bytes_per_channel[c][f]]).map_err(|e| {
                EngineError::DsdInvalidFile(format!("write audio: {e}"))
            })?;
        }
    }
    if audio_pad != 0 {
        w.write_all(&[0u8])
            .map_err(|e| EngineError::DsdInvalidFile(format!("write audio pad: {e}")))?;
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
                    .map(|i| ((c * 13 + i * 7) & 0xFF) as u8)
                    .collect()
            })
            .collect();
        DsdStream {
            rate: DsdRate::Dsd64,
            channels,
            bytes_per_channel: bytes,
            lsb_first: false,
        }
    }

    #[test]
    fn dff_roundtrip_stereo() {
        let s = sample_stream(2, 2048);
        let mut buf = Vec::<u8>::new();
        write_dff(&s, &mut buf).expect("write");
        let parsed = read_dff(Cursor::new(buf)).expect("read");
        assert_eq!(parsed, s);
    }

    #[test]
    fn dff_roundtrip_quad_odd_length() {
        // Odd number of frames per channel exercises the audio
        // padding code in both encode and decode.
        let s = sample_stream(4, 3);
        let mut buf = Vec::<u8>::new();
        write_dff(&s, &mut buf).expect("write");
        let parsed = read_dff(Cursor::new(buf)).expect("read");
        assert_eq!(parsed, s);
    }

    #[test]
    fn dff_refuses_dst_compression() {
        // Hand-build a DFF that contains a DST chunk.
        let mut buf = Vec::<u8>::new();
        buf.extend_from_slice(b"FRM8");
        buf.extend_from_slice(&(0u64).to_be_bytes());
        buf.extend_from_slice(b"DSD ");
        buf.extend_from_slice(b"DST ");
        buf.extend_from_slice(&(0u64).to_be_bytes());
        let err = read_dff(Cursor::new(buf)).unwrap_err();
        match err {
            EngineError::Decode(msg) => {
                assert!(msg.contains("DST"), "got {msg}");
            }
            other => panic!("expected EngineError::Decode, got {other:?}"),
        }
    }
}
