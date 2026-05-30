//! Qobee library indexing.
//!
//! Walks a folder, extracts metadata via [`lofty`], stores tracks/albums/
//! artists in a SQLite database (rusqlite, bundled feature), and writes
//! embedded covers to a disk cache. Covers are exposed by *path*; they
//! never travel through the IPC boundary as base64 or `Vec<u8>`.
//!
//! Public entry point: [`Library`]. Construct one with
//! [`Library::open`], then call [`Library::scan_folder`] to (re)index a
//! directory tree.

pub mod db;
pub mod model;
pub mod scan;

pub use db::Database;
pub use model::{
    Album, AlbumDetail, AlbumKind, AlbumWithKind, Artist, ArtistDetail, DsdRate, Genre,
    LibraryRoot, LibrarySource, LibraryStats, Playlist, PlaylistDetail, ScanProgress,
    SearchResults, SourceKind, Track,
};
pub use scan::index_remote_blob;
pub use scan::{scan_folder, ScanOptions, ScanReport};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;

/// All failure modes the library layer can produce.
#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("tag parsing error: {0}")]
    Tag(String),

    #[error("invalid path: {0}")]
    Path(String),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type LibraryResult<T> = Result<T, LibraryError>;

/// High-level handle that owns the SQLite connection and knows where the
/// cover cache lives on disk.
#[derive(Clone)]
pub struct Library {
    inner: Arc<LibraryInner>,
}

struct LibraryInner {
    db: Mutex<Database>,
    cover_cache_dir: PathBuf,
    db_path: PathBuf,
}

impl Library {
    /// Open (or create) the library database at `db_path` and ensure the
    /// cover cache directory exists at `cover_cache_dir`.
    pub fn open(db_path: &Path, cover_cache_dir: &Path) -> LibraryResult<Self> {
        std::fs::create_dir_all(cover_cache_dir)?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Database::open(db_path)?;
        Ok(Library {
            inner: Arc::new(LibraryInner {
                db: Mutex::new(db),
                cover_cache_dir: cover_cache_dir.to_path_buf(),
                db_path: db_path.to_path_buf(),
            }),
        })
    }

    /// Convenience constructor that uses the platform's standard data
    /// directory (e.g. `%APPDATA%\Qobee` on Windows).
    pub fn open_default() -> LibraryResult<Self> {
        let base = dirs::data_dir()
            .ok_or_else(|| LibraryError::Internal("no data dir on this platform".into()))?
            .join("Qobee");
        let db_path = base.join("library.sqlite3");
        let covers = base.join("covers");
        Self::open(&db_path, &covers)
    }

    pub fn cover_cache_dir(&self) -> &Path {
        &self.inner.cover_cache_dir
    }

    /// Indexes `root` recursively. Calls `progress` periodically with the
    /// current count of files seen / indexed; intended to be wired to a
    /// Tauri event so the UI can show a progress bar.
    pub fn scan_folder<F>(
        &self,
        root: &Path,
        options: ScanOptions,
        progress: F,
    ) -> LibraryResult<ScanReport>
    where
        F: FnMut(ScanProgress),
    {
        let report = scan_folder(
            root,
            &options,
            self.inner.cover_cache_dir.as_path(),
            progress,
            |track, cover_blob| {
                let mut db = self.inner.db.lock();
                db.upsert_track(track, cover_blob)
            },
        )?;
        Ok(report)
    }

    pub fn list_albums(&self) -> LibraryResult<Vec<Album>> {
        let db = self.inner.db.lock();
        db.list_albums()
    }

    pub fn list_artists(&self) -> LibraryResult<Vec<Artist>> {
        let db = self.inner.db.lock();
        db.list_artists()
    }

    pub fn get_album(&self, album_id: i64) -> LibraryResult<Option<AlbumDetail>> {
        let db = self.inner.db.lock();
        db.get_album(album_id)
    }

    pub fn get_track(&self, track_id: i64) -> LibraryResult<Option<Track>> {
        let db = self.inner.db.lock();
        db.get_track(track_id)
    }

