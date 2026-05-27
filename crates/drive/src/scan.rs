//! Recursive listing of audio files inside a Drive folder.
//!
//! Walks the folder tree breadth-first, yielding [`AudioFile`]
//! entries with the full virtual path (folder1/folder2/track.flac)
//! so the indexer can build a meaningful "library path" string for
//! the existing schema.
//!
//! No async, no parallel: Drive rate-limits aggressively, so we
//! page through one folder at a time.

use crate::api::{DriveClient, DriveFile};
use crate::error::DriveResult;

/// One audio file discovered by the walk.
#[derive(Debug, Clone)]
pub struct AudioFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: Option<u64>,
    pub md5_checksum: Option<String>,
    /// Virtual path inside the user-selected root folder, slash-
    /// separated. The root folder itself is *not* included.
    pub virtual_path: String,
    /// Drive's `modifiedTime` as an RFC3339 string (verbatim from
    /// the API). Translate via [`Self::modified_time_unix`] for a
    /// SQLite-friendly value.
    pub modified_time_rfc3339: Option<String>,
}

impl AudioFile {
    /// Best-effort parse of the RFC3339 timestamp. Returns `None`
    /// when Drive didn't supply a `modifiedTime` or the value is
    /// malformed.
    pub fn modified_time_unix(&self) -> Option<i64> {
        let s = self.modified_time_rfc3339.as_deref()?;
        // Tiny RFC3339 parser. Drive always sends the canonical
        // `YYYY-MM-DDTHH:MM:SS.sssZ` shape, so we don't need
        // chrono just for this. Fall back to None on anything we
        // can't decode.
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .ok()
            .map(|dt| dt.unix_timestamp())
    }

    /// File extension (lowercase, no dot) inferred from the name.
    pub fn extension(&self) -> Option<String> {
        std::path::Path::new(&self.name)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
    }
}

/// List every audio file under `root_id`, recursively. Calls
/// `progress` once per folder visited (not per file) so the UI
/// can show a coarse-grained spinner without flooding events.
pub fn list_audio_files<F>(
    client: &DriveClient,
    root_id: &str,
    mut progress: F,
) -> DriveResult<Vec<AudioFile>>
where
    F: FnMut(&str, usize),
{
    let mut out = Vec::new();
    // BFS: queue of (folder_id, virtual_prefix).
    let mut queue: Vec<(String, String)> = vec![(root_id.to_string(), String::new())];
    let mut visited = 0usize;

    while let Some((folder_id, prefix)) = queue.pop() {
        visited += 1;
        progress(&prefix, visited);

        let children: Vec<DriveFile> = client.list_folder(&folder_id)?;
        for child in children {
            let virtual_path = if prefix.is_empty() {
                child.name.clone()
            } else {
                format!("{prefix}/{}", child.name)
            };
            if child.is_folder() {
                queue.push((child.id, virtual_path));
            } else if child.is_audio() {
                out.push(AudioFile {
                    id: child.id,
                    name: child.name,
                    mime_type: child.mime_type,
                    size: child.size,
                    md5_checksum: child.md5_checksum,
                    virtual_path,
                    modified_time_rfc3339: child.modified_time,
                });
            }
        }
    }
    Ok(out)
}
