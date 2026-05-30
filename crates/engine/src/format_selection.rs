//! Pure output-format selection logic for the WASAPI Exclusive
//! negotiation (R9.4, R9.5).
//!
//! This module is intentionally **platform-independent**: it contains
//! no WASAPI / COM types, no I/O, and no `#[cfg(windows)]` gating, so
//! both the Windows-only backend (`backend_wasapi_exclusive`) and the
//! cross-platform property tests (`tests/properties/format_selection.rs`)
//! can reach it.
//!
//! The Windows backend probes the device for the channel layouts it
//! accepts, maps each accepted probe into a [`CandidateFormat`], and
//! then calls [`select_format`] to decide which one to use. The
//! decision rule (R9.4) is: when the source is stereo and the device
//! offers a wider layout, prefer a **native stereo** configuration if
//! one is available rather than upmixing to surround. More generally,
//! the selection prefers a layout that matches the source channel
//! count exactly (no upmix / no downmix), then the layout that
//! requires the least upmix, and only falls back to a narrower
//! (downmix) layout when nothing else is available.
//!
//! The `upmix` flag on the result (R9.5) is `Some { src, dst }` **iff**
//! the chosen layout has more channels than the source, so the player
//! can surface an "upmix active" indicator in `PlayerState`.

use crate::types::UpmixInfo;

/// A single output format the device reported as supported during
/// negotiation. Deliberately minimal and free of any WASAPI type so
/// the selection logic stays pure and testable cross-platform.
///
/// The Windows backend builds one of these per accepted probe (e.g.
/// after `is_supported_exclusive_with_quirks` succeeds for a given
/// sample-rate / channel-count pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateFormat {
    /// Sample rate in Hz the device accepts for this layout.
    pub sample_rate: u32,
    /// Channel count of this candidate layout (e.g. `2` for stereo,
    /// `6` for 5.1, `8` for 7.1).
    pub channels: u16,
}

impl CandidateFormat {
    /// Convenience constructor.
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }
}

/// Outcome of [`select_format`]: the chosen device layout plus the
/// upmix descriptor (R9.5), which is `None` unless the chosen layout
/// is wider than the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSelection {
    /// The candidate the selector picked.
    pub chosen: CandidateFormat,
    /// `Some { src, dst }` iff `chosen.channels > src_channels`
    /// (R9.5); `None` otherwise.
    pub upmix: Option<UpmixInfo>,
}

/// Rank a candidate's channel layout against the source channel count.
/// Lower is better. The first element is the coarse class (native /
/// upmix / downmix) and the second is the distance within that class,
/// so a native match always wins, then the least-upmix layout, and a
/// downmix layout is only chosen as a last resort.
fn channel_rank(candidate_channels: u16, src_channels: u16) -> (u8, u16) {
    use std::cmp::Ordering;
    match candidate_channels.cmp(&src_channels) {
        Ordering::Equal => (0, 0), // native: best
        Ordering::Greater => (1, candidate_channels - src_channels), // upmix: prefer least
        Ordering::Less => (2, src_channels - candidate_channels), // downmix: last resort
    }
}

/// Choose the best output format from the device's `candidates` for a
/// source with `src_channels` channels (R9.4, R9.5).
///
/// Selection order (best first):
///   1. A layout that matches `src_channels` exactly — no upmix and
///      no downmix. For a stereo source this is the *native stereo*
///      configuration the requirement asks us to prefer over surround
///      (R9.4).
///   2. The layout requiring the **least** upmix (smallest channel
///      count strictly greater than `src_channels`).
///   3. Only if neither exists, the **widest** narrower layout (least
///      downmix).
///
/// Within the same channel rank, the higher sample rate is preferred
/// (closer to a no-resample path); remaining ties keep the first
/// candidate in iteration order, so the result is deterministic.
///
/// Returns `None` only when `candidates` is empty.
///
/// The `upmix` field of the result is `Some(UpmixInfo { src, dst })`
/// **iff** `chosen.channels > src_channels` (R9.5).
pub fn select_format(candidates: &[CandidateFormat], src_channels: u16) -> Option<FormatSelection> {
    let chosen = *candidates.iter().min_by(|a, b| {
        // Primary: channel rank (native < least-upmix < downmix).
        channel_rank(a.channels, src_channels)
            .cmp(&channel_rank(b.channels, src_channels))
            // Secondary: prefer the higher sample rate (reverse order
            // so "greater" sorts as "smaller"/better).
            .then_with(|| b.sample_rate.cmp(&a.sample_rate))
    })?;

    let upmix = if chosen.channels > src_channels {
        Some(UpmixInfo {
            src_channels,
            dst_channels: chosen.channels,
        })
    } else {
        None
    };

    Some(FormatSelection { chosen, upmix })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_stereo_preferred_over_surround() {
        // Device offers stereo and 7.1 at the same rate; a stereo
        // source must get the native stereo layout, no upmix (R9.4).
        let candidates = [
            CandidateFormat::new(48_000, 8),
            CandidateFormat::new(48_000, 2),
        ];
        let sel = select_format(&candidates, 2).unwrap();
        assert_eq!(sel.chosen.channels, 2);
        assert!(sel.upmix.is_none(), "native stereo must not flag upmix");
    }

    #[test]
    fn surround_only_device_flags_upmix() {
        // Surround-only device + stereo source -> upmix 2 -> 8 (R9.5).
        let candidates = [CandidateFormat::new(48_000, 8)];
        let sel = select_format(&candidates, 2).unwrap();
        assert_eq!(sel.chosen.channels, 8);
        assert_eq!(
            sel.upmix,
            Some(UpmixInfo {
                src_channels: 2,
                dst_channels: 8
            })
        );
    }

    #[test]
    fn least_upmix_layout_chosen() {
        // No native stereo; prefer the smallest layout above src.
        let candidates = [
            CandidateFormat::new(48_000, 8),
            CandidateFormat::new(48_000, 6),
            CandidateFormat::new(48_000, 4),
        ];
        let sel = select_format(&candidates, 2).unwrap();
        assert_eq!(sel.chosen.channels, 4, "least upmix preferred");
        assert_eq!(
            sel.upmix,
            Some(UpmixInfo {
                src_channels: 2,
                dst_channels: 4
            })
        );
    }

    #[test]
    fn higher_sample_rate_breaks_ties_within_native_layout() {
        let candidates = [
            CandidateFormat::new(44_100, 2),
            CandidateFormat::new(96_000, 2),
            CandidateFormat::new(48_000, 2),
        ];
        let sel = select_format(&candidates, 2).unwrap();
        assert_eq!(sel.chosen.sample_rate, 96_000);
        assert!(sel.upmix.is_none());
    }

    #[test]
    fn downmix_only_as_last_resort() {
        // Source 6ch, only stereo offered -> downmix, no upmix flag.
        let candidates = [CandidateFormat::new(48_000, 2)];
        let sel = select_format(&candidates, 6).unwrap();
        assert_eq!(sel.chosen.channels, 2);
        assert!(
            sel.upmix.is_none(),
            "downmix must not set the upmix flag (chosen < src)"
        );
    }

    #[test]
    fn empty_candidates_returns_none() {
        assert!(select_format(&[], 2).is_none());
    }
}
