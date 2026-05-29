//! Lyrics extraction for a track.
//!
//! Tries, in order, until non-empty lyrics are found:
//!   1. A sidecar `.lrc` file next to the audio file (synced lyrics).
//!   2. The `Lyrics` tag embedded in the file's metadata (lofty).
//!   3. An on-disk cache populated by previous online fetches.
//!   4. The LRCLib public API (synced + plain, no key required).
//!   5. A best-effort scrape of the Genius lyrics page (plain only).
//!
//! Synced lyrics are parsed from LRC `[mm:ss.xx]` line prefixes and
//! returned as a sorted `Vec<{ ms, text }>`. Unsynced lyrics are
//! returned as a single string. Both can be present (synced takes
//! precedence in the UI).
//!
//! Online fetches are short-circuited on any IO/network error and
//! silently fall through to the next provider — the lyrics view is
//! best-effort and never blocks playback. Successful fetches are
//! cached to `<data_dir>/lyrics/<sha1>.json` so a second open of
//! the same track is instantaneous and offline-friendly.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LyricLine {
    /// Timestamp in milliseconds from the start of the track.
    pub ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lyrics {
    /// Plain unsynchronized lyrics (whole text), when available.
    pub unsynced: Option<String>,
    /// Synchronized lyrics parsed from an LRC sidecar.
    pub synced: Option<Vec<LyricLine>>,
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.unsynced.is_none() && self.synced.is_none()
    }
}

/// Read lyrics for the audio file at `audio_path`. Local-only:
/// sidecar LRC file, then the embedded tag. Use
/// [`read_for_track`] to also try the on-disk cache and the
/// network providers.
pub fn read_for(audio_path: &Path) -> Lyrics {
    let mut out = Lyrics::default();

    // 1. Sidecar LRC file (synced).
    if let Some(synced) = read_sidecar_lrc(audio_path) {
        if !synced.is_empty() {
            out.synced = Some(synced);
        }
    }

    // 2. Embedded lyrics tag (usually unsynced; sometimes contains
    //    LRC markers — we try parsing as LRC first and fall back to
    //    treating it as plain text).
    if let Some(text) = read_embedded_lyrics(audio_path) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            // Try LRC parse if the text looks like it has timestamps.
            if out.synced.is_none() && looks_like_lrc(trimmed) {
                let parsed = parse_lrc(trimmed);
                if !parsed.is_empty() {
                    out.synced = Some(parsed);
                }
            }
            // Always keep the plain text version too as a fallback.
            out.unsynced = Some(strip_lrc_brackets(trimmed));
        }
    }

    out
}

fn read_sidecar_lrc(audio_path: &Path) -> Option<Vec<LyricLine>> {
    let lrc_path = audio_path.with_extension("lrc");
    let content = fs::read_to_string(&lrc_path).ok()?;
    let parsed = parse_lrc(&content);
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn read_embedded_lyrics(audio_path: &Path) -> Option<String> {
    let tagged = Probe::open(audio_path).ok()?.read().ok()?;
    let primary = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    primary
        .get_string(lofty::tag::ItemKey::Lyrics)
        .map(|s| s.to_string())
}

fn looks_like_lrc(text: &str) -> bool {
    // A reasonable LRC heuristic: at least one line starts with
    // `[<digits>:`.
    text.lines().take(20).any(|line| {
        let t = line.trim_start();
        t.starts_with('[') && t.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
    })
}

/// Parse an LRC body. Recognizes `[mm:ss]`, `[mm:ss.x]`, `[mm:ss.xx]`,
/// `[mm:ss.xxx]`. Skips meta lines like `[ar:...]` `[ti:...]`
/// `[length:...]`. Multiple timestamps on a single line are
/// supported (each produces a `LyricLine` for the same text).
fn parse_lrc(body: &str) -> Vec<LyricLine> {
    let mut out: Vec<LyricLine> = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Collect all timestamps at the start of the line.
        let mut rest = line;
        let mut timestamps: Vec<u64> = Vec::new();
        loop {
            rest = rest.trim_start();
            if !rest.starts_with('[') {
                break;
            }
            let end = match rest.find(']') {
                Some(i) => i,
                None => break,
            };
            let content = &rest[1..end];
            // Try to parse as timestamp; otherwise it's a meta
            // tag and we skip the whole line.
            match parse_timestamp(content) {
                Some(ms) => {
                    timestamps.push(ms);
                    rest = &rest[end + 1..];
                }
                None => {
                    // Meta line. Drop it.
                    timestamps.clear();
                    break;
                }
            }
        }

        if timestamps.is_empty() {
            continue;
        }

        let text = rest.trim().to_string();
        for ms in timestamps {
            out.push(LyricLine {
                ms,
                text: text.clone(),
            });
        }
    }

    out.sort_by_key(|l| l.ms);
    out
}

