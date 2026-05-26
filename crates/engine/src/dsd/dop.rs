//! `DopPacker` — wraps DSD bits into 24-in-32 PCM samples per the
//! DoP (DSD over PCM) specification (R7.3 / R7.4).
//!
//! Layout of one 24-bit DoP word emitted on the carrier:
//!
//! ```text
//! bit 31..24 = marker (0x05 or 0xFA, alternating after every sample)
//! bit 23..16 = high 8 DSD bits (oldest in time)
//! bit 15..8  = low 8 DSD bits (newest in time)
//! bit 7..0   = unused (zero)
//! ```
//!
//! The whole 24-bit word is then sign-extended into a 32-bit
//! container (`I24In32Interleaved`) which is what WASAPI Exclusive
//! consumes when negotiated as `(32, 24, SampleType::Int)`.
//!
//! ## Marker alternation (R7.4 strict)
//!
//! The DoP spec mandates that the marker alternates `0x05 / 0xFA`
//! **after every sample** — *not* after every frame. In a stereo
//! stream the L sample at index 0 carries `0x05`, R at index 0
//! carries `0xFA`, L at index 1 carries `0x05`, and so on. Compliant
//! DACs sync on the marker pattern; an in-frame-aligned alternation
//! would put both L and R of frame 0 on the same marker and break
//! the sync.

/// Stateful packer that walks a DSD stream 16 bits at a time per
/// channel and emits one 24-in-32 sample per channel per call.
///
/// `marker_state` is the next marker to emit. `reset()` snaps it
/// back to the canonical starting value `0x05`.
pub struct DopPacker {
    marker_state: u8,
    channels: u16,
}

impl DopPacker {
    /// Build a packer for an `n`-channel stream. Initial marker is
    /// `0x05`.
    pub fn new(channels: u16) -> Self {
        Self {
            marker_state: 0x05,
            channels,
        }
    }

    /// Channel count this packer was built for.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Convert one [`super::DsdGroup16`] into `channels` `i32` samples.
    ///
    /// Each per-channel u16 holds 16 DSD bits with the most
    /// significant bit being the *oldest* in time on the DoP
    /// carrier. Returns one i32 per channel; the marker advances
    /// after every emitted sample (R7.4 strict).
    pub fn pack(&mut self, group: &super::DsdGroup16) -> Vec<i32> {
        let n = self.channels as usize;
        let mut out = Vec::with_capacity(n);
        for c in 0..n {
            let dsd16 = *group.per_channel.get(c).unwrap_or(&0) as u32;
            let marker = self.marker_state as u32;
            let pcm24: u32 = (marker << 16) | (dsd16 & 0xFFFF);
            // 24-bit sign-extension into i32: shift left 8 then
            // arithmetic-shift right 8.
            let pcm32 = ((pcm24 as i32) << 8) >> 8;
            out.push(pcm32);
            self.marker_state = if self.marker_state == 0x05 {
                0xFA
            } else {
                0x05
            };
        }
        out
    }

    /// Reset the alternation state back to its canonical starting
    /// marker (`0x05`). Called on seek and on track load so the
    /// next sample emitted lines up with what the DAC expects after
    /// a flush.
    pub fn reset(&mut self) {
        self.marker_state = 0x05;
    }

    /// Current marker state. Exposed for the round-trip property
    /// tests so they can verify the alternation invariant directly
    /// on the packer's internal counter.
    pub fn marker_state(&self) -> u8 {
        self.marker_state
    }
}

#[cfg(test)]
mod tests {
    use super::super::DsdGroup16;
    use super::*;

    fn group(per_channel: Vec<u16>) -> DsdGroup16 {
        DsdGroup16 { per_channel }
    }

    #[test]
    fn pack_alternates_marker_per_sample_not_per_frame() {
        let mut packer = DopPacker::new(2);
        let g = group(vec![0xABCD, 0x1234]);
        let out = packer.pack(&g);
        assert_eq!(out.len(), 2);

        // L sample (index 0) carries 0x05; R (index 1) carries 0xFA.
        let marker_l = ((out[0] as u32) >> 16) & 0xFF;
        let marker_r = ((out[1] as u32) >> 16) & 0xFF;
        assert_eq!(marker_l, 0x05);
        assert_eq!(marker_r, 0xFA);

        // Low 16 bits hold the DSD payload unchanged.
        assert_eq!((out[0] as u32) & 0xFFFF, 0xABCD);
        assert_eq!((out[1] as u32) & 0xFFFF, 0x1234);

        // Next call: L = 0x05 again, R = 0xFA. The alternation is
        // *per sample* and continues across `pack()` invocations.
        let g2 = group(vec![0x5555, 0xAAAA]);
        let out2 = packer.pack(&g2);
        let m2_l = ((out2[0] as u32) >> 16) & 0xFF;
        let m2_r = ((out2[1] as u32) >> 16) & 0xFF;
        assert_eq!(m2_l, 0x05);
        assert_eq!(m2_r, 0xFA);
    }

    #[test]
    fn reset_returns_to_marker_05() {
        let mut packer = DopPacker::new(1);
        packer.pack(&group(vec![0xABCD]));
        // Now marker_state == 0xFA.
        assert_eq!(packer.marker_state(), 0xFA);
        packer.reset();
        assert_eq!(packer.marker_state(), 0x05);
    }

    #[test]
    fn payload_is_sign_extended_to_i32() {
        let mut packer = DopPacker::new(1);
        // Marker 0xFA + payload 0xFFFF: 24-bit value 0xFAFFFF.
        // Bit 23 is set, so sign-extension to i32 produces a
        // negative number whose low 24 bits are exactly 0xFAFFFF.
        packer.marker_state = 0xFA;
        let out = packer.pack(&group(vec![0xFFFF]));
        let v = out[0];
        assert!(v < 0, "expected negative i32, got {v}");
        assert_eq!((v as u32) & 0x00FF_FFFF, 0x00FA_FFFF);
    }
}
