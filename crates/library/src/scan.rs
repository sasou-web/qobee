//! Folder scanning + metadata extraction.
//!
//! Walks a directory tree, picks files whose extension matches one of
//! [`AUDIO_EXTENSIONS`], extracts metadata via [`lofty`], and writes any
//! embedded cover art to the on-disk cache. Tracks are passed to a
//! caller-supplied closure so the [`crate::Library`] can persist them in
//! a single transactional batch.

use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::Picture;
use lofty::probe::Probe;
use lofty::tag::Accessor;
use walkdir::WalkDir;

use crate::{
    model::{ScanProgress, Track},
    LibraryError, LibraryResult,
};

/// File extensions we currently accept. Everything else is silently
/// skipped; the scan does not fail on non-audio files.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "wav", "m4a", "mp4", "aac", "alac", "ogg", "opus", "aiff", "aif", "wv",
];

/// Knobs that affect how a scan runs.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Maximum recursion depth (None = unbounded).
    pub max_depth: Option<usize>,
    /// Follow symlinks while walking.
    pub follow_symlinks: bool,
    /// Skip files we have already indexed when their `mtime` is unchanged.
    /// (The DB is queried per-file by the caller; we just expose the flag
    /// here for symmetry — the MVP always re-indexes.)
    pub skip_unchanged: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            max_depth: None,
            follow_symlinks: false,
            skip_unchanged: false,
        }
    }
}

/// Result returned at the end of a scan.
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub files_visited: u64,
    pub files_indexed: u64,
    pub errors: Vec<String>,
}

/// Walk `root`, extract metadata, and yield each track to `on_track`.
///
/// `on_progress` is called periodically with a [`ScanProgress`] snapshot
/// so the UI can render a progress bar. `cover_cache_dir` is where
/// embedded picture bytes are written; the relative cache key is stored
/// on the track so the database never holds image bytes.
pub fn scan_folder<P, T>(
    root: &Path,
    options: &ScanOptions,
    cover_cache_dir: &Path,
    mut on_progress: P,
    mut on_track: T,
) -> LibraryResult<ScanReport>
where
    P: FnMut(ScanProgress),
    T: FnMut(&Track, Option<&[u8]>) -> LibraryResult<i64>,
{
    if !root.is_dir() {
        return Err(LibraryError::Path(format!(
            "scan root is not a directory: {}",
            root.display()
        )));
    }

    let mut walker = WalkDir::new(root).follow_links(options.follow_symlinks);
    if let Some(d) = options.max_depth {
        walker = walker.max_depth(d);
    }

    let mut report = ScanReport::default();
    let mut last_progress_emit = 0u64;

    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());

        let is_audio = ext.as_deref().map(|e| AUDIO_EXTENSIONS.contains(&e)).unwrap_or(false);
        if !is_audio {
            continue;
        }

        report.files_visited += 1;

        match index_one(&path, cover_cache_dir) {
            Ok((track, cover_blob)) => {
                match on_track(&track, cover_blob.as_deref()) {
                    Ok(_) => report.files_indexed += 1,
                    Err(e) => report.errors.push(format!(
                        "db error for {}: {}",
                        path.display(),
                        e
                    )),
                }
            }
            Err(e) => {
                report.errors.push(format!(
                    "metadata error for {}: {}",
                    path.display(),
                    e
                ));
            }
        }

        if report.files_visited - last_progress_emit >= 8 {
            last_progress_emit = report.files_visited;
            on_progress(ScanProgress {
                files_visited: report.files_visited,
                files_indexed: report.files_indexed,
                current: Some(path.display().to_string()),
            });
        }
    }

    on_progress(ScanProgress {
        files_visited: report.files_visited,
        files_indexed: report.files_indexed,
        current: None,
    });
    Ok(report)
}

