//! Plain data structures shared between the library, the orchestration
//! layer, and the Tauri command surface.
//!
//! Everything here is `Serialize` so it can be returned to the frontend
//! verbatim. Cover information is stored as a *path* (a relative cache
//! key resolved against the cover cache directory at runtime); raw bytes
//! are never serialized.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    /// Stable identifier exposed to the frontend (string-encoded `id`).
    pub track_uid: String,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration_seconds: f64,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u16>,
    /// ReplayGain track gain in dB (from `REPLAYGAIN_TRACK_GAIN` /
    /// `R128_TRACK_GAIN`). `None` if the file has no RG metadata.
    pub replaygain_track_db: Option<f32>,
    /// ReplayGain album gain in dB (from `REPLAYGAIN_ALBUM_GAIN` /
    /// `R128_ALBUM_GAIN`). `None` if the file has no RG metadata.
    pub replaygain_album_db: Option<f32>,
    /// Cache key (filename under the cover cache dir, e.g.
    /// `ab/abcdef….jpg`) when an embedded cover was extracted, else
    /// `None`.
    pub cover_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
    pub track_count: i64,
    pub cover_key: Option<String>,
    /// Total duration in seconds across every track in the album.
    pub total_duration_seconds: f64,
    /// Sum of file sizes on disk, in bytes. Best-effort: missing files
    /// are silently skipped. Computed lazily by the UI when the field
    /// is needed.
    pub total_size_bytes: u64,
    /// Representative genre across the album's tracks (lexicographic
    /// max of the non-null per-track tags). `None` when no track in
    /// the album carries a genre tag. Used by the UI for the "Genre"
    /// sort criterion.
    pub genre: Option<String>,
    /// Unix timestamp (seconds) standing in for "when the album was
    /// added to the library". The library has no dedicated insertion
    /// column today, so we use the freshest file mtime across the
    /// album's tracks as a proxy: re-scanning a folder picks up newly
    /// dropped files without false positives on existing ones.
    pub date_added: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub album_count: i64,
    pub track_count: i64,
    /// Any cover key from one of the artist's tracks. Used by the
    /// Artists grid page to render a thumbnail next to the name. May
    /// be `None` for fully untagged libraries.
    pub cover_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlbumKind {
    Single,
    Ep,
    Album,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumWithKind {
    pub album: Album,
    pub kind: AlbumKind,
    /// Total duration in seconds (sum over all tracks).
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistDetail {
    pub name: String,
    pub track_count: i64,
    pub albums: Vec<AlbumWithKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumDetail {
    pub album: Album,
    pub tracks: Vec<Track>,
}

/// A genre row (one per distinct `genre` string in the library, plus a
/// summary track count and a representative cover).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Genre {
    pub name: String,
    pub track_count: i64,
    pub cover_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub track_count: i64,
    pub cover_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    pub tracks: Vec<Track>,
}

/// Aggregated search result. Each section is independently capped on
/// the SQL side so a single query stays cheap on large libraries.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchResults {
    pub query: String,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryRoot {
    pub id: i64,
    pub path: String,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryStats {
    pub track_count: i64,
    pub album_count: i64,
    pub artist_count: i64,
    pub playlist_count: i64,
    pub history_count: i64,
    pub db_size_bytes: u64,
    pub covers_size_bytes: u64,
    pub data_dir: String,
    pub covers_dir: String,
}

/// Progress event surfaced during a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    /// Total files visited so far (any extension).
    pub files_visited: u64,
    /// Files whose tags we successfully indexed.
    pub files_indexed: u64,
    /// Path of the most recently indexed file (for UI display).
    pub current: Option<String>,
}
