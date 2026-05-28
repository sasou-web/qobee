//! OS-side projectors of the R8 transport bus.
//!
//! Each platform has its own thin sink: SMTC on Windows, the
//! `MPNowPlayingInfoCenter` / `MPRemoteCommandCenter` pair on macOS.
//! Both implement [`qobee_core::MediaBridge`] and are driven by the
//! single fan-out task spawned in [`crate::lib::run`].
//!
//! The submodules are `cfg`-gated by target OS so the build never
//! pulls in `windows-rs` on macOS or `objc2*` on Windows / Linux.
//!
//! - Linux: no MPRIS bridge in this delivery — see the spec's
//!   "Hors périmètre" section. The fan-out simply skips the macOS /
//!   Windows blocks.

#[cfg(target_os = "macos")]
pub mod mac;

#[cfg(target_os = "windows")]
pub mod smtc;
