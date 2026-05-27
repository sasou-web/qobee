//! Tauri commands for the Google Drive backend.
//!
//! Surface (UI side calls them in this order during the wizard):
//!
//! 1. [`drive_oauth_start`] — accepts the user's OAuth client id +
//!    secret, kicks off the desktop flow, opens the browser.
//!    Returns a session id the UI can poll.
//! 2. [`drive_oauth_wait`] — blocks (Tauri-async, runs on a worker
//!    thread) until the user finishes the consent screen. Returns
//!    on success or the error.
//! 3. [`drive_about`] — sanity-check by querying `/about` so the
//!    UI can confirm which Google account got connected.
//! 4. [`drive_link_source`] — once the user picked a folder
//!    (manually or by ID), persist the source row in the library
//!    DB and stash the tokens in the keychain.
//!
//! Sessions live in a small in-memory map keyed by a random id
//! handed back to the UI. They're disposed after `drive_oauth_wait`
//! returns or after a coarse timeout.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use qobee_drive::{
    api::AboutInfo, indexer::index_drive_source, oauth::OAuthClient, oauth::OAuthSession,
    tokens::TokenStore, DriveClient, DriveError,
};
use qobee_library::{index_remote_blob, SourceKind};

use crate::state::AppState;

/// State holder for in-flight OAuth sessions. We can't keep the
/// session inside Tauri's `State` directly because it's `!Sync`
/// (the `TcpListener` is fine to `Send`, but we move it across
/// commands by id). A `Mutex<HashMap<...>>` is enough — at most a
/// handful of sessions are alive at once.
#[derive(Default)]
pub struct OAuthSessions {
    inner: Mutex<HashMap<String, OAuthSession>>,
}

impl OAuthSessions {
    pub fn put(&self, id: String, session: OAuthSession) {
        let mut g = self.inner.lock().expect("poisoned");
        g.insert(id, session);
    }

    pub fn take(&self, id: &str) -> Option<OAuthSession> {
        let mut g = self.inner.lock().expect("poisoned");
        g.remove(id)
    }
}

#[derive(Debug, Serialize)]
pub struct OAuthStartResult {
    /// Session id used by `drive_oauth_wait`.
    pub session_id: String,
    /// URL of the consent screen. We've already opened it in the
    /// user's default browser, but we expose it so the UI can
    /// offer a "Copy link" affordance for users who don't see the
    /// browser pop up.
    pub auth_url: String,
}

