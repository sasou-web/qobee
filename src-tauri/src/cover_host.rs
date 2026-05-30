//! Cover hosting for Discord Rich Presence.
//!
//! Discord renders `large_image` either from a registered asset key
//! or from an HTTPS URL it can reach **server-side** through its
//! image proxy. The `qobee-cover://` custom protocol and absolute
//! file paths are unreachable; the only way to actually display the
//! album art the user sees in Qobee is to make those bytes available
//! over public HTTPS.
//!
//! Strategy:
//!
//! 1. The cover lives at
//!    `<library>/covers/<aa>/<hash>.<ext>` (sharded blake3 hash).
//! 2. We hash the file once. If a settings entry
//!    `discord.cover_url::<hash>` exists, we already uploaded it →
//!    return the cached URL.
//! 3. Otherwise, upload the bytes to litterbox.catbox.moe (anonymous,
//!    free, no key) and persist the resulting URL.
//!
//! The whole pipeline runs on a small dedicated worker thread so the
//! Discord IPC worker is never blocked on a network upload, and the
//! UI is never frozen waiting for one. Failures fall back silently:
//! the worker keeps trying on subsequent track changes, and Discord
//! falls back to the default `qobee` asset key in the meantime.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use qobee_library::Library;

/// litterbox.catbox.moe doesn't require an API key and accepts files
/// up to 1 GB; we pick the longest retention window (3 days). For
/// covers that's well above what we ever need (the worker only
/// uploads each unique image once).
const LITTERBOX_URL: &str = "https://litterbox.catbox.moe/resources/internals/api.php";

/// Maximum bytes we'll consider hosting. Album covers are a few
/// hundred KB at most; anything bigger is almost certainly something
/// we don't want to push to a third party (and would slow Discord's
/// proxy).
const MAX_COVER_BYTES: u64 = 4 * 1024 * 1024;

/// Settings key namespace used to memoize uploaded URLs. The key is
/// the cover_key cache key, the value is the hosted https URL.
fn settings_key(cover_key: &str) -> String {
    format!("discord.cover_url::{cover_key}")
}

/// Result of a cover upload, sent back asynchronously to the
/// [`crate::discord::DiscordPresence`] worker.
#[derive(Debug, Clone)]
pub struct UploadedCover {
    /// The cover_key (e.g. `aa/abcd…1234.jpg`) the URL belongs to.
    pub cover_key: String,
    /// Public https URL Discord can fetch.
    pub url: String,
}

#[derive(Debug)]
struct UploadJob {
    cover_key: String,
}

/// Cheap-to-clone handle to the cover host worker.
#[derive(Clone)]
pub struct CoverHost {
    inner: Arc<Inner>,
}

struct Inner {
    tx: Sender<UploadJob>,
    /// In-process cache mirroring the persisted settings table, so
    /// repeated lookups for the same track avoid SQLite hits.
    cache: Mutex<HashMap<String, String>>,
    /// Library handle used for the persistent URL cache (settings
    /// table) and the cover cache directory.
    library: Library,
    /// Master switch: when `false`, [`CoverHost::request_upload`] is a
    /// no-op and the worker never sends bytes to a third party.
    /// Defaults to `false` for privacy: the user must opt in via
    /// Settings.
    enabled: Mutex<bool>,
}

impl CoverHost {
    /// Spawn the upload worker. Listeners receive [`UploadedCover`]
    /// notifications via the provided callback.
    pub fn new<F>(library: Library, on_uploaded: F) -> Self
    where
        F: Fn(UploadedCover) + Send + Sync + 'static,
    {
        // Bounded so a stuck network never balloons memory.
        let (tx, rx) = bounded::<UploadJob>(64);
        let cache: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let cache_for_worker = cache.clone();
        let library_for_worker = library.clone();

        thread::Builder::new()
            .name("qobee-cover-host".into())
            .spawn(move || worker_main(rx, library_for_worker, cache_for_worker, on_uploaded))
            .expect("failed to spawn cover host worker");

        Self {
            inner: Arc::new(Inner {
                tx,
                cache: Mutex::new(HashMap::new()),
                library,
                enabled: Mutex::new(false),
            }),
        }
    }

    /// Toggle whether [`Self::request_upload`] actually contacts the
    /// public host. Cached / already-uploaded URLs returned by
    /// [`Self::get_cached_via_settings`] keep working either way —
    /// turning the switch off does not delete already-shared URLs.
    pub fn set_enabled(&self, on: bool) {
        *self.inner.enabled.lock() = on;
    }

    pub fn is_enabled(&self) -> bool {
        *self.inner.enabled.lock()
    }

    /// Synchronous lookup against the in-memory cache and the
    /// persisted settings table. Returns the URL if we already
    /// uploaded the matching cover.
    pub fn get_cached_via_settings(&self, cover_key: &str) -> Option<String> {
        if let Some(u) = self.inner.cache.lock().get(cover_key).cloned() {
            return Some(u);
        }
        let key = settings_key(cover_key);
        match self.inner.library.get_setting(&key) {
            Ok(Some(u)) if u.starts_with("https://") => {
                self.inner
                    .cache
                    .lock()
                    .insert(cover_key.to_string(), u.clone());
                Some(u)
            }
            _ => None,
        }
    }

