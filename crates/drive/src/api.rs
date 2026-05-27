//! Thin Drive REST wrapper.
//!
//! Only the calls Qobee actually needs:
//!
//! * [`DriveClient::list_folder`] — list children of a folder, paginated.
//! * [`DriveClient::get_file`] — fetch one file's metadata.
//! * [`DriveClient::about`] — sanity check the access token + show
//!   the user which account they're connected to.
//! * [`DriveClient::download_range`] — byte-range read used by the
//!   streaming media source in PR3.
//!
//! Tokens are refreshed transparently before every call when the
//! current access token is within 60s of expiry. Token persistence
//! is delegated to the caller via the supplied [`TokenStore`].

use parking_lot::Mutex;
use reqwest::blocking::{Client, Response};
use reqwest::header::{AUTHORIZATION, RANGE};
use serde::Deserialize;
use std::sync::Arc;

use crate::error::{DriveError, DriveResult};
use crate::oauth::refresh_access_token;
use crate::tokens::{StoredTokens, TokenStore};

const USER_AGENT: &str = concat!("qobee/", env!("CARGO_PKG_VERSION"));

/// One file we care about — narrow projection of the Drive
/// `files#resource` schema.
#[derive(Debug, Clone, Deserialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    /// MIME type as reported by Drive (e.g. `audio/flac`).
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    /// File size in bytes, as a string in JSON (Drive's quirk).
    #[serde(default, rename = "size", with = "size_string")]
    pub size: Option<u64>,
    /// MD5 checksum (only present for binary user files; absent
    /// for Google-native types like Docs).
    #[serde(default, rename = "md5Checksum")]
    pub md5_checksum: Option<String>,
    /// Parents the file lives under. Always at most 1 in the new
    /// `My Drive` model.
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default, rename = "modifiedTime")]
    pub modified_time: Option<String>,
}

impl DriveFile {
    pub fn is_folder(&self) -> bool {
        self.mime_type == "application/vnd.google-apps.folder"
    }
    pub fn is_audio(&self) -> bool {
        self.mime_type.starts_with("audio/")
            || matches!(
                self.mime_type.as_str(),
                "application/octet-stream"
                    | "application/x-flac"
                    | "video/x-matroska"
                    | "audio/x-flac"
                    | "audio/x-wav"
            )
    }
}

mod size_string {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(s) => s.parse::<u64>().map(Some).map_err(serde::de::Error::custom),
        }
    }
}

/// One folder — same projection but the consumer doesn't need the
/// audio-only convenience.
#[derive(Debug, Clone)]
pub struct DriveFolder {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct FileListResponse {
    files: Vec<DriveFile>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// Drive client tied to one source. Holds a refresh-aware token
/// cache so consecutive calls don't re-read the keychain.
pub struct DriveClient {
    http: Client,
    tokens: Arc<Mutex<StoredTokens>>,
    store: TokenStore,
}

impl DriveClient {
    /// Build a client. Loads the tokens from the keychain on
    /// construction; returns `Err` if the source was never
    /// authorized or the credentials are gone.
    pub fn new(source_id: i64) -> DriveResult<Self> {
        let store = TokenStore::for_source(source_id)?;
        let tokens = store
            .load()?
            .ok_or_else(|| DriveError::NeedsReauth(format!("source {source_id} not authorized")))?;
        let http = Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self {
            http,
            tokens: Arc::new(Mutex::new(tokens)),
            store,
        })
    }

    fn ensure_fresh(&self) -> DriveResult<String> {
        let mut tokens = self.tokens.lock();
        if tokens.is_expired(unix_now()) {
            refresh_access_token(&mut tokens)?;
            self.store.save(&tokens)?;
        }
        Ok(tokens.access_token.clone())
    }

    fn authed_get(&self, url: &str) -> DriveResult<Response> {
        let token = self.ensure_fresh()?;
        Ok(self
            .http
            .get(url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send()?)
    }

    /// `about?fields=user(displayName,emailAddress)`. Used by the
    /// wizard to confirm the authorized account.
    pub fn about(&self) -> DriveResult<AboutInfo> {
        let url = format!(
            "{base}/about?fields=user(displayName,emailAddress,photoLink)",
            base = crate::DRIVE_API_BASE
        );
        let resp = self.authed_get(&url)?;
        if !resp.status().is_success() {
            return Err(DriveError::Api {
                status: resp.status().as_u16(),
                body: resp.text().unwrap_or_default(),
            });
        }
        Ok(resp.json()?)
    }

    /// One page of children inside `folder_id`. Use
    /// [`Self::list_folder_all`] when you want the complete list.
    pub fn list_folder_page(
        &self,
        folder_id: &str,
        page_token: Option<&str>,
    ) -> DriveResult<(Vec<DriveFile>, Option<String>)> {
        let mut url = format!(
            "{base}/files?q='{fid}'+in+parents+and+trashed=false\
             &fields=nextPageToken,files(id,name,mimeType,size,md5Checksum,parents,modifiedTime)\
             &pageSize=200",
            base = crate::DRIVE_API_BASE,
            fid = urlencoding_encode(folder_id),
        );
        if let Some(token) = page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding_encode(token));
        }
        let resp = self.authed_get(&url)?;
        if !resp.status().is_success() {
            return Err(DriveError::Api {
                status: resp.status().as_u16(),
                body: resp.text().unwrap_or_default(),
            });
        }
        let parsed: FileListResponse = resp.json()?;
        Ok((parsed.files, parsed.next_page_token))
    }