/// Read tags + properties for a single file and return a [`Track`] plus
/// the raw cover blob (for the caller to persist in the cover cache).
fn index_one(path: &Path, cover_cache_dir: &Path) -> LibraryResult<(Track, Option<Vec<u8>>)> {
    let tagged = Probe::open(path)
        .map_err(|e| LibraryError::Tag(e.to_string()))?
        .read()
        .map_err(|e| LibraryError::Tag(e.to_string()))?;

    let properties = tagged.properties().clone();
    let primary_tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let title = primary_tag
        .and_then(|t| t.title().map(|s| s.to_string()))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown Title")
                .to_string()
        });

    let artist = primary_tag
        .and_then(|t| t.artist().map(|s| s.to_string()))
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let album_artist = primary_tag.and_then(|t| {
        t.get_string(lofty::tag::ItemKey::AlbumArtist)
            .map(|s| s.to_string())
    });

    let album = primary_tag
        .and_then(|t| t.album().map(|s| s.to_string()))
        .unwrap_or_else(|| "Unknown Album".to_string());

    let track_number = primary_tag.and_then(|t| t.track());
    let disc_number = primary_tag.and_then(|t| t.disk());
    let year = primary_tag
        .and_then(|t| t.get_string(lofty::tag::ItemKey::Year))
        .and_then(|s| s.parse::<i32>().ok())
        .or_else(|| {
            primary_tag
                .and_then(|t| t.get_string(lofty::tag::ItemKey::RecordingDate))
                .and_then(|s| s.get(..4).and_then(|y| y.parse::<i32>().ok()))
        });
    let genre = primary_tag.and_then(|t| t.genre().map(|s| s.to_string()));

    let duration_seconds = properties.duration().as_secs_f64();
    let sample_rate = properties.sample_rate();
    let bit_depth = properties.bit_depth();
    let channels = properties.channels().map(|c| c as u16);

    // ReplayGain. Lofty surfaces these as plain string items via
    // `ItemKey`. Values look like "-7.45 dB" or "-7.45". We strip the
    // unit and parse to f32; absent or unparseable -> None.
    let parse_rg = |s: &str| -> Option<f32> {
        s.split_whitespace()
            .next()
            .and_then(|t| t.parse::<f32>().ok())
            .map(|v| v.clamp(-30.0, 30.0))
    };
    let replaygain_track_db = primary_tag
        .and_then(|t| t.get_string(lofty::tag::ItemKey::ReplayGainTrackGain))
        .and_then(parse_rg);
    let replaygain_album_db = primary_tag
        .and_then(|t| t.get_string(lofty::tag::ItemKey::ReplayGainAlbumGain))
        .and_then(parse_rg);

    let cover_info = primary_tag
        .and_then(|t| t.pictures().first().cloned())
        .and_then(|p: Picture| persist_cover(&p, cover_cache_dir).ok().flatten());

    let (cover_key, cover_blob) = match cover_info {
        Some((key, blob)) => (Some(key), Some(blob)),
        None => (None, None),
    };

    let track = Track {
        id: 0,
        track_uid: String::new(),
        path: path.to_string_lossy().to_string(),
        title,
        artist,
        album,
        album_artist,
        track_number,
        disc_number,
        year,
        genre,
        duration_seconds,
        sample_rate,
        bit_depth: bit_depth.map(|b| b as u8),
        channels,
        cover_key,
        replaygain_track_db,
        replaygain_album_db,
    };

    Ok((track, cover_blob))
}

/// Write `picture` to the cover cache and return `(cache_key, bytes)`.
///
/// The cache key is `<aa>/<hash>.<ext>` where `aa` is the first two hex
/// chars of the blake3 hash. Sharding into 256 sub-folders keeps any
/// single directory from blowing up on huge libraries.
fn persist_cover(
    picture: &Picture,
    cover_cache_dir: &Path,
) -> LibraryResult<Option<(String, Vec<u8>)>> {
    let bytes = picture.data();
    if bytes.is_empty() {
        return Ok(None);
    }

    let ext = match picture.mime_type() {
        Some(mime) => match mime.as_str() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/png" => "png",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "bin",
        },
        None => "bin",
    };

    let hash = blake3::hash(bytes);
    let hex = hash.to_hex();
    let prefix = &hex.as_str()[..2];
    let filename = format!("{hex}.{ext}");
    let key = format!("{prefix}/{filename}");

    let dir = cover_cache_dir.join(prefix);
    std::fs::create_dir_all(&dir)?;
    let path: PathBuf = dir.join(&filename);

    if !path.exists() {
        std::fs::write(&path, bytes)?;
    }

    Ok(Some((key, bytes.to_vec())))
}
