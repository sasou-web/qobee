//! Persistence layer for OAuth tokens.
//!
//! We store the access + refresh tokens in the OS keychain rather
//! than the SQLite settings table so the secrets never sit in a
//! file the user can email or commit to a backup. The `keyring`
//! crate forwards to:
//!   * Keychain Services (macOS),
//!   * Credential Manager (Windows),
//!   * Secret Service over D-Bus (Linux).
//!
//! Each `Library Source` row in SQLite is identified by an `i64`;
//! we key the keychain entry by `qobee.drive.tokens.<source_id>`
//! so two Drive sources don't collide.
//!
//! Tokens are stored as a single JSON blob — keyring entries are
//! one string, so we serialize. The blob looks like:
//!
//! ```json
//! { "access_token": "...", "refresh_token": "...", "expires_at": 1716480000, "client_id": "...", "client_secret": "..." }
//! ```
//!
//! `client_id` / `client_secret` live alongside the tokens because
//! they're equally sensitive — having them in plain SQL would defeat
//! the whole point.

use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::error::{DriveError, DriveResult};

/// Service name used by every Qobee Drive entry. The account name
/// (the second arg to `Entry::new`) varies per source so multiple
/// Drive accounts can coexist.
pub const SERVICE: &str = "app.qobee.player.drive";

/// Tokens + client config persisted for one Drive source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTokens {
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix epoch seconds when the access token stops being valid.
    pub expires_at: i64,
    /// Optional folder id selected by the user; null for "whole
    /// authorized scope".
    pub folder_id: Option<String>,
    /// Display name of the folder (falls back to "My Drive").
    pub folder_name: Option<String>,
}

impl StoredTokens {
    pub fn is_expired(&self, now: i64) -> bool {
        // Refresh a minute early to dodge clock skew + the network
        // latency budget of the next call.
        self.expires_at <= now + 60
    }
}

/// Keychain-backed store for one source.
pub struct TokenStore {
    entry: Entry,
    source_id: i64,
}

impl TokenStore {
    pub fn for_source(source_id: i64) -> DriveResult<Self> {
        let account = format!("source-{source_id}");
        let entry = Entry::new(SERVICE, &account)?;
        Ok(Self { entry, source_id })
    }

    pub fn source_id(&self) -> i64 {
        self.source_id
    }

    pub fn save(&self, tokens: &StoredTokens) -> DriveResult<()> {
        let json = serde_json::to_string(tokens)?;
        self.entry.set_password(&json)?;
        Ok(())
    }

    pub fn load(&self) -> DriveResult<Option<StoredTokens>> {
        match self.entry.get_password() {
            Ok(s) => {
                let parsed: StoredTokens = serde_json::from_str(&s)?;
                Ok(Some(parsed))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(DriveError::Keyring(e)),
        }
    }

    pub fn delete(&self) -> DriveResult<()> {
        match self.entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(DriveError::Keyring(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_window_is_one_minute() {
        let t = StoredTokens {
            client_id: "id".into(),
            client_secret: "sec".into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 1_000,
            folder_id: None,
            folder_name: None,
        };
        // Less than 60s of headroom = considered expired.
        assert!(t.is_expired(940));
        assert!(t.is_expired(1_000));
        // Plenty of time = fine.
        assert!(!t.is_expired(900));
    }
}