    pub fn get_album_for_track(&self, track_id: i64) -> LibraryResult<Option<AlbumDetail>> {
        let db = self.inner.db.lock();
        db.get_album_for_track(track_id)
    }

    /// Build an artist detail page (albums grouped by kind) by name.
    pub fn get_artist_detail(&self, name: &str) -> LibraryResult<Option<ArtistDetail>> {
        let db = self.inner.db.lock();
        db.get_artist_detail(name)
    }

    /// Every track id for an artist (matched via album_artist, falling
    /// back to artist). Used to start playback from the artist page.
    pub fn track_ids_by_artist(&self, name: &str) -> LibraryResult<Vec<i64>> {
        let db = self.inner.db.lock();
        db.track_ids_by_artist(name)
    }

    /// Sum the byte size of every track file in `album_id`. Best-effort:
    /// missing files are silently skipped. Returns 0 when the album has
    /// no resolvable tracks.
    pub fn album_size_bytes(&self, album_id: i64) -> LibraryResult<u64> {
        let detail = match self.get_album(album_id)? {
            Some(d) => d,
            None => return Ok(0),
        };
        let mut total: u64 = 0;
        for t in detail.tracks.iter() {
            if let Ok(meta) = std::fs::metadata(&t.path) {
                total = total.saturating_add(meta.len());
            }
        }
        Ok(total)
    }

    pub fn search(&self, query: &str, limit: i64) -> LibraryResult<SearchResults> {
        let db = self.inner.db.lock();
        db.search(query, limit)
    }

    pub fn list_genres(&self) -> LibraryResult<Vec<Genre>> {
        let db = self.inner.db.lock();
        db.list_genres()
    }

    pub fn list_albums_by_genre(&self, genre: &str) -> LibraryResult<Vec<Album>> {
        let db = self.inner.db.lock();
        db.list_albums_by_genre(genre)
    }

