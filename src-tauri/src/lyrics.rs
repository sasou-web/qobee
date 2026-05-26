//! Lyrics extraction for a track.
//!
//! Tries, in order:
//!   1. A sidecar `.lrc` file next to the audio file (synced lyrics).
//!   2. The `Lyrics` tag embedded in the file's metadata (lofty).
//!
//! Synced lyrics are parsed from LRC `[mm:ss.xx]` line prefixes and
//! returned as a sorted `Vec<{ ms, text }>`. Unsynced lyrics are
//! returned as a single string. Both can be present (synced takes
//! precedence in the UI).

use std::fs;
use std::path::Path;

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

/// Read lyrics for the audio file at `audio_path`.
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
}
