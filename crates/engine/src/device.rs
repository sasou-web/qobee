//! Output-device enumeration and selection.
//!
//! The output-mode machinery (Auto / Shared / Exclusive / ASIO) was
//! removed when the engine moved to `rodio`. The device *picker* is
//! preserved: the user can still list the available output endpoints
//! and pin playback to one of them. rodio's [`OutputStreamBuilder`]
//! can be pointed at a specific `cpal::Device`, so all we need here is
//! enumeration plus a resolver that maps a requested device id to a
//! live `cpal::Device` (falling back to the system default).

use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait};

use crate::error::{EngineError, EngineResult};
use crate::types::OutputDevice;

/// List every available output device. Each entry carries a stable id
/// (the CPAL device name), a human-readable label, the default-config
/// sample rate / channel count, and a flag marking the system default.
pub fn list_output_devices() -> EngineResult<Vec<OutputDevice>> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());

    let devices = host
        .output_devices()
        .map_err(|e| EngineError::Output(format!("could not enumerate output devices: {e}")))?;

    let mut out = Vec::new();
    for device in devices {
        // Skip devices we cannot name — the name is our stable id.
        let Ok(name) = device.name() else {
            continue;
        };
        let cfg = device.default_output_config().ok();
        out.push(OutputDevice {
            is_default: Some(&name) == default_name.as_ref(),
            default_sample_rate: cfg.as_ref().map(|c| c.sample_rate().0).unwrap_or(0),
            channels: cfg.as_ref().map(|c| c.channels()).unwrap_or(2),
            name: name.clone(),
            id: name,
        });
    }
    Ok(out)
}

/// Resolve a requested device id against the live device list.
///
/// Returns the matching `cpal::Device` when `requested` is `Some(id)`
/// and that id is present, otherwise the system default device. The
/// boolean is `true` when a fallback to the default occurred because
/// the requested id was absent (so the caller can surface a soft
/// warning). Passing `None` selects the default without flagging a
/// fallback.
pub fn resolve_output_device(
    requested: Option<&str>,
) -> EngineResult<(cpal::Device, bool /* fell_back */)> {
    let host = cpal::default_host();

    if let Some(id) = requested {
        if let Ok(devices) = host.output_devices() {
            for device in devices {
                if device.name().map(|n| n == id).unwrap_or(false) {
                    return Ok((device, false));
                }
            }
        }
        // Requested id not found: fall back to the default and flag it.
        let default = host.default_output_device().ok_or_else(|| {
            EngineError::BackendUnavailable("no default output device available".to_string())
        })?;
        return Ok((default, true));
    }

    let default = host.default_output_device().ok_or_else(|| {
        EngineError::BackendUnavailable("no default output device available".to_string())
    })?;
    Ok((default, false))
}
