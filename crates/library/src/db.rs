//! SQLite storage for tracks, albums, artists, and cover cache keys.
//!
//! Schema is tiny on purpose: three tables joined by string keys
//! (artist name, album title + album artist). The MVP optimizes for
//! "scan a folder, list everything" rather than relational purity.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::{
    model::{
        Album, AlbumDetail, Artist, Genre, LibraryRoot, Playlist, PlaylistDetail, SearchResults,
        Track,
    },
    LibraryError, LibraryResult,
};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tracks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    path            TEXT    NOT NULL UNIQUE,
    title           TEXT    NOT NULL,
    artist          TEXT    NOT NULL,
    album           TEXT    NOT NULL,
    album_artist    TEXT,
    track_number    INTEGER,
    disc_number     INTEGER,
    year            INTEGER,
    genre           TEXT,
    duration_seconds REAL   NOT NULL DEFAULT 0,
    sample_rate     INTEGER,
    bit_depth       INTEGER,
    channels        INTEGER,
    cover_key       TEXT,
    mtime           INTEGER NOT NULL DEFAULT 0,
    replaygain_track_db REAL,
    replaygain_album_db REAL,
    replaygain_track_peak REAL,
    replaygain_album_peak REAL
);

CREATE INDEX IF NOT EXISTS idx_tracks_album   ON tracks(album, album_artist);
CREATE INDEX IF NOT EXISTS idx_tracks_artist  ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_tracks_genre   ON tracks(genre);
CREATE INDEX IF NOT EXISTS idx_tracks_title   ON tracks(title COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS play_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id    INTEGER NOT NULL,
    played_at   INTEGER NOT NULL,
    FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_history_played_at ON play_history(played_at DESC);
CREATE INDEX IF NOT EXISTS idx_history_track     ON play_history(track_id);

CREATE TABLE IF NOT EXISTS playlists (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id INTEGER NOT NULL,
    track_id    INTEGER NOT NULL,
    position    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position),
    FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
    FOREIGN KEY(track_id)    REFERENCES tracks(id)    ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_playlist_tracks_pl ON playlist_tracks(playlist_id, position);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS library_roots (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT NOT NULL UNIQUE,
    added_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS favorite_tracks (
    track_id    INTEGER PRIMARY KEY,
    added_at    INTEGER NOT NULL,
    FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_favorite_tracks_added ON favorite_tracks(added_at DESC);
"#;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> LibraryResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        conn.execute_batch(SCHEMA)?;

        // Idempotent migrations for ReplayGain columns: SQLite does not
        // support `ADD COLUMN IF NOT EXISTS` in older versions, so we
        // probe and ignore the "duplicate column" error.
        for stmt in [
            "ALTER TABLE tracks ADD COLUMN replaygain_track_db REAL",
            "ALTER TABLE tracks ADD COLUMN replaygain_album_db REAL",
            "ALTER TABLE tracks ADD COLUMN replaygain_track_peak REAL",
            "ALTER TABLE tracks ADD COLUMN replaygain_album_peak REAL",
        ] {
            if let Err(e) = conn.execute(stmt, []) {
                let msg = e.to_string();
                if !msg.contains("duplicate column") {
                    tracing::debug!(target: "qobee::library", error = %msg, "schema migration: {}", stmt);
                }
            }
        }

        Ok(Database { conn })
    }

    /// Insert or update a single track. The cover bytes (if any) have
    /// already been written to the cache by the caller; we only store the
    /// cache key (relative path under the covers directory).
    pub fn upsert_track(
        &mut self,
        track: &Track,
        _cover_blob: Option<&[u8]>,
    ) -> LibraryResult<i64> {
        let mtime = std::fs::metadata(&track.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        self.conn.execute(
            r#"
            INSERT INTO tracks (
                path, title, artist, album, album_artist,
                track_number, disc_number, year, genre,
                duration_seconds, sample_rate, bit_depth, channels,
                cover_key, mtime, replaygain_track_db, replaygain_album_db,
                replaygain_track_peak, replaygain_album_peak
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
            ON CONFLICT(path) DO UPDATE SET
                title = excluded.title,
                artist = excluded.artist,
                album = excluded.album,
                album_artist = excluded.album_artist,
                track_number = excluded.track_number,
                disc_number = excluded.disc_number,
                year = excluded.year,
                genre = excluded.genre,
                duration_seconds = excluded.duration_seconds,
                sample_rate = excluded.sample_rate,
                bit_depth = excluded.bit_depth,
                channels = excluded.channels,
                cover_key = excluded.cover_key,
                mtime = excluded.mtime,
                replaygain_track_db = excluded.replaygain_track_db,
                replaygain_album_db = excluded.replaygain_album_db,
                replaygain_track_peak = excluded.replaygain_track_peak,
                replaygain_album_peak = excluded.replaygain_album_peak
            "#,
            params![
                track.path,
                track.title,
                track.artist,
                track.album,
                track.album_artist,
                track.track_number,
                track.disc_number,
                track.year,
                track.genre,
                track.duration_seconds,
                track.sample_rate,
                track.bit_depth,
                track.channels,
                track.cover_key,
                mtime,
                track.replaygain_track_db.map(|v| v as f64),
                track.replaygain_album_db.map(|v| v as f64),
                track.replaygain_track_peak.map(|v| v as f64),
                track.replaygain_album_peak.map(|v| v as f64),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_albums(&self) -> LibraryResult<Vec<Album>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                             AS id,
                album                               AS title,
                COALESCE(album_artist, artist)      AS artist,
                MIN(year)                           AS year,
                COUNT(*)                            AS track_count,
                MAX(cover_key)                      AS cover_key,
                COALESCE(SUM(duration_seconds), 0) AS total_duration,
                MAX(genre)                          AS genre,
                COALESCE(MAX(mtime), 0)             AS date_added
            FROM tracks
            GROUP BY album, COALESCE(album_artist, artist)
            ORDER BY artist COLLATE NOCASE, year, album COLLATE NOCASE
            "#,
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Album {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    year: row.get(3)?,
                    track_count: row.get(4)?,
                    cover_key: row.get(5)?,
                    total_duration_seconds: row.get::<_, f64>(6).unwrap_or(0.0),
                    total_size_bytes: 0,
                    genre: row.get(7)?,
                    date_added: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_artists(&self) -> LibraryResult<Vec<Artist>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                                    AS id,
                COALESCE(album_artist, artist)             AS name,
                COUNT(DISTINCT album)                      AS album_count,
                COUNT(*)                                   AS track_count,
                MAX(cover_key)                             AS cover_key
            FROM tracks
            GROUP BY COALESCE(album_artist, artist)
            ORDER BY name COLLATE NOCASE
            "#,
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Artist {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    album_count: row.get(2)?,
                    track_count: row.get(3)?,
                    cover_key: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_album(&self, album_id: i64) -> LibraryResult<Option<AlbumDetail>> {
        // Resolve "album_id" by looking up the seed track that exposed
        // it in `list_albums` (we use MIN(id) as the album's id).
        let row = self.conn.query_row(
            "SELECT album, COALESCE(album_artist, artist) FROM tracks WHERE id = ?1",
            params![album_id],
            |row| {
                let title: String = row.get(0)?;
                let artist: String = row.get(1)?;
                Ok((title, artist))
            },
        );
        let (title, artist) = match row {
            Ok(t) => t,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(LibraryError::Db(e)),
        };

        let mut album_stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                             AS id,
                album                               AS title,
                COALESCE(album_artist, artist)      AS artist,
                MIN(year)                           AS year,
                COUNT(*)                            AS track_count,
                MAX(cover_key)                      AS cover_key,
                COALESCE(SUM(duration_seconds), 0) AS total_duration,
                MAX(genre)                          AS genre,
                COALESCE(MAX(mtime), 0)             AS date_added
            FROM tracks
            WHERE album = ?1 AND COALESCE(album_artist, artist) = ?2
            "#,
        )?;
        let album = album_stmt.query_row(params![title, artist], |row| {
            Ok(Album {
                id: row.get(0)?,
                title: row.get(1)?,
                artist: row.get(2)?,
                year: row.get(3)?,
                track_count: row.get(4)?,
                cover_key: row.get(5)?,
                total_duration_seconds: row.get::<_, f64>(6).unwrap_or(0.0),
                total_size_bytes: 0,
                genre: row.get(7)?,
                date_added: row.get(8)?,
            })
        })?;

        let mut tracks_stmt = self.conn.prepare(
            r#"
            SELECT id, path, title, artist, album, album_artist,
                   track_number, disc_number, year, genre,
                   duration_seconds, sample_rate, bit_depth, channels, cover_key, replaygain_track_db, replaygain_album_db,
                   replaygain_track_peak, replaygain_album_peak
            FROM tracks
            WHERE album = ?1 AND COALESCE(album_artist, artist) = ?2
            ORDER BY COALESCE(disc_number, 1), COALESCE(track_number, 0), title COLLATE NOCASE
            "#,
        )?;
        let tracks = tracks_stmt
            .query_map(params![title, artist], track_from_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(AlbumDetail { album, tracks }))
    }

    pub fn get_track(&self, track_id: i64) -> LibraryResult<Option<Track>> {
        let track = self
            .conn
            .query_row(
                r#"
                SELECT id, path, title, artist, album, album_artist,
                       track_number, disc_number, year, genre,
                       duration_seconds, sample_rate, bit_depth, channels, cover_key, replaygain_track_db, replaygain_album_db,
                       replaygain_track_peak, replaygain_album_peak
                FROM tracks
                WHERE id = ?1
                "#,
                params![track_id],
                track_from_row,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(LibraryError::Db(other)),
            })?;
        Ok(track)
    }

    /// Resolve a track by its absolute file system path. The lookup
    /// matches on the `path` column verbatim, so callers must
    /// canonicalise (or at least normalise) the path before calling.
    pub fn find_track_id_by_path(&self, path: &str) -> LibraryResult<Option<i64>> {
        let id = self
            .conn
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                params![path],
                |row| row.get::<_, i64>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(LibraryError::Db(other)),
            })?;
        Ok(id)
    }

    /// Every track whose `path` starts with `folder`. The match is
    /// done with `LIKE folder || '%'` and is case-sensitive on
    /// platforms with case-sensitive filesystems; on Windows the
    /// caller can pre-lowercase both sides if needed.
    pub fn tracks_in_folder(&self, folder: &str) -> LibraryResult<Vec<Track>> {
        let pattern = format!("{}%", folder);
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, path, title, artist, album, album_artist,
                   track_number, disc_number, year, genre,
                   duration_seconds, sample_rate, bit_depth, channels,
                   cover_key, replaygain_track_db, replaygain_album_db,
                   replaygain_track_peak, replaygain_album_peak
            FROM tracks
            WHERE path LIKE ?1 ESCAPE '\'
            ORDER BY album, disc_number, track_number, title
            "#,
        )?;
        let rows = stmt
            .query_map(params![pattern], track_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Resolve the album that contains `track_id`, with all its tracks
    /// in playback order. Used by `play_track` to queue the surrounding
    /// album when the user clicks a single song.
    pub fn get_album_for_track(&self, track_id: i64) -> LibraryResult<Option<AlbumDetail>> {
        let row = self.conn.query_row(
            "SELECT album, COALESCE(album_artist, artist) FROM tracks WHERE id = ?1",
            params![track_id],
            |row| {
                let title: String = row.get(0)?;
                let artist: String = row.get(1)?;
                Ok((title, artist))
            },
        );
        let (title, artist) = match row {
            Ok(t) => t,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(LibraryError::Db(e)),
        };

        let mut album_stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                             AS id,
                album                               AS title,
                COALESCE(album_artist, artist)      AS artist,
                MIN(year)                           AS year,
                COUNT(*)                            AS track_count,
                MAX(cover_key)                      AS cover_key,
                COALESCE(SUM(duration_seconds), 0) AS total_duration,
                MAX(genre)                          AS genre,
                COALESCE(MAX(mtime), 0)             AS date_added
            FROM tracks
            WHERE album = ?1 AND COALESCE(album_artist, artist) = ?2
            "#,
        )?;
        let album = album_stmt.query_row(params![title, artist], |row| {
            Ok(Album {
                id: row.get(0)?,
                title: row.get(1)?,
                artist: row.get(2)?,
                year: row.get(3)?,
                track_count: row.get(4)?,
                cover_key: row.get(5)?,
                total_duration_seconds: row.get::<_, f64>(6).unwrap_or(0.0),
                total_size_bytes: 0,
                genre: row.get(7)?,
                date_added: row.get(8)?,
            })
        })?;

        let mut tracks_stmt = self.conn.prepare(
            r#"
            SELECT id, path, title, artist, album, album_artist,
                   track_number, disc_number, year, genre,
                   duration_seconds, sample_rate, bit_depth, channels, cover_key, replaygain_track_db, replaygain_album_db,
                   replaygain_track_peak, replaygain_album_peak
            FROM tracks
            WHERE album = ?1 AND COALESCE(album_artist, artist) = ?2
            ORDER BY COALESCE(disc_number, 1), COALESCE(track_number, 0), title COLLATE NOCASE
            "#,
        )?;
        let tracks = tracks_stmt
            .query_map(params![title, artist], track_from_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(AlbumDetail { album, tracks }))
    }

    // ------------------------------------------------------------------
    // Search
    // ------------------------------------------------------------------

    /// Multi-section search across tracks, albums and artists. Each
    /// section is independently capped at `limit` rows. Matching is
    /// case-insensitive substring; the MVP uses SQLite `LIKE` rather
    /// than FTS to keep the schema small.
    pub fn search(&self, query: &str, limit: i64) -> LibraryResult<SearchResults> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(SearchResults {
                query: trimmed.to_string(),
                ..Default::default()
            });
        }
        let pattern = format!("%{}%", trimmed.replace('%', r"\%").replace('_', r"\_"));

        let mut tracks_stmt = self.conn.prepare(
            r#"
            SELECT id, path, title, artist, album, album_artist,
                   track_number, disc_number, year, genre,
                   duration_seconds, sample_rate, bit_depth, channels, cover_key, replaygain_track_db, replaygain_album_db,
                   replaygain_track_peak, replaygain_album_peak
            FROM tracks
            WHERE title  LIKE ?1 ESCAPE '\'
               OR artist LIKE ?1 ESCAPE '\'
               OR album  LIKE ?1 ESCAPE '\'
            ORDER BY
                CASE
                    WHEN title  LIKE ?1 ESCAPE '\' THEN 0
                    WHEN artist LIKE ?1 ESCAPE '\' THEN 1
                    ELSE 2
                END,
                title COLLATE NOCASE
            LIMIT ?2
            "#,
        )?;
        let tracks = tracks_stmt
            .query_map(params![pattern, limit], track_from_row)?
            .collect::<Result<Vec<_>, _>>()?;

        let mut albums_stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                             AS id,
                album                               AS title,
                COALESCE(album_artist, artist)      AS artist,
                MIN(year)                           AS year,
                COUNT(*)                            AS track_count,
                MAX(cover_key)                      AS cover_key,
                COALESCE(SUM(duration_seconds), 0) AS total_duration,
                MAX(genre)                          AS genre,
                COALESCE(MAX(mtime), 0)             AS date_added
            FROM tracks
            WHERE album LIKE ?1 ESCAPE '\'
               OR album_artist LIKE ?1 ESCAPE '\'
               OR artist LIKE ?1 ESCAPE '\'
            GROUP BY album, COALESCE(album_artist, artist)
            ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE
            LIMIT ?2
            "#,
        )?;
        let albums = albums_stmt
            .query_map(params![pattern, limit], |row| {
                Ok(Album {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    year: row.get(3)?,
                    track_count: row.get(4)?,
                    cover_key: row.get(5)?,
                    total_duration_seconds: row.get::<_, f64>(6).unwrap_or(0.0),
                    total_size_bytes: 0,
                    genre: row.get(7)?,
                    date_added: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut artists_stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                                    AS id,
                COALESCE(album_artist, artist)             AS name,
                COUNT(DISTINCT album)                      AS album_count,
                COUNT(*)                                   AS track_count,
                MAX(cover_key)                             AS cover_key
            FROM tracks
            WHERE COALESCE(album_artist, artist) LIKE ?1 ESCAPE '\'
            GROUP BY COALESCE(album_artist, artist)
            ORDER BY name COLLATE NOCASE
            LIMIT ?2
            "#,
        )?;
        let artists = artists_stmt
            .query_map(params![pattern, limit], |row| {
                Ok(Artist {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    album_count: row.get(2)?,
                    track_count: row.get(3)?,
                    cover_key: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(SearchResults {
            query: trimmed.to_string(),
            tracks,
            albums,
            artists,
        })
    }

    // ------------------------------------------------------------------
    // Genres
    // ------------------------------------------------------------------

    pub fn list_genres(&self) -> LibraryResult<Vec<Genre>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                genre                       AS name,
                COUNT(*)                    AS track_count,
                MAX(cover_key)              AS cover_key
            FROM tracks
            WHERE genre IS NOT NULL AND TRIM(genre) <> ''
            GROUP BY genre
            ORDER BY track_count DESC, name COLLATE NOCASE
            "#,
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Genre {
                    name: row.get(0)?,
                    track_count: row.get(1)?,
                    cover_key: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_albums_by_genre(&self, genre: &str) -> LibraryResult<Vec<Album>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                             AS id,
                album                               AS title,
                COALESCE(album_artist, artist)      AS artist,
                MIN(year)                           AS year,
                COUNT(*)                            AS track_count,
                MAX(cover_key)                      AS cover_key,
                COALESCE(SUM(duration_seconds), 0) AS total_duration,
                MAX(genre)                          AS genre,
                COALESCE(MAX(mtime), 0)             AS date_added
            FROM tracks
            WHERE genre = ?1
            GROUP BY album, COALESCE(album_artist, artist)
            ORDER BY artist COLLATE NOCASE, year, album COLLATE NOCASE
            "#,
        )?;
        let rows = stmt
            .query_map(params![genre], |row| {
                Ok(Album {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    year: row.get(3)?,
                    track_count: row.get(4)?,
                    cover_key: row.get(5)?,
                    total_duration_seconds: row.get::<_, f64>(6).unwrap_or(0.0),
                    total_size_bytes: 0,
                    genre: row.get(7)?,
                    date_added: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Play history
    // ------------------------------------------------------------------

    /// Record that `track_id` was played at `played_at` (unix seconds).
    pub fn record_play(&mut self, track_id: i64, played_at: i64) -> LibraryResult<()> {
        self.conn.execute(
            "INSERT INTO play_history (track_id, played_at) VALUES (?1, ?2)",
            params![track_id, played_at],
        )?;
        Ok(())
    }

    /// Recently played tracks, deduplicated by `track_id` (only the most
    /// recent play wins). Returned in reverse chronological order.
    pub fn recently_played(&self, limit: i64) -> LibraryResult<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT t.id, t.path, t.title, t.artist, t.album, t.album_artist,
                   t.track_number, t.disc_number, t.year, t.genre,
                   t.duration_seconds, t.sample_rate, t.bit_depth, t.channels, t.cover_key, t.replaygain_track_db, t.replaygain_album_db,
                   t.replaygain_track_peak, t.replaygain_album_peak
            FROM (
                SELECT track_id, MAX(played_at) AS played_at
                FROM play_history
                GROUP BY track_id
            ) h
            JOIN tracks t ON t.id = h.track_id
            ORDER BY h.played_at DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt
            .query_map(params![limit], track_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Recently played albums, ordered by the most recent play in each
    /// album.
    pub fn recently_played_albums(&self, limit: i64) -> LibraryResult<Vec<Album>> {
        let mut stmt = self.conn.prepare(
            r#"
            WITH latest_play AS (
                SELECT
                    t.album,
                    COALESCE(t.album_artist, t.artist) AS artist_key,
                    MAX(h.played_at)                   AS played_at
                FROM play_history h
                JOIN tracks t ON t.id = h.track_id
                GROUP BY t.album, COALESCE(t.album_artist, t.artist)
            )
            SELECT
                MIN(t.id)                                  AS id,
                t.album                                    AS title,
                COALESCE(t.album_artist, t.artist)         AS artist,
                MIN(t.year)                                AS year,
                COUNT(*)                                   AS track_count,
                MAX(t.cover_key)                           AS cover_key,
                COALESCE(SUM(t.duration_seconds), 0)       AS total_duration,
                MAX(t.genre)                               AS genre,
                COALESCE(MAX(t.mtime), 0)                  AS date_added
            FROM tracks t
            JOIN latest_play lp
                ON lp.album = t.album
               AND lp.artist_key = COALESCE(t.album_artist, t.artist)
            GROUP BY t.album, COALESCE(t.album_artist, t.artist)
            ORDER BY MAX(lp.played_at) DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(Album {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    year: row.get(3)?,
                    track_count: row.get(4)?,
                    cover_key: row.get(5)?,
                    total_duration_seconds: row.get::<_, f64>(6).unwrap_or(0.0),
                    total_size_bytes: 0,
                    genre: row.get(7)?,
                    date_added: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Recently played artists, ordered by the most recent play across
    /// any of their tracks. The cover key is borrowed from any of
    /// their tracks (same trick as `list_artists`).
    pub fn recently_played_artists(&self, limit: i64) -> LibraryResult<Vec<Artist>> {
        let mut stmt = self.conn.prepare(
            r#"
            WITH artist_play AS (
                SELECT
                    COALESCE(t.album_artist, t.artist) AS artist_key,
                    MAX(h.played_at)                   AS played_at
                FROM play_history h
                JOIN tracks t ON t.id = h.track_id
                GROUP BY COALESCE(t.album_artist, t.artist)
            )
            SELECT
                MIN(t.id)                                    AS id,
                COALESCE(t.album_artist, t.artist)           AS name,
                COUNT(DISTINCT t.album)                      AS album_count,
                COUNT(*)                                     AS track_count,
                MAX(t.cover_key)                             AS cover_key
            FROM tracks t
            JOIN artist_play ap
                ON ap.artist_key = COALESCE(t.album_artist, t.artist)
            GROUP BY COALESCE(t.album_artist, t.artist)
            ORDER BY MAX(ap.played_at) DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(Artist {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    album_count: row.get(2)?,
                    track_count: row.get(3)?,
                    cover_key: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Playlists
    // ------------------------------------------------------------------

    pub fn list_playlists(&self) -> LibraryResult<Vec<Playlist>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                p.id,
                p.name,
                p.created_at,
                p.updated_at,
                COUNT(pt.track_id) AS track_count,
                MAX(t.cover_key)   AS cover_key
            FROM playlists p
            LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
            LEFT JOIN tracks t           ON t.id = pt.track_id
            GROUP BY p.id, p.name, p.created_at, p.updated_at
            ORDER BY p.updated_at DESC, p.name COLLATE NOCASE
            "#,
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Playlist {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    track_count: row.get(4)?,
                    cover_key: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn create_playlist(&mut self, name: &str, now: i64) -> LibraryResult<Playlist> {
        self.conn.execute(
            "INSERT INTO playlists (name, created_at, updated_at) VALUES (?1, ?2, ?2)",
            params![name, now],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(Playlist {
            id,
            name: name.to_string(),
            created_at: now,
            updated_at: now,
            track_count: 0,
            cover_key: None,
        })
    }

    pub fn delete_playlist(&mut self, playlist_id: i64) -> LibraryResult<()> {
        self.conn
            .execute("DELETE FROM playlists WHERE id = ?1", params![playlist_id])?;
        Ok(())
    }

    pub fn rename_playlist(
        &mut self,
        playlist_id: i64,
        new_name: &str,
        now: i64,
    ) -> LibraryResult<()> {
        self.conn.execute(
            "UPDATE playlists SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_name, now, playlist_id],
        )?;
        Ok(())
    }

    pub fn add_to_playlist(
        &mut self,
        playlist_id: i64,
        track_ids: &[i64],
        now: i64,
    ) -> LibraryResult<()> {
        let tx = self.conn.transaction()?;
        let next_position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks WHERE playlist_id = ?1",
                params![playlist_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        {
            let mut stmt = tx.prepare(
                "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
            )?;
            for (i, track_id) in track_ids.iter().enumerate() {
                stmt.execute(params![playlist_id, track_id, next_position + i as i64])?;
            }
        }
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now, playlist_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_from_playlist(
        &mut self,
        playlist_id: i64,
        position: i64,
        now: i64,
    ) -> LibraryResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND position = ?2",
            params![playlist_id, position],
        )?;
        // Compact positions so cursor / "next" semantics stay simple.
        tx.execute(
            r#"
            UPDATE playlist_tracks
               SET position = position - 1
             WHERE playlist_id = ?1 AND position > ?2
            "#,
            params![playlist_id, position],
        )?;
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now, playlist_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_playlist(&self, playlist_id: i64) -> LibraryResult<Option<PlaylistDetail>> {
        let pl = self.conn.query_row(
            r#"
            SELECT
                p.id,
                p.name,
                p.created_at,
                p.updated_at,
                (SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = p.id) AS track_count,
                (SELECT t2.cover_key
                   FROM playlist_tracks pt2
                   JOIN tracks t2 ON t2.id = pt2.track_id
                  WHERE pt2.playlist_id = p.id
                  ORDER BY pt2.position
                  LIMIT 1) AS cover_key
            FROM playlists p
            WHERE p.id = ?1
            "#,
            params![playlist_id],
            |row| {
                Ok(Playlist {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    track_count: row.get(4)?,
                    cover_key: row.get(5)?,
                })
            },
        );
        let playlist = match pl {
            Ok(p) => p,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(LibraryError::Db(e)),
        };

        let mut stmt = self.conn.prepare(
            r#"
            SELECT t.id, t.path, t.title, t.artist, t.album, t.album_artist,
                   t.track_number, t.disc_number, t.year, t.genre,
                   t.duration_seconds, t.sample_rate, t.bit_depth, t.channels, t.cover_key, t.replaygain_track_db, t.replaygain_album_db,
                   t.replaygain_track_peak, t.replaygain_album_peak
            FROM playlist_tracks pt
            JOIN tracks t ON t.id = pt.track_id
            WHERE pt.playlist_id = ?1
            ORDER BY pt.position
            "#,
        )?;
        let tracks = stmt
            .query_map(params![playlist_id], track_from_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(PlaylistDetail { playlist, tracks }))
    }

    // ------------------------------------------------------------------
    // Settings (key/value)
    // ------------------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> LibraryResult<Option<String>> {
        let r = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(LibraryError::Db(other)),
            })?;
        Ok(r)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> LibraryResult<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn list_settings(&self) -> LibraryResult<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Library roots
    // ------------------------------------------------------------------

    pub fn add_library_root(&mut self, path: &str, now: i64) -> LibraryResult<LibraryRoot> {
        self.conn.execute(
            "INSERT INTO library_roots (path, added_at) VALUES (?1, ?2)
             ON CONFLICT(path) DO NOTHING",
            params![path, now],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM library_roots WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )?;
        Ok(LibraryRoot {
            id,
            path: path.to_string(),
            added_at: now,
        })
    }

    pub fn remove_library_root(&mut self, root_id: i64) -> LibraryResult<()> {
        self.conn
            .execute("DELETE FROM library_roots WHERE id = ?1", params![root_id])?;
        Ok(())
    }

    pub fn list_library_roots(&self) -> LibraryResult<Vec<LibraryRoot>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, added_at FROM library_roots ORDER BY added_at DESC")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(LibraryRoot {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    added_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Favorites
    // ------------------------------------------------------------------

    /// Mark `track_id` as a favorite. Idempotent: re-favoriting an
    /// already-favorited track silently succeeds.
    pub fn add_favorite(&mut self, track_id: i64, now: i64) -> LibraryResult<()> {
        self.conn.execute(
            "INSERT INTO favorite_tracks (track_id, added_at) VALUES (?1, ?2)
             ON CONFLICT(track_id) DO NOTHING",
            params![track_id, now],
        )?;
        Ok(())
    }

    pub fn remove_favorite(&mut self, track_id: i64) -> LibraryResult<()> {
        self.conn.execute(
            "DELETE FROM favorite_tracks WHERE track_id = ?1",
            params![track_id],
        )?;
        Ok(())
    }

    pub fn is_favorite(&self, track_id: i64) -> LibraryResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM favorite_tracks WHERE track_id = ?1",
            params![track_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn list_favorite_track_ids(&self) -> LibraryResult<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT track_id FROM favorite_tracks ORDER BY added_at DESC")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Stats / maintenance
    // ------------------------------------------------------------------

    pub fn library_stats_partial(&self) -> LibraryResult<(i64, i64, i64, i64, i64)> {
        let track_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
        let album_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (SELECT 1 FROM tracks GROUP BY album, COALESCE(album_artist, artist))",
            [],
            |r| r.get(0),
        )?;
        let artist_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (SELECT 1 FROM tracks GROUP BY COALESCE(album_artist, artist))",
            [],
            |r| r.get(0),
        )?;
        let playlist_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0))?;
        let history_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM play_history", [], |r| r.get(0))?;
        Ok((
            track_count,
            album_count,
            artist_count,
            playlist_count,
            history_count,
        ))
    }

    pub fn clear_history(&mut self) -> LibraryResult<()> {
        self.conn.execute("DELETE FROM play_history", [])?;
        Ok(())
    }

    /// Wipe every user-visible row but keep the schema. Equivalent to
    /// "factory reset" without removing the DB file from disk.
    pub fn wipe_library(&mut self) -> LibraryResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM playlist_tracks", [])?;
        tx.execute("DELETE FROM playlists", [])?;
        tx.execute("DELETE FROM play_history", [])?;
        tx.execute("DELETE FROM favorite_tracks", [])?;
        tx.execute("DELETE FROM tracks", [])?;
        tx.execute("DELETE FROM library_roots", [])?;
        // Settings are kept on purpose; they are user preferences, not
        // library data. The "settings" command can clear them too.
        tx.commit()?;
        // Reclaim the freed pages.
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    pub fn clear_settings(&mut self) -> LibraryResult<()> {
        self.conn.execute("DELETE FROM settings", [])?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Artist detail
    // ------------------------------------------------------------------

    /// Every track id by `name` (matched against `album_artist`,
    /// falling back to `artist`). Used to start playback from an
    /// artist page (e.g. "Play all" / shuffle).
    pub fn track_ids_by_artist(&self, name: &str) -> LibraryResult<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id
            FROM tracks
            WHERE COALESCE(album_artist, artist) = ?1
            ORDER BY year, album COLLATE NOCASE,
                     disc_number, track_number, title COLLATE NOCASE
            "#,
        )?;
        let rows = stmt
            .query_map(params![name], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_artist_detail(&self, name: &str) -> LibraryResult<Option<crate::ArtistDetail>> {
        let mut album_stmt = self.conn.prepare(
            r#"
            SELECT
                MIN(id)                             AS id,
                album                               AS title,
                COALESCE(album_artist, artist)      AS artist,
                MIN(year)                           AS year,
                COUNT(*)                            AS track_count,
                MAX(cover_key)                      AS cover_key,
                COALESCE(SUM(duration_seconds), 0) AS total_duration,
                MAX(genre)                          AS genre,
                COALESCE(MAX(mtime), 0)             AS date_added
            FROM tracks
            WHERE COALESCE(album_artist, artist) = ?1
            GROUP BY album
            ORDER BY year, album COLLATE NOCASE
            "#,
        )?;
        let albums: Vec<Album> = album_stmt
            .query_map(params![name], |row| {
                Ok(Album {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist: row.get(2)?,
                    year: row.get(3)?,
                    track_count: row.get(4)?,
                    cover_key: row.get(5)?,
                    total_duration_seconds: row.get::<_, f64>(6).unwrap_or(0.0),
                    total_size_bytes: 0,
                    genre: row.get(7)?,
                    date_added: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if albums.is_empty() {
            return Ok(None);
        }

        let total_tracks: i64 = albums.iter().map(|a| a.track_count).sum();

        let with_kind: Vec<crate::AlbumWithKind> = albums
            .into_iter()
            .map(|album| {
                let kind = classify_album(album.track_count, album.total_duration_seconds);
                let duration = album.total_duration_seconds;
                crate::AlbumWithKind {
                    album,
                    kind,
                    duration_seconds: duration,
                }
            })
            .collect();

        Ok(Some(crate::ArtistDetail {
            name: name.to_string(),
            track_count: total_tracks,
            albums: with_kind,
        }))
    }
}

/// Classify an album as Single / EP / Album based on simple heuristics.
/// We deliberately avoid relying on tags here: most ripped libraries
/// don't carry an "AlbumType" tag, but track count + duration is a
/// fairly reliable proxy.
fn classify_album(track_count: i64, total_duration_seconds: f64) -> crate::AlbumKind {
    let mins = total_duration_seconds / 60.0;
    if track_count <= 3 || mins < 12.0 {
        crate::AlbumKind::Single
    } else if track_count <= 6 || mins < 30.0 {
        crate::AlbumKind::Ep
    } else {
        crate::AlbumKind::Album
    }
}

fn track_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
    let id: i64 = row.get(0)?;
    Ok(Track {
        id,
        track_uid: id.to_string(),
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        album_artist: row.get(5)?,
        track_number: row.get::<_, Option<i64>>(6)?.map(|v| v as u32),
        disc_number: row.get::<_, Option<i64>>(7)?.map(|v| v as u32),
        year: row.get(8)?,
        genre: row.get(9)?,
        duration_seconds: row.get(10)?,
        sample_rate: row.get::<_, Option<i64>>(11)?.map(|v| v as u32),
        bit_depth: row.get::<_, Option<i64>>(12)?.map(|v| v as u8),
        channels: row.get::<_, Option<i64>>(13)?.map(|v| v as u16),
        cover_key: row.get(14)?,
        replaygain_track_db: row
            .get::<_, Option<f64>>(15)
            .ok()
            .flatten()
            .map(|v| v as f32),
        replaygain_album_db: row
            .get::<_, Option<f64>>(16)
            .ok()
            .flatten()
            .map(|v| v as f32),
        replaygain_track_peak: row
            .get::<_, Option<f64>>(17)
            .ok()
            .flatten()
            .map(|v| v as f32),
        replaygain_album_peak: row
            .get::<_, Option<f64>>(18)
            .ok()
            .flatten()
            .map(|v| v as f32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic in-memory `Track` that does not refer to any
    /// real file on disk. `upsert_track` calls `fs::metadata(...)` to
    /// fetch a mtime; the call fails silently and we get `mtime = 0`,
    /// which is fine for the round-trip tests.
    fn make_track(path: &str, peak: Option<f32>, album_peak: Option<f32>) -> Track {
        Track {
            id: 0,
            track_uid: String::new(),
            path: path.to_string(),
            title: "Title".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_artist: None,
            track_number: None,
            disc_number: None,
            year: None,
            genre: None,
            duration_seconds: 0.0,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            replaygain_track_db: None,
            replaygain_album_db: None,
            replaygain_track_peak: peak,
            replaygain_album_peak: album_peak,
            cover_key: None,
        }
    }

    /// PRAGMA table_info returns column metadata. We assert the two
    /// new peak columns are present after `Database::open`, regardless
    /// of whether the table was created from `SCHEMA` or migrated via
    /// the idempotent `ALTER TABLE` statements.
    #[test]
    fn replaygain_peak_columns_exist_after_open() {
        let db = Database::open(Path::new(":memory:")).expect("open in-memory db");
        let mut stmt = db
            .conn
            .prepare("PRAGMA table_info(tracks)")
            .expect("pragma");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        assert!(
            columns.iter().any(|c| c == "replaygain_track_peak"),
            "expected `replaygain_track_peak` in columns: {columns:?}",
        );
        assert!(
            columns.iter().any(|c| c == "replaygain_album_peak"),
            "expected `replaygain_album_peak` in columns: {columns:?}",
        );
    }

    /// Inserting peak values must round-trip back through `get_track`
    /// at f32 precision. We use a generous epsilon since the column is
    /// stored as REAL (f64) and the model is f32.
    #[test]
    fn track_peak_round_trip_with_values() {
        let mut db = Database::open(Path::new(":memory:")).expect("open in-memory db");
        let track = make_track(
            "/synthetic/path/with-peaks.flac",
            Some(0.987654),
            Some(1.0234),
        );
        let id = db.upsert_track(&track, None).expect("upsert");
        let got = db.get_track(id).expect("get_track").expect("track exists");
        let tp = got.replaygain_track_peak.expect("track peak set");
        let ap = got.replaygain_album_peak.expect("album peak set");
        assert!(
            (tp - 0.987654_f32).abs() < 1e-5,
            "track peak round-trip: got {tp}"
        );
        assert!(
            (ap - 1.0234_f32).abs() < 1e-5,
            "album peak round-trip: got {ap}"
        );
    }

    /// `None` peaks (the common case for non-RG-tagged files) must
    /// survive the round-trip as `None`, not as `Some(0.0)` or a
    /// silent default.
    #[test]
    fn track_peak_round_trip_with_none() {
        let mut db = Database::open(Path::new(":memory:")).expect("open in-memory db");
        let track = make_track("/synthetic/path/no-peaks.flac", None, None);
        let id = db.upsert_track(&track, None).expect("upsert");
        let got = db.get_track(id).expect("get_track").expect("track exists");
        assert!(
            got.replaygain_track_peak.is_none(),
            "expected None, got {:?}",
            got.replaygain_track_peak
        );
        assert!(
            got.replaygain_album_peak.is_none(),
            "expected None, got {:?}",
            got.replaygain_album_peak
        );
    }
}