    /// Record a play in the history table. `played_at` is unix seconds.
    pub fn record_play(&self, track_id: i64, played_at: i64) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.record_play(track_id, played_at)
    }

    pub fn recently_played(&self, limit: i64) -> LibraryResult<Vec<Track>> {
        let db = self.inner.db.lock();
        db.recently_played(limit)
    }

    pub fn recently_played_albums(&self, limit: i64) -> LibraryResult<Vec<Album>> {
        let db = self.inner.db.lock();
        db.recently_played_albums(limit)
    }

    pub fn recently_played_artists(&self, limit: i64) -> LibraryResult<Vec<Artist>> {
        let db = self.inner.db.lock();
        db.recently_played_artists(limit)
    }

    pub fn list_playlists(&self) -> LibraryResult<Vec<Playlist>> {
        let db = self.inner.db.lock();
        db.list_playlists()
    }

    pub fn create_playlist(&self, name: &str) -> LibraryResult<Playlist> {
        let mut db = self.inner.db.lock();
        db.create_playlist(name, now_secs())
    }

    pub fn delete_playlist(&self, playlist_id: i64) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.delete_playlist(playlist_id)
    }

    pub fn rename_playlist(&self, playlist_id: i64, new_name: &str) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.rename_playlist(playlist_id, new_name, now_secs())
    }

    /// Adds the given tracks to a playlist and returns the number of
    /// tracks actually inserted. The count is propagated up to the
    /// command layer and the UI so the front-end can confirm
    /// persistence (task 1.2, R3.2/R3.3).
    pub fn add_to_playlist(&self, playlist_id: i64, track_ids: &[i64]) -> LibraryResult<usize> {
        let mut db = self.inner.db.lock();
        db.add_to_playlist(playlist_id, track_ids, now_secs())
    }

    pub fn remove_from_playlist(&self, playlist_id: i64, position: i64) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.remove_from_playlist(playlist_id, position, now_secs())
    }

    pub fn get_playlist(&self, playlist_id: i64) -> LibraryResult<Option<PlaylistDetail>> {
        let db = self.inner.db.lock();
        db.get_playlist(playlist_id)
    }

    /// Find a track id by its absolute path. Used by Windows
    /// integration to resolve files passed on the command line or
    /// through `qobee://play?path=...` links.
    ///
    /// Returns `Ok(None)` if the path is not in the library — the
    /// caller is expected to scan the parent directory first if
    /// they want a one-shot import.
    pub fn find_track_id_by_path(&self, path: &str) -> LibraryResult<Option<i64>> {
        let db = self.inner.db.lock();
        db.find_track_id_by_path(path)
    }

    /// List every track whose path starts with `folder` (after path
    /// normalisation). Used by the "Play folder in Qobee" /
    /// "Add folder to queue" right-click verbs.
    pub fn tracks_in_folder(&self, folder: &str) -> LibraryResult<Vec<Track>> {
        let db = self.inner.db.lock();
        db.tracks_in_folder(folder)
    }

    // ---- Settings ----

    pub fn get_setting(&self, key: &str) -> LibraryResult<Option<String>> {
        let db = self.inner.db.lock();
        db.get_setting(key)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.set_setting(key, value)
    }

    pub fn list_settings(&self) -> LibraryResult<Vec<(String, String)>> {
        let db = self.inner.db.lock();
        db.list_settings()
    }

    pub fn clear_settings(&self) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.clear_settings()
    }

    /// Supprime tous les réglages dont la clé commence par `prefix` et
    /// retourne le nombre de lignes supprimées. Utilisé par la purge des
    /// liens de covers Discord (`discord.cover_url::*`).
    pub fn clear_settings_by_prefix(&self, prefix: &str) -> LibraryResult<usize> {
        let mut db = self.inner.db.lock();
        db.clear_settings_by_prefix(prefix)
    }

    // ---- Library roots ----

    pub fn add_library_root(&self, path: &str) -> LibraryResult<LibraryRoot> {
        let mut db = self.inner.db.lock();
        db.add_library_root(path, now_secs())
    }

    pub fn remove_library_root(&self, root_id: i64) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.remove_library_root(root_id)
    }

    pub fn list_library_roots(&self) -> LibraryResult<Vec<LibraryRoot>> {
        let db = self.inner.db.lock();
        db.list_library_roots()
    }

    // ---- Library sources (PR1: scaffolding) ----

    /// List sources stored in the dedicated `library_sources` table.
    /// Local folders are *not* projected here in PR1; the existing
    /// `list_library_roots` surface is left untouched. Once the
    /// Drive backend lands in PR3 we'll either unify both surfaces
    /// or migrate `library_roots` rows into this table — TBD.
    pub fn list_library_sources(&self) -> LibraryResult<Vec<LibrarySource>> {
        let db = self.inner.db.lock();
        db.list_library_sources()
    }

    pub fn add_library_source(
        &self,
        kind: SourceKind,
        name: &str,
        config_json: &str,
    ) -> LibraryResult<i64> {
        let mut db = self.inner.db.lock();
        db.add_library_source(kind, name, config_json, now_secs())
    }

    pub fn remove_library_source(&self, source_id: i64) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.remove_library_source(source_id)
    }

    pub fn set_library_source_enabled(&self, source_id: i64, enabled: bool) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.set_library_source_enabled(source_id, enabled)
    }

    /// Insert or update a track stored remotely. Mirrors
    /// [`Self::scan_folder`]'s upsert path but lets the caller (the
    /// Drive indexer) supply the synthetic path + parent source.
    pub fn upsert_remote_track(
        &self,
        track: &Track,
        source_id: i64,
        mtime: i64,
    ) -> LibraryResult<i64> {
        let mut db = self.inner.db.lock();
        db.upsert_remote_track(track, source_id, mtime)
    }

    pub fn delete_tracks_for_source(&self, source_id: i64) -> LibraryResult<usize> {
        let mut db = self.inner.db.lock();
        db.delete_tracks_for_source(source_id)
    }

    /// Drive file ids that are currently favorited for `source_id`.
    pub fn favorite_drive_file_ids(&self, source_id: i64) -> LibraryResult<Vec<String>> {
        let db = self.inner.db.lock();
        db.favorite_drive_file_ids(source_id)
    }

    /// Replace the favorites for `source_id` with the given list
    /// of Drive file ids.
    pub fn set_drive_favorites(
        &self,
        source_id: i64,
        drive_file_ids: &[String],
    ) -> LibraryResult<usize> {
        let mut db = self.inner.db.lock();
        db.set_drive_favorites(source_id, drive_file_ids, now_secs())
    }

    // ---- Favorites ----

    pub fn add_favorite(&self, track_id: i64) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.add_favorite(track_id, now_secs())
    }

    pub fn remove_favorite(&self, track_id: i64) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.remove_favorite(track_id)
    }

    pub fn is_favorite(&self, track_id: i64) -> LibraryResult<bool> {
        let db = self.inner.db.lock();
        db.is_favorite(track_id)
    }

    pub fn list_favorite_track_ids(&self) -> LibraryResult<Vec<i64>> {
        let db = self.inner.db.lock();
        db.list_favorite_track_ids()
    }

    // ---- Ratings & play counts ----

    /// Set a track's star rating (1..=5). `rating == 0` clears it.
    pub fn set_rating(&self, track_id: i64, rating: u8) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.set_rating(track_id, rating, now_secs())
    }

    /// A track's star rating, or `0` when unrated.
    pub fn get_rating(&self, track_id: i64) -> LibraryResult<u8> {
        let db = self.inner.db.lock();
        db.get_rating(track_id)
    }

    /// Number of recorded plays for a track.
    pub fn play_count(&self, track_id: i64) -> LibraryResult<i64> {
        let db = self.inner.db.lock();
        db.play_count(track_id)
    }

    // ---- Stats / maintenance ----

    pub fn library_stats(&self) -> LibraryResult<LibraryStats> {
        let (track_count, album_count, artist_count, playlist_count, history_count) = {
            let db = self.inner.db.lock();
            db.library_stats_partial()?
        };
        let db_size_bytes = std::fs::metadata(&self.inner.db_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let covers_size_bytes = dir_size_bytes(&self.inner.cover_cache_dir).unwrap_or(0);
        let data_dir = self
            .inner
            .db_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let covers_dir = self.inner.cover_cache_dir.to_string_lossy().into_owned();
        Ok(LibraryStats {
            track_count,
            album_count,
            artist_count,
            playlist_count,
            history_count,
            db_size_bytes,
            covers_size_bytes,
            data_dir,
            covers_dir,
        })
    }

    pub fn clear_history(&self) -> LibraryResult<()> {
        let mut db = self.inner.db.lock();
        db.clear_history()
    }

    pub fn clear_cover_cache(&self) -> LibraryResult<u64> {
        let dir = &self.inner.cover_cache_dir;
        let before = dir_size_bytes(dir).unwrap_or(0);
        if dir.is_dir() {
            // Remove every file but keep the directory itself.
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let p = entry.path();
                if p.is_dir() {
                    std::fs::remove_dir_all(&p)?;
                } else {
                    std::fs::remove_file(&p)?;
                }
            }
        }
        Ok(before)
    }

    /// Wipe library content (tracks, albums, playlists, history, roots,
    /// covers on disk). Settings are kept; call `clear_settings()` if
    /// you also want to reset preferences.
    pub fn wipe_all(&self) -> LibraryResult<()> {
        let _bytes = self.clear_cover_cache()?;
        let mut db = self.inner.db.lock();
        db.wipe_library()
    }
}

fn dir_size_bytes(path: &Path) -> std::io::Result<u64> {
    if !path.is_dir() {
        return Ok(0);
    }
    let mut total: u64 = 0;
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        for entry in std::fs::read_dir(&p)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
