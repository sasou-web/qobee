//! Tag indexer for Drive sources.
//!
//! For each audio file we discover via [`crate::scan::list_audio_files`]
//! we download a small head + tail slice and feed those bytes to a
//! caller-supplied tag-extraction function (typically
//! [`qobee_library::index_remote_blob`]).
//!
//! Why head + tail rather than a full download:
//! - FLAC, MP3 (ID3v2), Opus, M4A all carry their primary tags in
//!   the first few hundred kilobytes of the file. Head 384 KB
//!   covers the worst case we've seen in practice (FLAC + huge
//!   embedded cover art).
//! - MP3 sometimes carries an ID3v1 tag in the *last* 128 bytes; a
//!   tiny tail slice is cheap insurance.
//! - Skipping the audio body avoids paying for the entire file
//!   transfer up front, which matters on big libraries.

use std::time::Duration;

use crate::api::DriveClient;
use crate::error::{DriveError, DriveResult};
use crate::scan::AudioFile;

/// Bytes downloaded from the start of each file. Sized to fit the
/// metadata of any FLAC + cover combo we've encountered.
pub const HEAD_BYTES: u64 = 384 * 1024;

/// Bytes downloaded from the end of each file (for ID3v1 et al).
pub const TAIL_BYTES: u64 = 64 * 1024;

/// Indexed item ready to hand off to the library.
pub struct IndexedRemoteFile {
    pub source_id: i64,
    pub file: AudioFile,
    pub blob: Vec<u8>,
    pub mtime: i64,
}

/// Progress event emitted during a Drive index.
#[derive(Debug, Clone)]
pub struct IndexProgress {
    pub files_visited: u64,
    pub files_indexed: u64,
    pub current: Option<String>,
}

/// Walk the source's selected folder, download the metadata blobs
/// for every audio file, and call `on_track` for each one with the
/// concatenated bytes ready for tag extraction. The callback is
/// expected to do the actual lofty parse + DB write.
///
/// `progress` is called every few files so the UI can show a
/// spinner. `should_cancel` is checked between files; returning
/// `true` aborts the index cleanly.
pub fn index_drive_source<P, T, C>(
    client: &DriveClient,
    source_id: i64,
    root_id: &str,
    mut progress: P,
    mut on_track: T,
    mut should_cancel: C,
) -> DriveResult<IndexReport>
where
    P: FnMut(IndexProgress),
    T: FnMut(IndexedRemoteFile) -> Result<(), String>,
    C: FnMut() -> bool,
{
    use crate::scan::list_audio_files;

    // Phase 1: discovery. Cheap per-folder paginated calls.
    let mut report = IndexReport::default();
    let files = list_audio_files(client, root_id, |path, _visited| {
        progress(IndexProgress {
            files_visited: 0,
            files_indexed: 0,
            current: Some(path.to_string()),
        });
    })?;

    let total = files.len() as u64;
    let mut indexed = 0u64;

    // Phase 2: per-file metadata download. Sequential — Drive
    // rate-limits aggressively and we don't gain much from
    // parallel reads on tag-sized blobs.
    for (i, file) in files.into_iter().enumerate() {
        if should_cancel() {
            return Err(DriveError::Cancelled);
        }
        report.files_visited += 1;

        let mtime = file.modified_time_unix().unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

        match download_head_and_tail(client, &file) {
            Ok(blob) => {
                let item = IndexedRemoteFile {
                    source_id,
                    file: file.clone(),
                    blob,
                    mtime,
                };
                match on_track(item) {
                    Ok(()) => {
                        indexed += 1;
                        report.files_indexed += 1;
                    }
                    Err(e) => {
                        report.errors.push(format!("{}: {}", file.virtual_path, e));
                    }
                }
            }
            Err(e) => {
                report.errors.push(format!("{}: {}", file.virtual_path, e));
            }
        }

        if (i + 1) % 4 == 0 || (i as u64 + 1) == total {
            progress(IndexProgress {
                files_visited: report.files_visited,
                files_indexed: indexed,
                current: Some(file.virtual_path.clone()),
            });
        }

        // Tiny politeness pause — Drive's per-user quota is 1000
        // requests / 100s. Indexing 5 files / sec stays safely
        // under it even with the discovery calls layered in.
        std::thread::sleep(Duration::from_millis(40));
    }

    progress(IndexProgress {
        files_visited: report.files_visited,
        files_indexed: report.files_indexed,
        current: None,
    });
    Ok(report)
}

/// Download the head of `file` plus the tail (if the file is large
/// enough). Returns the head bytes followed by the tail bytes,
/// concatenated.
fn download_head_and_tail(client: &DriveClient, file: &AudioFile) -> DriveResult<Vec<u8>> {
    let size = file.size.unwrap_or(0);
    let head_end = HEAD_BYTES.min(size.max(1)).saturating_sub(1);

    let head_bytes = read_range(client, &file.id, 0, head_end)?;

    // Only pull a tail when the file is bigger than head + tail
    // *and* the tail wouldn't overlap the head (we already have
    // the front of the file, no point re-asking for it).
    let mut blob = head_bytes;
    if size > HEAD_BYTES + TAIL_BYTES {
        let tail_start = size - TAIL_BYTES;
        let tail_end = size - 1;
        match read_range(client, &file.id, tail_start, tail_end) {
            Ok(bytes) => blob.extend(bytes),
            Err(e) => {
                tracing::debug!(target: "qobee::drive", error = %e, "tail download failed");
            }
        }
    }
    Ok(blob)
}

fn read_range(client: &DriveClient, file_id: &str, start: u64, end: u64) -> DriveResult<Vec<u8>> {
    use std::io::Read;
    let mut resp = client.download_range(file_id, start, end)?;
    let mut buf = Vec::with_capacity(((end - start) + 1) as usize);
    resp.read_to_end(&mut buf)?;
    Ok(buf)
}

#[derive(Debug, Default, Clone)]
pub struct IndexReport {
    pub files_visited: u64,
    pub files_indexed: u64,
    pub errors: Vec<String>,
}