    /// All children of `folder_id`. Pages through the cursor.
    pub fn list_folder(&self, folder_id: &str) -> DriveResult<Vec<DriveFile>> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) = self.list_folder_page(folder_id, token.as_deref())?;
            all.extend(page);
            match next {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        Ok(all)
    }

    /// Stream a `[start, end]` byte range from a file. The caller
    /// is responsible for reading the body. Returns the response
    /// directly so callers can plug it into `Read` or stream it.
    pub fn download_range(&self, file_id: &str, start: u64, end: u64) -> DriveResult<Response> {
        let url = format!(
            "{base}/{id}?alt=media",
            base = crate::DRIVE_DOWNLOAD_BASE,
            id = urlencoding_encode(file_id),
        );
        let token = self.ensure_fresh()?;
        let resp = self
            .http
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(RANGE, format!("bytes={start}-{end}"))
            .send()?;
        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            return Err(DriveError::Api {
                status: resp.status().as_u16(),
                body: resp.text().unwrap_or_default(),
            });
        }
        Ok(resp)
    }

    /// Download a file in full. Used by the sync layer to fetch
    /// the small JSON state blob written into the source folder.
    pub fn download_full(&self, file_id: &str) -> DriveResult<Vec<u8>> {
        use std::io::Read;
        let mut resp = self.download_range(file_id, 0, u64::MAX / 2)?;
        let mut body = Vec::new();
        resp.read_to_end(&mut body)?;
        Ok(body)
    }

    /// Find a single child of `folder_id` whose name matches
    /// exactly `name`. Returns `Ok(None)` when nothing matches —
    /// the caller can decide whether to create the file.
    pub fn find_in_folder_by_name(
        &self,
        folder_id: &str,
        name: &str,
    ) -> DriveResult<Option<DriveFile>> {
        // Drive's `q` syntax uses single quotes around literals;
        // an apostrophe in the file name has to be doubled.
        let escaped_name = name.replace('\'', "\\'");
        let url = format!(
            "{base}/files?q=name='{name}'+and+'{fid}'+in+parents+and+trashed=false\
             &fields=files(id,name,mimeType,size,md5Checksum,parents,modifiedTime)\
             &pageSize=1",
            base = crate::DRIVE_API_BASE,
            name = urlencoding_encode(&escaped_name),
            fid = urlencoding_encode(folder_id),
        );
        let resp = self.authed_get(&url)?;
        if !resp.status().is_success() {
            return Err(DriveError::Api {
                status: resp.status().as_u16(),
                body: resp.text().unwrap_or_default(),
            });
        }
        let parsed: FileListResponse = resp.json()?;
        Ok(parsed.files.into_iter().next())
    }

    /// Create a new JSON file inside `folder_id` with the given
    /// `name` and `content` (UTF-8 JSON). Returns the new file id.
    pub fn create_text_file(
        &self,
        folder_id: &str,
        name: &str,
        content: &str,
    ) -> DriveResult<String> {
        // Multipart upload via `uploads` endpoint. Two parts: a
        // metadata JSON (name + parents + mimeType) and the body.
        let token = self.ensure_fresh()?;
        let metadata = serde_json::json!({
            "name": name,
            "parents": [folder_id],
            "mimeType": "application/json",
        });

        let boundary = "qobee_boundary_42";
        let body = format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n\
             --{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{content}\r\n\
             --{boundary}--",
            boundary = boundary,
            meta = metadata,
            content = content,
        );

        let resp = self
            .http
            .post("https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/related; boundary={boundary}"),
            )
            .body(body)
            .send()?;
        if !resp.status().is_success() {
            return Err(DriveError::Api {
                status: resp.status().as_u16(),
                body: resp.text().unwrap_or_default(),
            });
        }
        #[derive(serde::Deserialize)]
        struct CreateResp {
            id: String,
        }
        let parsed: CreateResp = resp.json()?;
        Ok(parsed.id)
    }

    /// Overwrite the content of an existing file. The Drive
    /// metadata (name, parents) is left untouched.
    pub fn update_file_content(&self, file_id: &str, content: &str) -> DriveResult<()> {
        let token = self.ensure_fresh()?;
        let url = format!(
            "https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=media",
            id = urlencoding_encode(file_id),
        );
        let resp = self
            .http
            .patch(&url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(content.to_string())
            .send()?;
        if !resp.status().is_success() {
            return Err(DriveError::Api {
                status: resp.status().as_u16(),
                body: resp.text().unwrap_or_default(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AboutInfo {
    pub user: AboutUser,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AboutUser {
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "emailAddress")]
    pub email_address: Option<String>,
    #[serde(default, rename = "photoLink")]
    pub photo_link: Option<String>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Tiny percent-encoder: only needed for query-string components
/// of file IDs and page tokens, which Drive returns as opaque
/// ASCII tokens.
fn urlencoding_encode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let unreserved =
            matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~');
        if unreserved {
            out.push(b as char);
        } else {
            write!(out, "%{b:02X}").unwrap();
        }
    }
    out
}