/// Parse `mm:ss[.xx]` into milliseconds. Returns `None` when the
/// content doesn't look like a timestamp.
fn parse_timestamp(content: &str) -> Option<u64> {
    // `mm:ss.xx`  (two digits of fractional seconds)
    // `mm:ss.xxx` (three digits, milliseconds)
    let (mins_str, rest) = content.split_once(':')?;

    let mins: u64 = mins_str.parse().ok()?;

    let (secs_str, frac_str) = match rest.find('.') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };

    let secs: u64 = secs_str.parse().ok()?;
    let frac_ms: u64 = if frac_str.is_empty() {
        0
    } else {
        // Pad / truncate to 3 digits for milliseconds.
        let mut s = frac_str.to_string();
        while s.len() < 3 {
            s.push('0');
        }
        s.truncate(3);
        s.parse().ok()?
    };

    Some(mins * 60_000 + secs * 1_000 + frac_ms)
}

/// Drop leading `[...]` segments from each line so the unsynced view
/// shows just the lyric text.
fn strip_lrc_brackets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut rest = line;
        loop {
            let trimmed = rest.trim_start();
            if !trimmed.starts_with('[') {
                rest = trimmed;
                break;
            }
            match trimmed.find(']') {
                Some(i) => rest = &trimmed[i + 1..],
                None => break,
            }
        }
        out.push_str(rest.trim());
        out.push('\n');
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Online lyrics resolution (LRCLib priority, Genius fallback) + cache.
// ---------------------------------------------------------------------------

/// Lookup payload — what we know about the track on the local side.
/// Used by [`read_for_track`] to query online providers when no
/// local lyrics could be found.
#[derive(Debug, Clone)]
pub struct TrackLookup<'a> {
    pub audio_path: &'a Path,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration_seconds: f64,
}

/// Resolve lyrics for a track, going through every available
/// source: local sidecar / embedded tag, then the persistent cache,
/// then LRCLib, then Genius.
///
/// `cache_dir` is typically `<data_dir>/lyrics`. The directory is
/// created on demand. Failures along the way (network outages,
/// missing data dir, malformed responses) are swallowed — the worst
/// case is an empty `Lyrics` struct, which the UI already handles.
pub fn read_for_track(lookup: TrackLookup<'_>, cache_dir: Option<&Path>) -> Lyrics {
    // 1+2. Local first. Sidecar / embedded lyrics always win — they
    //      are usually hand-curated and the user might have
    //      authored them on purpose.
    let local = read_for(lookup.audio_path);
    if !local.is_empty() {
        return local;
    }

    // 3. Persistent cache.
    if let Some(dir) = cache_dir {
        if let Some(cached) = read_cache(dir, lookup.artist, lookup.title) {
            if !cached.is_empty() {
                return cached;
            }
        }
    }

    // 4. LRCLib (public, no key, supports synced).
    let lrclib = fetch_lrclib(
        lookup.title,
        lookup.artist,
        lookup.album,
        lookup.duration_seconds,
    );
    if let Some(found) = lrclib {
        if !found.is_empty() {
            if let Some(dir) = cache_dir {
                let _ = write_cache(dir, lookup.artist, lookup.title, &found);
            }
            return found;
        }
    }

    // 5. Genius fallback (plain only, scraped page).
    if let Some(text) = fetch_genius(lookup.title, lookup.artist) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let out = Lyrics {
                unsynced: Some(trimmed.to_string()),
                synced: None,
            };
            if let Some(dir) = cache_dir {
                let _ = write_cache(dir, lookup.artist, lookup.title, &out);
            }
            return out;
        }
    }

    Lyrics::default()
}

// --- Cache --------------------------------------------------------------

fn cache_path(dir: &Path, artist: &str, title: &str) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    normalize_for_match(artist).hash(&mut h);
    normalize_for_match(title).hash(&mut h);
    let key = format!("{:016x}", h.finish());
    dir.join(format!("{key}.json"))
}

