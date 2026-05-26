//! DSD (Direct Stream Digital) pipeline — parsers, in-memory stream,
//! and DoP packer.
//!
//! The audio-quality-improvements spec (R7) adds native DSD playback
//! to Qobee: the engine reads `.dsf` and `.dff` files (DSD64 → DSD512),
//! never converts them to PCM, and forwards the bit stream wrapped in
//! 24-bit DoP markers to a WASAPI Exclusive endpoint.
//!
//! ## Module layout
//!
//! - [`dsf`] — DSF (DSD Stream File) parser and writer.
//! - [`dff`] — DFF / DSDIFF (Interchange File Format) parser and
//!   writer. Refuses DST-compressed payloads.
//! - [`dop`] — `DopPacker` that wraps 16 DSD bits per channel into
//!   24-bit PCM samples, alternating the `0x05`/`0xFA` marker after
//!   *every* sample (R7.4 strict).
//!
//! All parsers are pure (no I/O outside the supplied `Read + Seek`
//! reader), allocate a single in-memory copy of the per-channel bit
//! stream, and pair with a writer of the same name so the property
//! tests can round-trip a generator output through `read → write →
//! read` and assert the second [`DsdStream`] equals the first
//! sample-for-sample (R7.9).

pub mod dff;
pub mod dop;
pub mod dsf;

pub use dop::DopPacker;

use serde::{Deserialize, Serialize};

/// DSD sample-rate identifier (R7.2). Mirrored on the library side
/// via `qobee_library::DsdRate`; we keep an engine-local copy so
/// neither layer needs to depend on the other for the type.
///
/// `hz()` is the bit rate (the "DSD64" name is shorthand for
/// `64 × 44_100 = 2_822_400` bits/s/channel). `dop_carrier_rate()`
/// is the PCM carrier frequency the DAC sees in WASAPI Exclusive
/// once the DoP marker has been packed in: it is exactly `hz / 16`
/// because each PCM sample carries 16 DSD bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DsdRate {
    Dsd64,
    Dsd128,
    Dsd256,
    Dsd512,
}

impl DsdRate {
    /// Bit-rate per channel (the "DSD64 = 2.8224 MHz" figure).
    pub fn hz(self) -> u32 {
        match self {
            DsdRate::Dsd64 => 2_822_400,
            DsdRate::Dsd128 => 5_644_800,
            DsdRate::Dsd256 => 11_289_600,
            DsdRate::Dsd512 => 22_579_200,
        }
    }

    /// Human label exposed in `PlayerState::dsd_rate_label`.
    pub fn label(self) -> &'static str {
        match self {
            DsdRate::Dsd64 => "DSD64",
            DsdRate::Dsd128 => "DSD128",
            DsdRate::Dsd256 => "DSD256",
            DsdRate::Dsd512 => "DSD512",
        }
    }

    /// PCM carrier rate (Hz) the DoP packer hands to WASAPI
    /// Exclusive. Sixteen DSD bits per channel are wrapped in one
    /// 24-bit PCM sample, so the carrier is the bit rate divided by
    /// 16.
    pub fn dop_carrier_rate(self) -> u32 {
        self.hz() / 16
    }

    /// Recognise a `(sample_rate, bit_depth)` pair as one of the
    /// canonical DSD rates. Mirrors the library's
    /// `DsdRate::from_track` so the engine doesn't need to depend on
    /// the library crate.
    pub fn from_hz_bits(sample_rate: u32, bit_depth: u8) -> Option<Self> {
        if bit_depth != 1 {
            return None;
        }
        match sample_rate {
            2_822_400 => Some(DsdRate::Dsd64),
            5_644_800 => Some(DsdRate::Dsd128),
            11_289_600 => Some(DsdRate::Dsd256),
            22_579_200 => Some(DsdRate::Dsd512),
            _ => None,
        }
    }
}

/// In-memory DSD stream: per-channel bytes + bit ordering metadata.
///
/// Each channel's bytes are stored contiguously, deinterleaved from
/// whichever container layout the file used (DSF interleaves by
/// 4096-byte blocks per channel; DFF interleaves byte-by-byte).
/// `lsb_first` records whether each byte's least-significant bit
/// comes first in time:
///
///   * DSF stores LSB-first (`lsb_first = true`).
///   * DFF stores MSB-first (`lsb_first = false`).
///
/// The `DopPacker` reads a fresh [`DsdGroup16`] every 16 bits per
/// channel; the bit ordering tells it which end of each byte to
/// pick first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DsdStream {
    pub rate: DsdRate,
    pub channels: u16,
    /// One `Vec<u8>` per channel. All channels MUST have the same
    /// length; parsers return `EngineError::DsdInvalidFile` when
    /// they don't.
    pub bytes_per_channel: Vec<Vec<u8>>,
    /// `true` when each byte stores the least-significant DSD bit
    /// first in time (DSF semantics). `false` for MSB-first (DFF).
    pub lsb_first: bool,
}

impl DsdStream {
    /// Frames per channel (one frame == 8 DSD bits, matching the
    /// container's byte granularity). Equal to
    /// `bytes_per_channel[0].len()` after a successful parse.
    pub fn frames(&self) -> usize {
        self.bytes_per_channel
            .first()
            .map(|c| c.len())
            .unwrap_or(0)
    }

    /// Number of 16-bit DoP groups produceable from this stream
    /// (i.e. `frames / 2`). Trailing bytes that do not complete a
    /// group are not packed.
    pub fn dop_groups(&self) -> usize {
        self.frames() / 2
    }
}

/// One 16-bit DSD slice per channel, ready for the [`DopPacker`].
///
/// Field is exposed as `Vec<u16>` rather than a fixed-size array to
/// support multichannel streams (DSF and DFF both allow up to 6
/// channels in the wild; the format reserves room for more).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DsdGroup16 {
    /// One u16 per channel. Bit 15 is the *first* DSD bit in time
    /// for the carrier-side DoP packer (the packer always emits
    /// MSB-first 24-bit words on the carrier; the per-byte bit
    /// ordering of the source is normalised by the parser when it
    /// fills this group).
    pub per_channel: Vec<u16>,
}
