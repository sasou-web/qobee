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
///
/// `dsf` and `dff` are DSD container formats added by the
/// audio-quality-improvements spec (R7.1). Lofty ≥ 0.24 surfaces
/// their tags (DSF carries ID3v2, DFF an internal metadata block);
/// the engine's parsers in `crates/engine/src/dsd/` decode the bit
/// stream itself.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "wav", "m4a", "mp4", "aac", "alac", "ogg", "opus", "aiff", "aif", "wv", "dsf",
    "dff",
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
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    let is_dsd = matches!(ext.as_deref(), Some("dsf") | Some("dff"));

    // For DSD containers (R7.1) we want a track row in the DB even
    // when lofty cannot read tags: the engine's `dsd_rate()` only
    // needs `(sample_rate, bit_depth=1)` to play the file. Fall
    // back to a stub track with the file stem as title rather than
    // failing the whole scan on a tagless DSF/DFF.
    let tagged = match Probe::open(path)
        .map_err(|e| LibraryError::Tag(e.to_string()))
        .and_then(|p| p.read().map_err(|e| LibraryError::Tag(e.to_string())))
    {
        Ok(t) => Some(t),
        Err(e) => {
            if is_dsd {
                tracing::debug!(
                    target: "qobee::library",
                    error = %e,
                    path = %path.display(),
                    "DSD file: lofty tag read failed; persisting stub track"
                );
                None
            } else {
                return Err(e);
            }
        }
    };

    let properties = tagged.as_ref().map(|t| t.properties().clone());
    let primary_tag = tagged
        .as_ref()
        .and_then(|t| t.primary_tag().or_else(|| t.first_tag()));

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

    let duration_seconds = properties
        .as_ref()
        .map(|p| p.duration().as_secs_f64())
        .unwrap_or(0.0);
    let mut sample_rate = properties.as_ref().and_then(|p| p.sample_rate());
    let mut bit_depth = properties.as_ref().and_then(|p| p.bit_depth());
    let mut channels = properties.as_ref().and_then(|p| p.channels()).map(|c| c as u16);

    // R7.1 — DSD overrides. The DB recognises DSD tracks by the
    // pair `(sample_rate, bit_depth=1)`; a DSD64 file therefore
    // stores `sample_rate = 2_822_400`. Lofty surfaces the bit
    // rate inconsistently across taggers, so we always override
    // these two fields by reading the DSD container header
    // directly. On any parse failure we fall back to
    // `sample_rate = 0` so `Track::dsd_rate()` returns `None`
    // (the file is still indexed, just not flagged as DSD).
    if is_dsd {
        bit_depth = Some(1);
        let parsed = match ext.as_deref() {
            Some("dsf") => read_dsf_header(path).ok(),
            Some("dff") => read_dff_header(path).ok(),
            _ => None,
        };
        match parsed {
            Some((hz, ch)) => {
                sample_rate = Some(hz);
                if ch > 0 {
                    channels = Some(ch);
                }
            }
            None => {
                sample_rate = Some(0);
            }
        }
    }

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

    // RG peak: linear amplitude, full-scale = 1.0 (true-peak values
    // can technically exceed 1.0, e.g. "1.0234"). Some taggers append
    // a stray " dB" suffix even though the value is linear; we strip
    // any trailing token and parse the leading float. Clamp to a
    // generous `[0.0, 4.0]` window — anything outside that range is
    // almost certainly garbage.
    let parse_rg_peak = |s: &str| -> Option<f32> {
        s.split_whitespace()
            .next()
            .and_then(|t| t.parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 4.0))
    };
    let replaygain_track_peak = primary_tag
        .and_then(|t| t.get_string(lofty::tag::ItemKey::ReplayGainTrackPeak))
        .and_then(parse_rg_peak);
    let replaygain_album_peak = primary_tag
        .and_then(|t| t.get_string(lofty::tag::ItemKey::ReplayGainAlbumPeak))
        .and_then(parse_rg_peak);

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
        replaygain_track_peak,
        replaygain_album_peak,
    };

    Ok((track, cover_blob))
}