fn read_cache(dir: &Path, artist: &str, title: &str) -> Option<Lyrics> {
    let path = cache_path(dir, artist, title);
    let raw = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(dir: &Path, artist: &str, title: &str, lyrics: &Lyrics) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = cache_path(dir, artist, title);
    let body = serde_json::to_string(lyrics)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(path, body)
}

/// Lower-case + strip diacritics so "Jérôme" and "Jerome" cache to
/// the same bucket.
fn normalize_for_match(s: &str) -> String {
    s.trim().to_lowercase()
}

// --- LRCLib provider ----------------------------------------------------

/// Hit `https://lrclib.net/api/get`. The endpoint returns 404 when
/// the track is unknown; both `syncedLyrics` and `plainLyrics` may
/// be present in the JSON, and we honour LRCLib's recommendation to
/// pass `duration` (their matching is duration-aware).
fn fetch_lrclib(title: &str, artist: &str, album: &str, duration_s: f64) -> Option<Lyrics> {
    #[derive(Deserialize)]
    struct LrcLibResponse {
        #[serde(rename = "syncedLyrics", default)]
        synced_lyrics: Option<String>,
        #[serde(rename = "plainLyrics", default)]
        plain_lyrics: Option<String>,
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent("Qobee/0.5 (https://github.com/sasou-web/qobee)")
        .build()
        .ok()?;

    let mut req = client
        .get("https://lrclib.net/api/get")
        .query(&[("track_name", title), ("artist_name", artist)]);
    if !album.is_empty() {
        req = req.query(&[("album_name", album)]);
    }
    if duration_s > 0.0 {
        req = req.query(&[("duration", (duration_s.round() as i64).to_string())]);
    }

    let resp = req.send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let parsed: LrcLibResponse = resp.json().ok()?;

    let mut out = Lyrics::default();
    if let Some(synced_text) = parsed.synced_lyrics.as_deref() {
        let trimmed = synced_text.trim();
        if !trimmed.is_empty() {
            let parsed_lines = parse_lrc(trimmed);
            if !parsed_lines.is_empty() {
                out.synced = Some(parsed_lines);
            }
        }
    }
    if let Some(plain) = parsed.plain_lyrics.as_deref() {
        let trimmed = plain.trim();
        if !trimmed.is_empty() {
            out.unsynced = Some(trimmed.to_string());
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// --- Genius fallback (page scrape) --------------------------------------

/// Best-effort scrape of `https://genius.com/<artist>-<title>-lyrics`.
/// Genius does not require a key for the public lyrics page, but the
/// HTML layout is owned by them and may break at any time. We pull
/// every node tagged `data-lyrics-container="true"`, strip its HTML
/// and concatenate.
///
/// Returns `None` on network or parse failure. Synced lyrics are
/// not exposed by Genius.
fn fetch_genius(title: &str, artist: &str) -> Option<String> {
    let slug = genius_slug(artist, title);
    let url = format!("https://genius.com/{slug}-lyrics");

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Qobee/0.5 \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0",
        )
        .build()
        .ok()?;

    let html = client.get(&url).send().ok()?;
    if !html.status().is_success() {
        return None;
    }
    let body = html.text().ok()?;

    let extracted = extract_genius_lyrics(&body)?;
    let trimmed = extracted.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Build a Genius-style URL slug from "Artist Name" and "Track Title".
/// Genius normalises to lowercase, strips most punctuation and
/// joins with hyphens.
fn genius_slug(artist: &str, title: &str) -> String {
    let mut s = String::with_capacity(artist.len() + title.len() + 1);
    s.push_str(&slug_segment(artist));
    if !s.is_empty() {
        s.push('-');
    }
    s.push_str(&slug_segment(title));
    s
}

fn slug_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_dash = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch.is_whitespace() || matches!(ch, '-' | '_' | '.' | '/' | '\\' | '&'))
            && !prev_dash
        {
            out.push('-');
            prev_dash = true;
        }
        // Diacritics + other punctuation are dropped silently. The
        // mapping isn't perfect (e.g. "Beyoncé" → "beyonc"), which
        // is exactly what Genius does for the URL path itself, so
        // we still hit the right page in most cases.
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Pull every `<div data-lyrics-container="true">…</div>` block
/// from the page HTML and concatenate their text content. Newlines
/// are inferred from `<br>` tags so verses keep their shape.
fn extract_genius_lyrics(html: &str) -> Option<String> {
    let needle = r#"data-lyrics-container="true""#;
    let mut acc = String::new();
    let mut search_from = 0;
    let mut found_any = false;

    while let Some(rel) = html[search_from..].find(needle) {
        let attr_pos = search_from + rel;
        // Walk back to the `<div` that owns this attribute.
        let div_start = html[..attr_pos].rfind("<div").unwrap_or(attr_pos);
        // Walk forward to the matching `</div>`. Genius nests
        // `<a>`, `<span>`, `<i>` inside, but no further `<div>`s
        // inside a single lyrics container in current production
        // markup (2024–2026). If they ever do, we just truncate at
        // the first `</div>` and miss the tail — which is still
        // better than parsing nothing.
        let after = &html[div_start..];
        let div_end_rel = after.find("</div>")?;
        let block = &after[..div_end_rel];
        // Inner HTML only: skip past the opening tag's `>`.
        let inner_start = block.find('>').map(|i| i + 1).unwrap_or(0);
        let inner = &block[inner_start..];

        acc.push_str(&strip_html(inner));
        acc.push('\n');
        found_any = true;
        search_from = div_start + div_end_rel + "</div>".len();
    }

    if found_any {
        Some(acc)
    } else {
        None
    }
}

/// Tiny HTML-to-text helper. Replaces `<br>` and `<br/>` with
/// newlines, drops every other tag, then decodes the most common
/// HTML entities.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '<' {
            // Read up to the matching '>'.
            let mut tag = String::new();
            for c in chars.by_ref() {
                if c == '>' {
                    break;
                }
                tag.push(c);
            }
            let lower = tag.to_ascii_lowercase();
            let lower = lower.trim_start();
            if lower.starts_with("br") {
                out.push('\n');
            }
            // Every other tag: skipped (we just want text).
        } else {
            out.push(ch);
        }
    }

    decode_html_entities(&out)
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_lrc() {
        let body = "[ar: Pink Floyd]\n[ti: Time]\n[00:01.00]Tick tock\n[00:03.50]Time goes by";
        let lines = parse_lrc(body);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].ms, 1_000);
        assert_eq!(lines[0].text, "Tick tock");
        assert_eq!(lines[1].ms, 3_500);
    }

    #[test]
    fn handles_multi_timestamp_lines() {
        let body = "[00:01.00][00:30.00]Repeating chorus";
        let lines = parse_lrc(body);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].ms, 1_000);
        assert_eq!(lines[1].ms, 30_000);
        assert_eq!(lines[0].text, lines[1].text);
    }

    #[test]
    fn slug_lowers_and_hyphenates() {
        assert_eq!(genius_slug("Daft Punk", "Get Lucky"), "daft-punk-get-lucky");
        assert_eq!(
            genius_slug("Tame Impala", "Let It Happen"),
            "tame-impala-let-it-happen",
        );
    }

    #[test]
    fn slug_drops_punctuation_and_collapses_dashes() {
        // "Sigur Rós" → diacritics dropped silently → "Sigur Rs",
        // "Hoppípolla" → "Hopppolla". The combined slug is what
        // Genius's lyric URLs typically end up looking like for
        // tracks with non-ASCII characters.
        assert_eq!(genius_slug("Sigur Rós", "Hoppípolla"), "sigur-rs-hopppolla");
        assert_eq!(genius_slug("AC/DC", "T.N.T."), "ac-dc-t-n-t");
    }

    #[test]
    fn strips_basic_html_and_br() {
        let html = "Line one<br>Line two<br/>Line three<i>!</i>";
        assert_eq!(strip_html(html), "Line one\nLine two\nLine three!");
    }

    #[test]
    fn extracts_genius_block() {
        let html = r#"
            <html><body>
              <div data-lyrics-container="true" class="x">
                Verse one<br>Verse two
              </div>
              <p>Other text</p>
              <div data-lyrics-container="true">
                Chorus<br>Chorus two
              </div>
            </body></html>"#;
        let extracted = extract_genius_lyrics(html).unwrap();
        assert!(extracted.contains("Verse one"));
        assert!(extracted.contains("Verse two"));
        assert!(extracted.contains("Chorus"));
        assert!(!extracted.contains("Other text"));
    }
}