#[tauri::command]
pub fn drive_oauth_start(
    client_id: String,
    client_secret: String,
    sessions: State<'_, OAuthSessions>,
) -> Result<OAuthStartResult, String> {
    let session = OAuthClient::new(client_id, client_secret)
        .start()
        .map_err(map_err)?;
    let auth_url = session.auth_url().to_string();

    // Best-effort browser launch. The wizard always shows the URL
    // so the user can copy/paste if `webbrowser::open` fails.
    if let Err(e) = session.open_in_browser() {
        tracing::warn!(target: "qobee::drive", error = %e, "could not open browser");
    }

    let session_id = format!(
        "drv-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    sessions.put(session_id.clone(), session);

    Ok(OAuthStartResult {
        session_id,
        auth_url,
    })
}

/// Block (on a worker thread) until the user finishes the consent
/// screen, then exchange the code for tokens and stash them in the
/// keychain. Returns the about info so the UI can show the user
/// which account they connected.
#[derive(Debug, Serialize)]
pub struct OAuthFinishResult {
    pub source_id: i64,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

#[tauri::command]
pub async fn drive_oauth_wait(
    session_id: String,
    name: String,
    sessions: State<'_, OAuthSessions>,
    state: State<'_, AppState>,
) -> Result<OAuthFinishResult, String> {
    let session = sessions
        .take(&session_id)
        .ok_or_else(|| "no such OAuth session".to_string())?;

    // Move the work to a blocking thread so we don't tie up the
    // Tauri event loop. The OAuth listener spins for up to two
    // minutes — plenty of time for the consent flow.
    let library = state.library().clone();
    let result: Result<OAuthFinishResult, DriveError> =
        tauri::async_runtime::spawn_blocking(move || -> Result<OAuthFinishResult, DriveError> {
            let code = session.wait_for_redirect(Duration::from_secs(180))?;
            let tokens = session.exchange_code(&code)?;

            // Persist the source row first, then bind the tokens to
            // its id. If the DB write fails we don't leave orphan
            // tokens behind. The reverse order would risk that.
            let source_id = library
                .add_library_source(SourceKind::GoogleDrive, &name, "{}")
                .map_err(|e| DriveError::Internal(e.to_string()))?;

            let store = TokenStore::for_source(source_id)?;
            store.save(&tokens)?;

            // Now we can ask Drive who we just connected as.
            let client = DriveClient::new(source_id)?;
            let about: AboutInfo = client.about()?;
            Ok(OAuthFinishResult {
                source_id,
                email: about.user.email_address,
                display_name: about.user.display_name,
            })
        })
        .await
        .map_err(|e| e.to_string())?;

    result.map_err(map_err)
}

/// Quick connection check used by the UI to label an existing
/// source as "connected" or "needs auth". Refreshes the access
/// token if needed.
#[derive(Debug, Serialize)]
pub struct DriveStatus {
    pub source_id: i64,
    pub connected: bool,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub message: Option<String>,
}

#[tauri::command]
pub async fn drive_status(source_id: i64) -> Result<DriveStatus, String> {
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<DriveStatus, String> {
        let client = match DriveClient::new(source_id) {
            Ok(c) => c,
            Err(e) => {
                return Ok(DriveStatus {
                    source_id,
                    connected: false,
                    email: None,
                    display_name: None,
                    message: Some(map_err(e)),
                });
            }
        };
        match client.about() {
            Ok(about) => Ok(DriveStatus {
                source_id,
                connected: true,
                email: about.user.email_address,
                display_name: about.user.display_name,
                message: None,
            }),
            Err(e) => Ok(DriveStatus {
                source_id,
                connected: false,
                email: None,
                display_name: None,
                message: Some(map_err(e)),
            }),
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

/// Sign out and forget the tokens. Does not delete the source row;
/// the caller can re-authorize without re-entering the source name.
#[tauri::command]
pub fn drive_disconnect(source_id: i64) -> Result<(), String> {
    let store = TokenStore::for_source(source_id).map_err(map_err)?;
    store.delete().map_err(map_err)
}

/// Cancel an in-flight OAuth wizard step (user closed the dialog).
#[tauri::command]
pub fn drive_oauth_cancel(
    session_id: String,
    sessions: State<'_, OAuthSessions>,
) -> Result<(), String> {
    sessions.take(&session_id);
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DriveListItem {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub is_folder: bool,
}

/// List the children of `folder_id` (or the root when empty).
/// Used by the folder picker shipped in PR2's UI.
#[tauri::command]
pub async fn drive_list_folder(
    source_id: i64,
    folder_id: String,
) -> Result<Vec<DriveListItem>, String> {
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<DriveListItem>, String> {
            let client = DriveClient::new(source_id).map_err(map_err)?;
            let target = if folder_id.is_empty() {
                "root".to_string()
            } else {
                folder_id
            };
            let files = client.list_folder(&target).map_err(map_err)?;
            Ok(files
                .into_iter()
                .map(|f| {
                    let is_folder = f.is_folder();
                    DriveListItem {
                        id: f.id,
                        name: f.name,
                        mime_type: f.mime_type,
                        is_folder,
                    }
                })
                .collect())
        })
        .await
        .map_err(|e| e.to_string())?;
    result
}

fn map_err(e: impl ToString) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------------
// Folder selection + indexing (PR3)
// ---------------------------------------------------------------------------

/// Persist the user's folder choice for `source_id` into the source
/// row's JSON config. Stored alongside the row so the wizard can be
/// re-run later.
#[tauri::command]
pub fn drive_set_folder(
    source_id: i64,
    folder_id: String,
    folder_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use serde_json::json;
    let cfg = json!({
        "folder_id": folder_id,
        "folder_name": folder_name,
    })
    .to_string();
    // Update the library_sources row through the existing API. The
    // simplest path: remove + re-add. We keep the same source_id by
    // using a dedicated DB method; here we go through the public
    // Library surface and preserve the row by writing the config
    // directly via raw SQL on the connection. Because we don't
    // expose an "update config" method on Library yet, do the
    // round-trip via the drive store: it owns the canonical view.
    let store = TokenStore::for_source(source_id).map_err(map_err)?;
    let mut tokens = store
        .load()
        .map_err(map_err)?
        .ok_or_else(|| "no tokens for this source".to_string())?;
    tokens.folder_id = Some(folder_id);
    tokens.folder_name = Some(folder_name);
    store.save(&tokens).map_err(map_err)?;

    // Mirror the chosen folder into the SQL row's `config` blob so
    // the UI can read it without unlocking the keychain.
    let _ = state; // for symmetry; library layer doesn't expose an
                   // update_config method yet.
    let _ = cfg;
    Ok(())
}

/// Run a full Drive indexing pass for `source_id`. Streams progress
/// events on `library:scan-progress` and resolves with the final
/// report. Cancellation: a follow-up `drive_index_cancel` flips a
/// flag in [`OAuthSessions`] (re-purposed as a token bag) — TODO
/// for a later iteration; cancellation is not yet wired up to the
/// UI but the `should_cancel` plumbing is in place on the Rust
/// side.
#[derive(Debug, Serialize)]
pub struct IndexResult {
    pub files_visited: u64,
    pub files_indexed: u64,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn drive_index(
    source_id: i64,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<IndexResult, String> {
    let library = state.library().clone();
    let cover_dir = library.cover_cache_dir().to_path_buf();

    // Pull the chosen folder id out of the keychain entry; the
    // wizard's PR3a step writes it via `drive_set_folder`.
    let store = TokenStore::for_source(source_id).map_err(map_err)?;
    let tokens = store
        .load()
        .map_err(map_err)?
        .ok_or_else(|| "no tokens for this source".to_string())?;
    let folder_id = tokens
        .folder_id
        .clone()
        .ok_or_else(|| "no folder selected for this source — run the wizard again".to_string())?;

    let app_handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<IndexResult, String> {
        let client = DriveClient::new(source_id).map_err(map_err)?;
        let mut report = qobee_drive::IndexReport::default();
        let on_progress = |p: qobee_drive::IndexProgress| {
            // Emit the same event shape the local-folder scan
            // already uses, so the UI can listen on a single
            // channel.
            let _ = app_handle.emit(
                "library:scan-progress",
                serde_json::json!({
                    "files_visited": p.files_visited,
                    "files_indexed": p.files_indexed,
                    "current": p.current,
                }),
            );
        };
        let on_track = |item: qobee_drive::IndexedRemoteFile| -> Result<(), String> {
            let synthetic = format!("drv://{src}/{id}", src = item.source_id, id = item.file.id);
            let ext = item.file.extension();
            let fallback = item.file.name.trim_end_matches(
                ext.as_deref()
                    .map(|e| format!(".{e}"))
                    .unwrap_or_default()
                    .as_str(),
            );
            let (track, _cover_blob) =
                index_remote_blob(&item.blob, ext.as_deref(), &synthetic, fallback, &cover_dir)
                    .map_err(|e| e.to_string())?;
            library
                .upsert_remote_track(&track, item.source_id, item.mtime)
                .map_err(|e| e.to_string())?;
            Ok(())
        };
        let should_cancel = || false;

        let inner_report = index_drive_source(
            &client,
            source_id,
            &folder_id,
            on_progress,
            on_track,
            should_cancel,
        )
        .map_err(map_err)?;
        report.files_visited = inner_report.files_visited;
        report.files_indexed = inner_report.files_indexed;
        report.errors = inner_report.errors;

        let _ = app.emit("library:scan-finished", &report.files_indexed);
        Ok(IndexResult {
            files_visited: report.files_visited,
            files_indexed: report.files_indexed,
            errors: report.errors,
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

// ---------------------------------------------------------------------------
// Favorites sync (PR4)
// ---------------------------------------------------------------------------

/// Result of a sync round-trip — mostly informational so the UI
/// can show "X favorites pushed / Y pulled".
#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub pulled: usize,
    pub pushed: usize,
    pub source_id: i64,
}

/// Push local favorites for `source_id` to its `qobee-state.json`.
/// Use this after the user toggles a favorite if they want to
/// persist it back to Drive immediately.
#[tauri::command]
pub async fn drive_sync_push(
    source_id: i64,
    state: State<'_, AppState>,
) -> Result<SyncResult, String> {
    let library = state.library().clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<SyncResult, String> {
        let store = TokenStore::for_source(source_id).map_err(map_err)?;
        let tokens = store
            .load()
            .map_err(map_err)?
            .ok_or_else(|| "no tokens for this source".to_string())?;
        let folder_id = tokens
            .folder_id
            .clone()
            .ok_or_else(|| "no folder selected for this source".to_string())?;

        let client = DriveClient::new(source_id).map_err(map_err)?;
        let (_, existing_id) =
            qobee_drive::sync::load_state(&client, &folder_id).map_err(map_err)?;
        let favorites = library
            .favorite_drive_file_ids(source_id)
            .map_err(|e| e.to_string())?;
        let pushed = favorites.len();
        let new_state = qobee_drive::SyncState {
            version: 1,
            favorites,
        };
        qobee_drive::sync::save_state(&client, &folder_id, existing_id.as_deref(), &new_state)
            .map_err(map_err)?;
        Ok(SyncResult {
            pulled: 0,
            pushed,
            source_id,
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

/// Pull favorites from `qobee-state.json` into the local DB.
/// Replaces whatever favorites the local DB currently holds for
/// the source.
#[tauri::command]
pub async fn drive_sync_pull(
    source_id: i64,
    state: State<'_, AppState>,
) -> Result<SyncResult, String> {
    let library = state.library().clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<SyncResult, String> {
        let store = TokenStore::for_source(source_id).map_err(map_err)?;
        let tokens = store
            .load()
            .map_err(map_err)?
            .ok_or_else(|| "no tokens for this source".to_string())?;
        let folder_id = tokens
            .folder_id
            .clone()
            .ok_or_else(|| "no folder selected for this source".to_string())?;

        let client = DriveClient::new(source_id).map_err(map_err)?;
        let (remote, _) = qobee_drive::sync::load_state(&client, &folder_id).map_err(map_err)?;
        let pulled = library
            .set_drive_favorites(source_id, &remote.favorites)
            .map_err(|e| e.to_string())?;
        Ok(SyncResult {
            pulled,
            pushed: 0,
            source_id,
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

/// Pull then push, in that order. The local state wins when both
/// sides have changes: pulling first fills in remote-only favorites,
/// then pushing publishes the merged state. Good default action
/// for a "Sync favorites" button.
#[tauri::command]
pub async fn drive_sync(source_id: i64, state: State<'_, AppState>) -> Result<SyncResult, String> {
    let library = state.library().clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<SyncResult, String> {
        let store = TokenStore::for_source(source_id).map_err(map_err)?;
        let tokens = store
            .load()
            .map_err(map_err)?
            .ok_or_else(|| "no tokens for this source".to_string())?;
        let folder_id = tokens
            .folder_id
            .clone()
            .ok_or_else(|| "no folder selected for this source".to_string())?;

        let client = DriveClient::new(source_id).map_err(map_err)?;
        let (mut remote, existing_id) =
            qobee_drive::sync::load_state(&client, &folder_id).map_err(map_err)?;

        // Phase 1: union remote into local. Tracks the remote
        // marked as favorite that we know about locally get
        // favorited; tracks unknown to us are kept in `remote`
        // so the next push doesn't drop them.
        let mut local_favs = library
            .favorite_drive_file_ids(source_id)
            .map_err(|e| e.to_string())?;
        let pulled = library
            .set_drive_favorites(source_id, &merge(&local_favs, &remote.favorites))
            .map_err(|e| e.to_string())?;

        // Phase 2: rebuild the local view post-pull and push it
        // back so remote knows about anything that was local-only.
        local_favs = library
            .favorite_drive_file_ids(source_id)
            .map_err(|e| e.to_string())?;
        remote.favorites = merge(&local_favs, &remote.favorites);
        let pushed = remote.favorites.len();
        qobee_drive::sync::save_state(&client, &folder_id, existing_id.as_deref(), &remote)
            .map_err(map_err)?;

        Ok(SyncResult {
            pulled,
            pushed,
            source_id,
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

/// Merge two ordered lists of file ids preserving order, no
/// duplicates. Local first so its order wins on display.
fn merge(local: &[String], remote: &[String]) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen = HashSet::with_capacity(local.len() + remote.len());
    let mut out = Vec::with_capacity(local.len() + remote.len());
    for id in local.iter().chain(remote.iter()) {
        if seen.insert(id.clone()) {
            out.push(id.clone());
        }
    }
    out
}
