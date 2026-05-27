//! Symphonia [`MediaSource`] backed by HTTP byte-range reads against
//! Google Drive.
//!
//! Why this exists:
//!
//! Symphonia decoders read audio from anything that implements
//! `Read + Seek`. Local files are trivial; remote files need us to
//! turn a `seek + read` workload into the HTTP `Range:` requests
//! Drive understands. The trick is doing it without re-fetching
//! the same bytes every packet — Symphonia issues lots of small
//! reads, especially around the file headers.
//!
//! Strategy:
//!
//!   * The source carries a fixed-size LRU **page cache** (default
//!     8 × 512 KB = 4 MB). Reads inside a cached page are
//!     copies; misses fault the page in via one `Range:` request.
//!   * We always know the total file size (Symphonia asks for it
//!     up front via `byte_len()`). The size comes from the Drive
//!     `files.get` metadata call before the source is built.
//!   * `Send + Sync` is required: we wrap the [`DriveClient`] in
//!     an `Arc` and the cache in a `Mutex`. The decoder thread is
//!     the only consumer in practice, but Symphonia models the
//!     source as `Sync`.
//!
//! Limitations:
//!
//!   * No background prefetch yet. The caller can warm-pull the
//!     first page synchronously before handing the source off to
//!     Symphonia, which sidesteps the worst of the start-up
//!     latency for FLAC headers.
//!   * Mid-track seeks past the cache window result in one round
//!     trip. Acceptable for a music app where seeks are rare.

use std::io::{Read, Result as IoResult, Seek, SeekFrom};
use std::sync::Arc;

use parking_lot::Mutex;
use symphonia::core::io::MediaSource;

use crate::api::DriveClient;
use crate::error::{DriveError, DriveResult};

/// Page size used by the cache. 512 KB is large enough that one
/// page covers a typical FLAC frame yet small enough that a seek
/// to a fresh region pays for at most ~250 ms of network at
/// home-broadband speeds.
pub const PAGE_BYTES: u64 = 512 * 1024;

/// Maximum number of pages kept in memory. 8 × 512 KB = 4 MB per
/// open track. Adjustable; the small fixed budget is what makes
/// the cache an LRU rather than an unbounded buffer.
pub const MAX_PAGES: usize = 8;

/// Cached page: `start` is the absolute offset of the first byte.
struct Page {
    start: u64,
    bytes: Vec<u8>,
}

struct CacheState {
    pages: Vec<Page>,
}

impl CacheState {
    fn new() -> Self {
        Self {
            pages: Vec::with_capacity(MAX_PAGES),
        }
    }

    /// Look up the page that covers `offset` if any, returning a
    /// reference to its bytes plus the local index of `offset`
    /// inside the page.
    fn get(&self, offset: u64) -> Option<(usize, &[u8])> {
        for (i, page) in self.pages.iter().enumerate() {
            let end = page.start + page.bytes.len() as u64;
            if offset >= page.start && offset < end {
                let local = (offset - page.start) as usize;
                return Some((i, &page.bytes[local..]));
            }
        }
        None
    }

    /// Bump the page at `idx` to the front of the LRU.
    fn touch(&mut self, idx: usize) {
        if idx > 0 {
            let page = self.pages.remove(idx);
            self.pages.insert(0, page);
        }
    }

    /// Insert a new page at the front, evicting the oldest when
    /// the budget is exceeded.
    fn insert(&mut self, page: Page) {
        self.pages.insert(0, page);
        if self.pages.len() > MAX_PAGES {
            self.pages.pop();
        }
    }
}

/// Symphonia-friendly source for one Drive file.
pub struct DriveMediaSource {
    client: Arc<DriveClient>,
    file_id: String,
    total_len: u64,
    pos: Mutex<u64>,
    cache: Mutex<CacheState>,
}

impl DriveMediaSource {
    pub fn new(client: Arc<DriveClient>, file_id: String, total_len: u64) -> Self {
        Self {
            client,
            file_id,
            total_len,
            pos: Mutex::new(0),
            cache: Mutex::new(CacheState::new()),
        }
    }

    /// Probe the file size up front. The caller can use this to
    /// build the source without a separate Drive API call when
    /// the size is already known (e.g. cached on the track row).
    pub fn open(client: Arc<DriveClient>, file_id: &str) -> DriveResult<Self> {
        // Fetch one byte to learn the total length via
        // Content-Range. This is cheap (one round trip) and avoids
        // adding a second Drive endpoint.
        let resp = client.download_range(file_id, 0, 0)?;
        let total_len = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| {
                DriveError::Internal("Drive did not return a Content-Range total".into())
            })?;
        Ok(Self::new(client, file_id.to_string(), total_len))
    }

    fn fetch_page(&self, page_start: u64) -> IoResult<()> {
        let end = (page_start + PAGE_BYTES)
            .min(self.total_len)
            .saturating_sub(1);
        let mut resp = self
            .client
            .download_range(&self.file_id, page_start, end)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let mut bytes = Vec::with_capacity(((end - page_start) + 1) as usize);
        resp.read_to_end(&mut bytes)?;
        self.cache.lock().insert(Page {
            start: page_start,
            bytes,
        });
        Ok(())
    }

    fn page_start_for(offset: u64) -> u64 {
        (offset / PAGE_BYTES) * PAGE_BYTES
    }
}

impl Read for DriveMediaSource {
    fn read(&mut self, dst: &mut [u8]) -> IoResult<usize> {
        let mut pos_lock = self.pos.lock();
        let pos = *pos_lock;
        if pos >= self.total_len {
            return Ok(0);
        }
        // Try the cache first. If the page that covers `pos`
        // is present we serve straight out of memory.
        {
            let mut cache = self.cache.lock();
            if let Some((idx, slice)) = cache.get(pos) {
                let n = slice.len().min(dst.len());
                dst[..n].copy_from_slice(&slice[..n]);
                cache.touch(idx);
                *pos_lock = pos + n as u64;
                return Ok(n);
            }
        }
        // Miss. Fault the page covering `pos` and retry.
        let page_start = Self::page_start_for(pos);
        drop(pos_lock); // release while doing the network call
        self.fetch_page(page_start)?;
        // Retry — the page is now present.
        let mut pos_lock = self.pos.lock();
        let pos = *pos_lock;
        let cache = self.cache.lock();
        let (_, slice) = cache.get(pos).expect("page just fetched");
        let n = slice.len().min(dst.len());
        dst[..n].copy_from_slice(&slice[..n]);
        *pos_lock = pos + n as u64;
        Ok(n)
    }
}

impl Seek for DriveMediaSource {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::End(n) => {
                if n >= 0 {
                    self.total_len.saturating_add(n as u64)
                } else {
                    self.total_len.saturating_sub(n.unsigned_abs())
                }
            }
            SeekFrom::Current(n) => {
                let cur = *self.pos.lock();
                if n >= 0 {
                    cur.saturating_add(n as u64)
                } else {
                    cur.saturating_sub(n.unsigned_abs())
                }
            }
        };
        *self.pos.lock() = new_pos;
        Ok(new_pos)
    }
}

impl MediaSource for DriveMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.total_len)
    }
}