/// Parse just enough of a DSF header to extract
/// `(sampling_frequency_hz, channel_count)` for the library index.
///
/// DSF layout: `DSD ` (28 bytes) + `fmt ` chunk header (12 bytes) +
/// `fmt ` body (40 bytes; channel_num at offset 12, sampling_frequency
/// at offset 16, both little-endian u32).
fn read_dsf_header(path: &Path) -> std::io::Result<(u32, u16)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = [0u8; 28 + 12 + 40];
    f.read_exact(&mut buf)?;
    if &buf[0..4] != b"DSD " {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing DSD magic",
        ));
    }
    if &buf[28..32] != b"fmt " {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing fmt chunk",
        ));
    }
    // fmt body starts at byte 40 (28 + 12).
    let body = &buf[40..80];
    let channel_num = u32::from_le_bytes(body[12..16].try_into().unwrap());
    let sample_rate = u32::from_le_bytes(body[16..20].try_into().unwrap());
    Ok((sample_rate, channel_num as u16))
}

/// Parse the DFF/DSDIFF outer container far enough to extract
/// `(sampling_frequency_hz, channel_count)` from the
/// `PROP/SND `→`FS  ` and `CHNL` sub-chunks.
///
/// DFF wraps everything in a `FRM8` chunk with a 64-bit size; the
/// `PROP` sub-chunk uses 32-bit sizes for its own children. We
/// walk the inner stream until both fields are found or the
/// reader runs out.
fn read_dff_header(path: &Path) -> std::io::Result<(u32, u16)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;

    // Outer `FRM8` chunk header (4-byte id + 8-byte size + 4-byte form type).
    let mut hdr = [0u8; 16];
    f.read_exact(&mut hdr)?;
    if &hdr[0..4] != b"FRM8" || &hdr[12..16] != b"DSD " {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a DSDIFF/FRM8 container",
        ));
    }

    let mut sample_rate: Option<u32> = None;
    let mut channels: Option<u16> = None;

    loop {
        let mut id = [0u8; 4];
        match f.read_exact(&mut id) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        // FRM8 / DSD / DST carry 64-bit sizes; everything else 32.
        let size: u64 = if &id == b"FRM8" || &id == b"DSD " || &id == b"DST " {
            let mut s = [0u8; 8];
            f.read_exact(&mut s)?;
            u64::from_be_bytes(s)
        } else {
            let mut s = [0u8; 4];
            f.read_exact(&mut s)?;
            u32::from_be_bytes(s) as u64
        };

        if &id == b"PROP" {
            // PROP body: 4-byte form type ("SND ") + sub-chunks.
            let mut prop_type = [0u8; 4];
            f.read_exact(&mut prop_type)?;
            if &prop_type != b"SND " {
                f.seek(SeekFrom::Current((size as i64) - 4))?;
                continue;
            }
            let mut remaining = (size as i64) - 4;
            while remaining > 0 {
                let mut sid = [0u8; 4];
                f.read_exact(&mut sid)?;
                let mut sz_buf = [0u8; 4];
                f.read_exact(&mut sz_buf)?;
                let sz = u32::from_be_bytes(sz_buf);
                match &sid {
                    b"FS  " => {
                        let mut fs_buf = [0u8; 4];
                        f.read_exact(&mut fs_buf)?;
                        sample_rate = Some(u32::from_be_bytes(fs_buf));
                        let pad = (sz as i64) - 4 + ((sz as i64) & 1);
                        if pad > 0 {
                            f.seek(SeekFrom::Current(pad))?;
                        }
                    }
                    b"CHNL" => {
                        let mut nc_buf = [0u8; 2];
                        f.read_exact(&mut nc_buf)?;
                        channels = Some(u16::from_be_bytes(nc_buf));
                        let pad = (sz as i64) - 2 + ((sz as i64) & 1);
                        if pad > 0 {
                            f.seek(SeekFrom::Current(pad))?;
                        }
                    }
                    _ => {
                        let pad = (sz as i64) + ((sz as i64) & 1);
                        f.seek(SeekFrom::Current(pad))?;
                    }
                }
                remaining -= 8 + (sz as i64) + ((sz as i64) & 1);
                if sample_rate.is_some() && channels.is_some() {
                    break;
                }
            }
            if sample_rate.is_some() && channels.is_some() {
                break;
            }
        } else {
            // Skip whatever we don't care about (FVER, DSD, DST,
            // unknown). IFF chunks pad to even size.
            let pad = (size as i64) + ((size as i64) & 1);
            f.seek(SeekFrom::Current(pad))?;
        }
    }

    let sample_rate = sample_rate.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing FS sub-chunk")
    })?;
    let channels = channels.unwrap_or(0);
    Ok((sample_rate, channels))
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