    /// Drop the in-memory mirror of the persisted URL cache. Called
    /// after the user explicitly purges the uploaded cover links
    /// (R8.4) so a URL removed from the `settings` table is not
    /// re-served from memory for the rest of the session. The
    /// persisted rows are deleted separately by the command via
    /// `clear_settings_by_prefix`.
    pub fn clear_link_cache(&self) {
        self.inner.cache.lock().clear();
    }

    /// Enqueue an async upload. Idempotent: dropping the job when the
    /// queue is full is fine — the next track change will retry.
    /// Refuses silently when uploads are disabled (RGPD-friendly
    /// default).
    pub fn request_upload(&self, cover_key: &str) {
        if !self.is_enabled() {
            return;
        }
        let _ = self.inner.tx.try_send(UploadJob {
            cover_key: cover_key.to_string(),
        });
    }
}

/// Persist a freshly uploaded URL both in memory and in the library
/// settings table. Failures here are non-fatal: at worst we'll
/// re-upload on the next session.
fn remember(library: &Library, cache: &Mutex<HashMap<String, String>>, cover_key: &str, url: &str) {
    cache.lock().insert(cover_key.to_string(), url.to_string());
    let _ = library.set_setting(&settings_key(cover_key), url);
}

fn worker_main<F>(
    rx: Receiver<UploadJob>,
    library: Library,
    cache: Arc<Mutex<HashMap<String, String>>>,
    on_uploaded: F,
) where
    F: Fn(UploadedCover) + Send + Sync + 'static,
{
    let cache_dir = library.cover_cache_dir().to_path_buf();

    // One blocking client for the whole worker. rustls keeps us
    // OpenSSL-free on Windows. Modest connect/timeout so a stuck
    // host never wedges the queue.
    let client = match reqwest::blocking::Client::builder()
        .user_agent("Qobee/0.1 (+https://github.com/qobee)")
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(target: "qobee::cover_host", error = %e, "could not build HTTP client; covers will not be uploaded");
            return;
        }
    };

    while let Ok(job) = rx.recv() {
        // Re-check cache: a previous job for the same key may have
        // already finished while this one was waiting in the queue.
        if cache.lock().contains_key(&job.cover_key) {
            continue;
        }
        if let Ok(Some(url)) = library.get_setting(&settings_key(&job.cover_key)) {
            if url.starts_with("https://") {
                cache.lock().insert(job.cover_key.clone(), url.clone());
                on_uploaded(UploadedCover {
                    cover_key: job.cover_key,
                    url,
                });
                continue;
            }
        }

        let path = resolve_cover_path(&cache_dir, &job.cover_key);
        let bytes = match read_cover(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(
                    target: "qobee::cover_host",
                    cover_key = %job.cover_key,
                    error = %e,
                    "could not read cover from disk; skipping upload"
                );
                continue;
            }
        };

        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("cover.jpg")
            .to_string();

        match upload_to_litterbox(&client, filename, bytes) {
            Ok(url) => {
                tracing::info!(
                    target: "qobee::cover_host",
                    cover_key = %job.cover_key,
                    url = %url,
                    "uploaded cover for Discord"
                );
                remember(&library, &cache, &job.cover_key, &url);
                on_uploaded(UploadedCover {
                    cover_key: job.cover_key,
                    url,
                });
            }
            Err(e) => {
                tracing::debug!(
                    target: "qobee::cover_host",
                    cover_key = %job.cover_key,
                    error = %e,
                    "cover upload failed; will retry next time"
                );
                // Brief pause so a flapping host doesn't burn the
                // queue. The next track change re-enqueues.
                thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

/// Cover keys are stored as `aa/<hash>.<ext>` with forward slashes;
/// the cache dir uses native separators.
fn resolve_cover_path(cache_dir: &std::path::Path, cover_key: &str) -> PathBuf {
    let normalized = cover_key.replace('/', std::path::MAIN_SEPARATOR_STR);
    cache_dir.join(normalized)
}

fn read_cover(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_COVER_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cover too large: {} bytes", metadata.len()),
        ));
    }
    std::fs::read(path)
}

/// Upload `bytes` to litterbox.catbox.moe and return the resulting
/// `https://files.catbox.moe/...` URL on success. Failures are
/// reported as a generic boxed error; the worker logs the message
/// and retries later.
fn upload_to_litterbox(
    client: &reqwest::blocking::Client,
    filename: String,
    bytes: Vec<u8>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mime = match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    };

    let part = reqwest::blocking::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)?;

    let form = reqwest::blocking::multipart::Form::new()
        .text("reqtype", "fileupload")
        // 72h is the longest retention window litterbox offers; the
        // URL itself is cached forever in our settings, so the only
        // cost of expiry is one re-upload per cover, much later.
        .text("time", "72h")
        .part("fileToUpload", part);

    let resp = client.post(LITTERBOX_URL).multipart(form).send()?;
    if !resp.status().is_success() {
        return Err(format!("litterbox returned status {}", resp.status()).into());
    }
    let body = resp.text()?.trim().to_string();
    if !body.starts_with("https://") {
        return Err(format!("unexpected litterbox response: {body}").into());
    }
    Ok(body)
}
