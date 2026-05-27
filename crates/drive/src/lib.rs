//! Google Drive backend for Qobee.
//!
//! Surface:
//!
//! * [`oauth::OAuthSession`] — handles the desktop OAuth 2.0 PKCE
//!   flow: spawns a temporary loopback HTTP listener, opens the
//!   user's browser at the consent screen, captures the
//!   authorization code from the redirect, exchanges it for tokens,
//!   and persists them in the OS keychain.
//! * [`tokens::TokenStore`] — reads / writes / refreshes the
//!   access + refresh tokens. Backed by the `keyring` crate (no
//!   tokens ever land in plain SQLite).
//! * [`api::DriveClient`] — thin Drive REST wrapper for the calls
//!   we actually make: list files in a folder (with paging),
//!   stream a byte range, fetch metadata.
//! * [`scan::list_audio_files`] — recursive walk of a Drive folder
//!   yielding every audio file with its parent path. Used by the
//!   indexer in PR3.
//!
//! Threading: every public function is blocking. The Tauri layer
//! offloads them onto `tokio::task::spawn_blocking` so the UI
//! stays responsive. Library SQLite access stays under
//! `parking_lot::Mutex` exactly as before.

pub mod api;
pub mod error;
pub mod indexer;
pub mod media_source;
pub mod oauth;
pub mod scan;
pub mod sync;
pub mod tokens;

pub use api::{DriveClient, DriveFile, DriveFolder};
pub use error::{DriveError, DriveResult};
pub use indexer::{index_drive_source, IndexProgress, IndexReport, IndexedRemoteFile};
pub use media_source::DriveMediaSource;
pub use oauth::{OAuthClient, OAuthSession};
pub use sync::{load_state, save_state, SyncState, STATE_FILENAME};
pub use tokens::{StoredTokens, TokenStore};

/// Drive scopes Qobee asks for. We request `drive.file` so favorites /
/// sync state can be written back to the connected folder later;
/// listing + reading is covered by the same scope when the user
/// picks a folder via the file picker (file picker grants
/// per-folder access). For broader-than-folder reads we'd need
/// `drive.readonly`, which the user can toggle via PR4 if we ship
/// it.
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// Drive REST API base URL.
pub const DRIVE_API_BASE: &str = "https://www.googleapis.com/drive/v3";

/// Files endpoint used for raw byte-range downloads.
pub const DRIVE_DOWNLOAD_BASE: &str = "https://www.googleapis.com/drive/v3/files";

/// Custom URI scheme used in `tracks.path` for files indexed
/// from a Drive source. Format: `drv://<source_id>/<file_id>`.
pub const URI_SCHEME: &str = "drv://";

/// Parse a `drv://<source_id>/<file_id>` URI into its parts.
/// Returns `None` for non-Drive paths so the caller can dispatch
/// the local-file path through the legacy backend.
pub fn parse_drive_uri(uri: &str) -> Option<(i64, &str)> {
    let rest = uri.strip_prefix(URI_SCHEME)?;
    let (source_id, file_id) = rest.split_once('/')?;
    let source_id = source_id.parse::<i64>().ok()?;
    if file_id.is_empty() {
        return None;
    }
    Some((source_id, file_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_drive_uri() {
        assert_eq!(parse_drive_uri("drv://12/abc"), Some((12, "abc")));
        assert_eq!(
            parse_drive_uri("drv://1/file_with-weird.chars"),
            Some((1, "file_with-weird.chars"))
        );
    }

    #[test]
    fn rejects_non_drive_uri() {
        assert!(parse_drive_uri("file:///tmp/x.flac").is_none());
        assert!(parse_drive_uri("C:\\Music\\song.flac").is_none());
        assert!(parse_drive_uri("drv://").is_none());
        assert!(parse_drive_uri("drv://abc/file").is_none()); // non-int source id
        assert!(parse_drive_uri("drv://12/").is_none()); // empty file id
    }
}
