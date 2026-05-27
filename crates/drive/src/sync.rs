//! Two-way sync of small JSON state with a Drive source folder.
//!
//! Today we only sync the favorites list — extended state (sort
//! orders, smart playlists, EQ presets) can ride the same channel
//! later by adding fields to [`SyncState`].
//!
//! File layout: `qobee-state.json` lives at the root of the
//! user-selected source folder. The state file is plain JSON, so
//! the user can also edit / inspect it from the Drive web UI if
//! they ever want to.
//!
//! Concurrency: Drive doesn't give us a CAS primitive, so we just
//! pick the freshest copy when in doubt. Two devices syncing in
//! lockstep is unsupported (and would be unusual for a music
//! player).

use serde::{Deserialize, Serialize};

use crate::api::DriveClient;
use crate::error::DriveResult;

/// Filename used for the JSON state blob inside the source folder.
pub const STATE_FILENAME: &str = "qobee-state.json";

/// Versioned state envelope. Bump `version` only when adding a
/// breaking field; new optional fields are backwards compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    pub version: u32,
    /// Set of Drive file ids the user has favorited. We store ids
    /// rather than names so renames don't lose favorites.
    #[serde(default)]
    pub favorites: Vec<String>,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            version: 1,
            favorites: Vec::new(),
        }
    }
}

/// Read-or-default: fetch the state file from the source folder,
/// returning a fresh empty state when no file exists yet.
pub fn load_state(
    client: &DriveClient,
    folder_id: &str,
) -> DriveResult<(SyncState, Option<String>)> {
    let existing = client.find_in_folder_by_name(folder_id, STATE_FILENAME)?;
    let Some(file) = existing else {
        return Ok((SyncState::default(), None));
    };
    let bytes = client.download_full(&file.id)?;
    if bytes.is_empty() {
        return Ok((SyncState::default(), Some(file.id)));
    }
    // Be lenient: if the JSON is malformed (concurrent write,
    // hand-edit gone wrong), start over rather than wedge the
    // app. The user can re-export later.
    let parsed = serde_json::from_slice::<SyncState>(&bytes).unwrap_or_default();
    Ok((parsed, Some(file.id)))
}

/// Persist `state` to the source folder. Creates the file the
/// first time, overwrites otherwise.
pub fn save_state(
    client: &DriveClient,
    folder_id: &str,
    existing_file_id: Option<&str>,
    state: &SyncState,
) -> DriveResult<String> {
    let json = serde_json::to_string_pretty(state)?;
    if let Some(id) = existing_file_id {
        client.update_file_content(id, &json)?;
        Ok(id.to_string())
    } else {
        client.create_text_file(folder_id, STATE_FILENAME, &json)
    }
}
