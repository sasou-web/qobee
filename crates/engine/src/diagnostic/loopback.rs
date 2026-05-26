//! WASAPI loopback capture for the null-test diagnostic.
//!
//! On Windows we open the system render device (or the friendly-name
//! match the caller passes in) for *capture* via `IAudioClient` with
//! `AUDCLNT_STREAMFLAGS_LOOPBACK`. The wasapi crate exposes that flag
//! transparently when an `AudioClient` constructed with
//! `Direction::Render` is initialised in `Direction::Capture` shared
//! mode (R11.3).
//!
//! On every other OS the function is a stub returning
//! [`EngineError::BackendUnavailable`] so the orchestrator can still
//! surface a clean error message to the UI.

use std::time::Duration;

use crate::error::{EngineError, EngineResult};

#[cfg(target_os = "windows")]
pub fn capture_loopback(device_name: Option<&str>, duration: Duration) -> EngineResult<Vec<f32>> {
    use std::collections::VecDeque;

    use wasapi::{initialize_mta, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    // COM init is idempotent on this thread.
    let _ = initialize_mta().ok();

    let enumerator = DeviceEnumerator::new()
        .map_err(|e| EngineError::NullTestLoopbackFailed(format!("device enumerator: {e:?}")))?;

    // Resolve the requested render device by friendly name, or fall
    // back to the system default. Loopback is always sourced from a
    // *render* endpoint.
    let device = match device_name {
        Some(want) => {
            let mut found = None;
            if let Ok(coll) = enumerator.get_device_collection(&Direction::Render) {
                if let Ok(n) = coll.get_nbr_devices() {
                    for i in 0..n {
                        if let Ok(d) = coll.get_device_at_index(i) {
                            if let Ok(name) = d.get_friendlyname() {
                                if name == want {
                                    found = Some(d);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            match found {
                Some(d) => d,
                None => enumerator
                    .get_default_device(&Direction::Render)
                    .map_err(|e| {
                        EngineError::NullTestLoopbackFailed(format!("default device: {e:?}"))
                    })?,
            }
        }
        None => enumerator
            .get_default_device(&Direction::Render)
            .map_err(|e| EngineError::NullTestLoopbackFailed(format!("default device: {e:?}")))?,
    };

    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| EngineError::NullTestLoopbackFailed(format!("get_iaudioclient: {e:?}")))?;
    let mix_format = audio_client
        .get_mixformat()
        .map_err(|e| EngineError::NullTestLoopbackFailed(format!("get_mixformat: {e:?}")))?;
    let sample_rate = mix_format.get_samplespersec();
    let channels = mix_format.get_nchannels();
    let bits_per_sample = mix_format.get_bitspersample();
    let valid_bits = mix_format.get_validbitspersample();
    let block_align = mix_format.get_blockalign() as usize;
    let sample_type = mix_format.get_subformat().unwrap_or(SampleType::Float);

    // Reuse the device's mix format verbatim so the loopback path
    // never asks the OS to convert; we re-create a `WaveFormat` from
    // it because `wasapi::AudioClient::initialize_client` borrows the
    // wrapper, not the raw `WAVEFORMATEXTENSIBLE`.
    let wave_format = WaveFormat::new(
        bits_per_sample as usize,
        valid_bits as usize,
        &sample_type,
        sample_rate as usize,
        channels as usize,
        None,
    );

    // Period 200 000 hns = 20 ms. Loopback uses Shared mode by
    // construction, so we go through `EventsShared`.
    let stream_mode = StreamMode::EventsShared {
        autoconvert: false,
        buffer_duration_hns: 200_000,
    };
    audio_client
        .initialize_client(&wave_format, &Direction::Capture, &stream_mode)
        .map_err(|e| {
            EngineError::NullTestLoopbackFailed(format!("initialize_client (loopback): {e:?}"))
        })?;

    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| EngineError::NullTestLoopbackFailed(format!("set_get_eventhandle: {e:?}")))?;
    let capture_client = audio_client.get_audiocaptureclient().map_err(|e| {
        EngineError::NullTestLoopbackFailed(format!("get_audiocaptureclient: {e:?}"))
    })?;
    audio_client
        .start_stream()
        .map_err(|e| EngineError::NullTestLoopbackFailed(format!("start_stream: {e:?}")))?;

    let total_frames = (sample_rate as u64 * duration.as_millis() as u64 / 1_000) as usize;
    let mut out = Vec::with_capacity(total_frames * channels as usize);
    let mut byte_q: VecDeque<u8> = VecDeque::with_capacity(block_align * 4096);

    let start = std::time::Instant::now();
    let timeout_ms: u32 = 200;
    while start.elapsed() < duration && out.len() < total_frames * channels as usize {
        if h_event.wait_for_event(timeout_ms).is_err() {
            // No data this tick; go around. We bail out cleanly when
            // the duration wall-clock budget is exhausted.
            continue;
        }
        if capture_client
            .read_from_device_to_deque(&mut byte_q)
            .is_err()
        {
            break;
        }
        // Convert raw bytes → f32 samples. The mix format is the
        // device's, which on Windows is almost always 32-bit float
        // (Shared mixer); we still handle the i16 / i24-in-32 cases
        // defensively for older drivers.
        match sample_type {
            SampleType::Float => {
                while byte_q.len() >= 4 {
                    let mut buf = [0u8; 4];
                    for b in buf.iter_mut() {
                        *b = byte_q.pop_front().unwrap();
                    }
                    out.push(f32::from_le_bytes(buf));
                }
            }
            SampleType::Int => match bits_per_sample {
                16 => {
                    while byte_q.len() >= 2 {
                        let mut buf = [0u8; 2];
                        for b in buf.iter_mut() {
                            *b = byte_q.pop_front().unwrap();
                        }
                        out.push(f32::from(i16::from_le_bytes(buf)) / 32_768.0);
                    }
                }
                24 => {
                    while byte_q.len() >= 3 {
                        let mut buf = [0u8; 4];
                        for b in buf.iter_mut().take(3) {
                            *b = byte_q.pop_front().unwrap();
                        }
                        // Sign-extend the 24-bit sample into i32.
                        if buf[2] & 0x80 != 0 {
                            buf[3] = 0xFF;
                        }
                        let s = i32::from_le_bytes(buf);
                        out.push(s as f32 / 8_388_608.0);
                    }
                }
                32 => {
                    while byte_q.len() >= 4 {
                        let mut buf = [0u8; 4];
                        for b in buf.iter_mut() {
                            *b = byte_q.pop_front().unwrap();
                        }
                        let s = i32::from_le_bytes(buf);
                        out.push(s as f32 / 2_147_483_648.0);
                    }
                }
                _ => {
                    return Err(EngineError::NullTestLoopbackFailed(format!(
                        "unsupported integer sample size {} bits",
                        bits_per_sample
                    )));
                }
            },
        }
    }

    let _ = audio_client.stop_stream();
    Ok(out)
}

#[cfg(not(target_os = "windows"))]
pub fn capture_loopback(_device_name: Option<&str>, _duration: Duration) -> EngineResult<Vec<f32>> {
    Err(EngineError::BackendUnavailable(
        "WASAPI loopback capture is only available on Windows".to_string(),
    ))
}
