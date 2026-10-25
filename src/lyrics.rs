use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

use crate::library::Track;
use crate::util;

#[derive(Clone, Debug)]
pub struct LyricLine {
    pub time: f64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Local,
    Embedded,
    Cache,
    Lrclib,
}

impl Source {
    pub fn note(self) -> &'static str {
        match self {
            Source::Local => "file",
            Source::Embedded => "tags",
            Source::Cache => "cache",
            Source::Lrclib => "lrclib.net",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub synced: bool,
    pub instrumental: bool,
    pub source: Source,
}

/// One banner line and its footer note.
pub struct CurrentLyric {
    pub text: String,
    pub note: String,
    pub start: f64,
    pub end: f64,
}

impl CurrentLyric {
    pub fn progress(&self, t: f64) -> f64 {
        if self.end <= self.start {
            0.0
        } else {
            ((t - self.start) / (self.end - self.start)).clamp(0.0, 1.0)
        }
    }
}

impl Lyrics {
    /// The line playing at `pos`; plain lyrics use the manually selected index.
    pub fn current(&self, pos: f64, title: &str, plain_index: usize) -> CurrentLyric {
        if self.instrumental {
            return CurrentLyric {
                text: "INSTRUMENTAL".to_string(),
                note: "instrumental".to_string(),
                start: 0.0,
                end: 0.0,
            };
        }
        if self.synced {
            let idx = self.lines.partition_point(|line| line.time <= pos);
            if idx == 0 {
                return CurrentLyric {
                    text: title.to_string(),
                    note: "intro".to_string(),
                    start: 0.0,
                    end: self.lines.first().map(|line| line.time).unwrap_or(0.0),
                };
            }
            let index = idx - 1;
            let start = self.lines[index].time;
            let end = self
                .lines
                .get(index + 1)
                .map(|line| line.time)
                .unwrap_or(start + 8.0);
            return CurrentLyric {
                text: self.lines[index].text.clone(),
                note: String::new(),
                start,
                end,
            };
        }
        if !self.lines.is_empty() {
            let index = plain_index.min(self.lines.len() - 1);
            return CurrentLyric {
                text: self.lines[index].text.clone(),
                note: format!(
                    "unsynced · {} / {} · ↑/↓",
                    index + 1,
                    self.lines.len()
                ),
                start: 0.0,
                end: 0.0,
            };
        }
        CurrentLyric {
            text: String::new(),
            note: String::new(),
            start: 0.0,
            end: 0.0,
        }
    }
}

fn parse_stamp(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let parts: Vec<&str> = raw.split(':').collect();
    match parts.len() {
        2 => {
            let minutes: f64 = parts[0].parse().ok()?;
            let seconds: f64 = parts[1].replace(',', ".").parse().ok()?;
            Some(minutes * 60.0 + seconds)
        }
        3 => {
            let minutes: f64 = parts[0].parse().ok()?;
            let seconds: f64 = parts[1].parse().ok()?;
            let frac: f64 = parts[2].replace(',', ".").parse().ok()?;
            Some(minutes * 60.0 + seconds + frac / 100.0)
        }
        _ => None,
    }
}

/// Strip enhanced-LRC word timings.
fn strip_word_tags(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('>') {
            let tag = &after[..end];
            if tag.starts_with(|c: char| c.is_ascii_digit()) && tag.contains(':') {
                rest = &after[end + 1..];
                continue;
            }
        }
        out.push('<');
        rest = after;
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    // `[offset:+500]` shifts every timestamp.
    let mut offset = 0.0;
    for raw in text.lines() {
        let line = raw.trim();
        let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
            continue;
        };
        if let Some((key, value)) = inner.split_once(':') {
            if key.trim().eq_ignore_ascii_case("offset") {
                if let Ok(ms) = value.trim().parse::<f64>() {
                    offset = ms / 1000.0;
                }
            }
        }
    }
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim_start();
        let mut stamps = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[1..end];
            if let Some(time) = parse_stamp(tag) {
                stamps.push(time);
            }
            rest = &rest[end + 1..];
        }
        let body = strip_word_tags(rest);
        if body.is_empty() {
            continue;
        }
        for time in stamps {
            out.push(LyricLine {
                time: (time + offset).max(0.0),
                text: body.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (a.time - b.time).abs() < 0.001 && a.text == b.text);
    out
}

fn from_plain(text: &str, duration: f64) -> Vec<LyricLine> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let step = if duration > 5.0 {
        duration / lines.len() as f64
    } else {
        4.0
    };
    lines
        .iter()
        .enumerate()
        .map(|(i, text)| LyricLine {
            time: i as f64 * step,
            text: (*text).to_string(),
        })
        .collect()
}

fn build(
    synced: Option<String>,
    plain: Option<String>,
    duration: f64,
    source: Source,
    instrumental: bool,
) -> Option<Lyrics> {
    // Real text wins over the instrumental flag; the flag only matters when
    // there is no text at all.
    if let Some(text) = synced {
        let lines = parse_lrc(&text);
        if !lines.is_empty() {
            return Some(Lyrics {
                lines,
                synced: true,
                instrumental: false,
                source,
            });
        }
    }
    if let Some(text) = plain {
        let lines = from_plain(&text, duration);
        if !lines.is_empty() {
            return Some(Lyrics {
                lines,
                synced: false,
                instrumental: false,
                source,
            });
        }
    }
    if instrumental {
        return Some(Lyrics {
            lines: Vec::new(),
            synced: false,
            instrumental: true,
            source,
        });
    }
    None
}

fn local_lrc_path(track: &Track) -> Option<PathBuf> {
    // Exact same basename first.
    for ext in ["lrc", "LRC"] {
        let path = track.path.with_extension(ext);
        if path.is_file() {
            return Some(path);
        }
    }

    let parent = track.path.parent()?;
    let entries = fs::read_dir(parent).ok()?;
    let stem = track
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let track_norm = normalize_name(&stem);
    let title_norm = normalize_name(track.title_or());
    let artist_norm = normalize_name(track.artist_or());

    let mut best: Option<(i32, PathBuf)> = None;
    let mut tie = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_lrc = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("lrc"))
            .unwrap_or(false);
        if !is_lrc {
            continue;
        }
        let Some(file_stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        let file_norm = normalize_name(&file_stem);
        if file_norm.is_empty() {
            continue;
        }
        let score = if !track_norm.is_empty() && file_norm == track_norm {
            100
        } else if !title_norm.is_empty() && file_norm == title_norm {
            90
        } else if contains_phrase(&file_norm, &title_norm) {
            70
        } else if !artist_norm.is_empty()
            && contains_phrase(&file_norm, &artist_norm)
            && contains_phrase(&file_norm, &title_norm)
        {
            50
        } else {
            continue;
        };
        match &best {
            Some((best_score, _)) if score < *best_score => {}
            Some((best_score, _)) if score == *best_score => tie = true,
            _ => {
                best = Some((score, path));
                tie = false;
            }
        }
    }
    if tie {
        None
    } else {
        best.map(|(_, path)| path)
    }
}

fn normalize_name(raw: &str) -> String {
    let stripped = strip_track_number(raw.trim());
    let mut out = String::new();
    for ch in stripped.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
        } else if ch.is_whitespace()
            || matches!(ch, '-' | '_' | '.' | ',' | '(' | ')' | '[' | ']')
        {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        }
    }
    out.trim().to_string()
}

fn strip_track_number(raw: &str) -> &str {
    let bytes = raw.as_bytes();
    let mut digits = 0;
    while digits < bytes.len() && bytes[digits].is_ascii_digit() {
        digits += 1;
    }
    if digits == 0 || digits > 3 {
        return raw;
    }
    let rest = raw[digits..].trim_start_matches(|c: char| matches!(c, '.' | '-' | '_' | ')' | ' '));
    if rest.is_empty() {
        raw
    } else {
        rest
    }
}

fn contains_phrase(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack == needle
        || haystack.starts_with(&format!("{needle} "))
        || haystack.ends_with(&format!(" {needle}"))
        || haystack.contains(&format!(" {needle} "))
}

fn local_lrc(track: &Track) -> Option<String> {
    let path = local_lrc_path(track)?;
    fs::read_to_string(path).ok()
}

/// Save lyrics next to the track, replacing an existing local `.lrc` if there
/// is one; otherwise write `<track>.lrc`.
pub fn save_lrc(track: &Track, lyrics: &Lyrics) -> Result<PathBuf, String> {
    if lyrics.lines.is_empty() {
        return Err("nothing to save".to_string());
    }
    let path = local_lrc_path(track).unwrap_or_else(|| track.path.with_extension("lrc"));
    let mut out = String::new();
    if lyrics.synced {
        for line in &lyrics.lines {
            out.push_str(&format!("[{}]{}\n", lrc_stamp(line.time), line.text));
        }
    } else {
        for line in &lyrics.lines {
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    fs::write(&path, out).map_err(|error| error.to_string())?;
    Ok(path)
}

fn lrc_stamp(seconds: f64) -> String {
    let centis = (seconds.max(0.0) * 100.0).round() as u64;
    let minutes = centis / 6000;
    let rest = (centis % 6000) as f64 / 100.0;
    format!("{minutes:02}:{rest:05.2}")
}

fn cache_key(track: &Track) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    track.artist.to_lowercase().hash(&mut hasher);
    track.title.to_lowercase().hash(&mut hasher);
    track.album.to_lowercase().hash(&mut hasher);
    (track.duration as i64).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct Cached {
    #[serde(default)]
    synced: Option<String>,
    #[serde(default)]
    plain: Option<String>,
    #[serde(default)]
    instrumental: bool,
}

fn cache_file(key: &str) -> PathBuf {
    util::cache_dir().join("lyrics").join(format!("{key}.json"))
}

fn read_cache(key: &str) -> Option<Cached> {
    let text = fs::read_to_string(cache_file(key)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(key: &str, cached: &Cached) {
    let dir = util::cache_dir().join("lyrics");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(cached) {
        let _ = fs::write(dir.join(format!("{key}.json")), text);
    }
}

#[derive(Debug, Deserialize)]
struct ApiTrack {
    #[serde(rename = "syncedLyrics", default)]
    synced: Option<String>,
    #[serde(rename = "plainLyrics", default)]
    plain: Option<String>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    duration: f64,
}

fn agent() -> ureq::Agent {
    // The request runs in the background, so a generous timeout is fine.
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(20))
        .build()
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// lrclib base URL. `TMUS_LRCLIB_URL` lets tests point at a mock.
fn api_url(path: &str) -> String {
    match std::env::var("TMUS_LRCLIB_URL") {
        Ok(base) if !base.trim().is_empty() => {
            let base = base.trim().trim_end_matches('/');
            let root = base.strip_suffix("/api/get").unwrap_or(base);
            format!("{root}{path}")
        }
        _ => format!("https://lrclib.net{path}"),
    }
}

fn fetch(track: &Track) -> Result<Cached, String> {
    let agent = agent();
    let mut url = format!(
        "{}?artist_name={}&track_name={}",
        api_url("/api/get"),
        encode(&track.artist),
        encode(&track.title)
    );
    if !track.album.trim().is_empty() {
        url.push_str(&format!("&album_name={}", encode(&track.album)));
    }
    if track.duration > 0.0 {
        url.push_str(&format!("&duration={}", track.duration.round() as i64));
    }

    let response = agent
        .get(&url)
        .set("User-Agent", "tmus/0.1 (TUI player; lrclib.net)")
        .call();

    let found: Option<ApiTrack> = match response {
        Ok(resp) => match resp.into_json() {
            Ok(track) => Some(track),
            Err(e) => {
                let text = e.to_string();
                if text.contains("timed out") || text.contains("timeout") {
                    return Err("lrclib did not respond in time".to_string());
                }
                return Err(format!("failed to read lrclib response: {text}"));
            }
        },
        Err(ureq::Error::Status(404, _)) => None,
        Err(ureq::Error::Status(_, _)) => None,
        Err(ureq::Error::Transport(e)) => {
            return Err(format!("network unavailable: {e}"));
        }
    };

    if found.as_ref().is_some_and(|item| item.instrumental) {
        return Ok(Cached {
            synced: None,
            plain: None,
            instrumental: true,
        });
    }

    let found = match found {
        Some(item) if item.synced.is_some() => Some(item),
        Some(item) => {
            // The exact lookup can return a plain-only entry while search has
            // a timed one. Keep the plain entry if the search fails.
            match search(&agent, track) {
                Ok(Some(better)) if better.synced.is_some() => Some(better),
                _ => Some(item),
            }
        }
        None => search(&agent, track)?,
    };

    let Some(found) = found else {
        return Err("lyrics not found on lrclib.net".to_string());
    };
    if found.synced.is_none() && found.plain.is_none() {
        return Err("lrclib.net has no lyrics for this track".to_string());
    }
    Ok(Cached {
        synced: found.synced,
        plain: found.plain,
        instrumental: false,
    })
}

fn search(agent: &ureq::Agent, track: &Track) -> Result<Option<ApiTrack>, String> {
    let mut url = format!(
        "{}?track_name={}&artist_name={}",
        api_url("/api/search"),
        encode(&track.title),
        encode(&track.artist)
    );
    if url.len() > 1800 {
        url = format!("{}?q={}", api_url("/api/search"), encode(&track.label()));
    }
    let response = match agent
        .get(&url)
        .set("User-Agent", "tmus/0.1 (TUI player; lrclib.net)")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(_, _)) => return Ok(None),
        Err(ureq::Error::Transport(e)) => {
            return Err(format!("network unavailable: {e}"));
        }
    };
    let list: Vec<ApiTrack> = response
        .into_json()
        .map_err(|e| format!("failed to read lrclib response: {e}"))?;

    let mut best: Option<ApiTrack> = None;
    let mut best_score = f64::MAX;
    for item in list {
        if item.instrumental || (item.synced.is_none() && item.plain.is_none()) {
            continue;
        }
        let mut score = if item.synced.is_some() { 0.0 } else { 1000.0 };
        if track.duration > 0.0 && item.duration > 0.0 {
            score += (item.duration - track.duration).abs();
        }
        if score < best_score {
            best_score = score;
            best = Some(item);
        }
    }
    Ok(best)
}

pub fn load_blocking(track: &Track) -> Result<Lyrics, String> {
    if let Some(text) = local_lrc(track) {
        // A local .lrc may be synced or plain; try both parsers.
        if let Some(lyrics) = build(
            Some(text.clone()),
            Some(text),
            track.duration,
            Source::Local,
            false,
        ) {
            return Ok(lyrics);
        }
    }

    if let Ok(tags) = crate::tags::read(&track.path) {
        if !tags.lyrics.trim().is_empty() {
            if let Some(lyrics) = build(
                Some(tags.lyrics.clone()),
                Some(tags.lyrics),
                track.duration,
                Source::Embedded,
                false,
            ) {
                return Ok(lyrics);
            }
        }
    }

    let key = cache_key(track);
    let cached = read_cache(&key);
    if let Some(cached) = &cached {
        if let Some(lyrics) = build(
            cached.synced.clone(),
            cached.plain.clone(),
            track.duration,
            Source::Cache,
            cached.instrumental,
        ) {
            return Ok(lyrics);
        }
    }

    fetch_blocking(track)
}

fn fetch_blocking(track: &Track) -> Result<Lyrics, String> {
    let key = cache_key(track);
    let fresh = fetch(track)?;
    write_cache(&key, &fresh);
    build(
        fresh.synced,
        fresh.plain,
        track.duration,
        Source::Lrclib,
        fresh.instrumental,
    )
    .ok_or_else(|| "lyrics not found".to_string())
}

pub fn spawn(track: Track) -> Receiver<Result<Lyrics, String>> {
    spawn_thread(move || load_blocking(&track))
}

/// Fetch from lrclib even when a local `.lrc` or a cached copy exists.
pub fn spawn_remote(track: Track) -> Receiver<Result<Lyrics, String>> {
    spawn_thread(move || fetch_blocking(&track))
}

fn spawn_thread(
    load: impl FnOnce() -> Result<Lyrics, String> + Send + 'static,
) -> Receiver<Result<Lyrics, String>> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(load());
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_and_enhanced_tags() {
        let lines = parse_lrc(
            "[offset:-500]\n[00:10.00]<00:10.00>Hello <00:10.50>world\n[00:20.00]Bye",
        );
        assert_eq!(lines.len(), 2);
        assert!((lines[0].time - 9.5).abs() < 1e-6);
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[1].text, "Bye");
    }

    #[test]
    fn several_stamps_on_one_line() {
        let lines = parse_lrc("[00:01.00][00:03.00]Again");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time, 1.0);
        assert_eq!(lines[1].time, 3.0);
    }

    #[test]
    fn plain_text_gets_placeholder_times() {
        let lines = from_plain("one\ntwo\nthree", 30.0);
        assert_eq!(lines.len(), 3);
        assert!((lines[1].time - 10.0).abs() < 1e-6);
    }

    #[test]
    fn instrumental_only_when_there_is_no_text() {
        let plain = build(None, Some("one\ntwo".into()), 20.0, Source::Cache, true)
            .expect("plain lyrics");
        assert!(!plain.instrumental);
        assert!(!plain.synced);
        assert_eq!(plain.lines.len(), 2);

        let synced = build(
            Some("[00:01.00]hi".into()),
            None,
            20.0,
            Source::Cache,
            true,
        )
        .expect("synced lyrics");
        assert!(!synced.instrumental);
        assert!(synced.synced);

        let none = build(None, None, 20.0, Source::Cache, true).expect("instrumental");
        assert!(none.instrumental);
        assert!(none.lines.is_empty());
    }

    #[test]
    fn current_line_matches_lyr() {
        let lyrics = build(
            Some("[00:00.00]intro\n[00:10.00]middle\n[00:20.00]end".into()),
            None,
            30.0,
            Source::Local,
            false,
        )
        .expect("lyrics");
        let intro = lyrics.current(2.0, "Title", 0);
        assert_eq!(intro.text, "intro");
        assert!((intro.end - 10.0).abs() < 1e-6);
        let middle = lyrics.current(12.0, "Title", 0);
        assert_eq!(middle.text, "middle");
        assert!((middle.progress(15.0) - 0.5).abs() < 1e-6);
        let last = lyrics.current(25.0, "Title", 0);
        assert_eq!(last.text, "end");
        assert!((last.end - 28.0).abs() < 1e-6);
    }

    #[test]
    fn intro_shows_the_title() {
        let lyrics = build(
            Some("[00:05.00]first".into()),
            None,
            30.0,
            Source::Local,
            false,
        )
        .expect("lyrics");
        let intro = lyrics.current(1.0, "My Song", 0);
        assert_eq!(intro.text, "My Song");
        assert_eq!(intro.note, "intro");
    }

    #[test]
    fn local_plain_lrc_is_used_without_network() {
        let dir = std::env::temp_dir().join(format!("tmus-lrc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mp3 = dir.join("song.mp3");
        fs::write(&mp3, b"").unwrap();
        fs::write(dir.join("song.lrc"), "one\ntwo\nthree\n").unwrap();
        let track = Track {
            path: mp3,
            artist: "A".into(),
            title: "T".into(),
            album: "L".into(),
            duration: 30.0,
            track_no: None,
            year: None,
            genre: String::new(),
        };
        let lyrics = load_blocking(&track).expect("local plain lyrics");
        assert_eq!(lyrics.source, Source::Local);
        assert!(!lyrics.synced);
        assert!(!lyrics.instrumental);
        assert_eq!(lyrics.lines.len(), 3);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plain_lyrics_use_the_manual_index() {
        let lyrics = build(None, Some("one\ntwo\nthree".into()), 30.0, Source::Local, false)
            .expect("plain lyrics");
        assert_eq!(lyrics.current(0.0, "T", 0).text, "one");
        assert_eq!(lyrics.current(29.0, "T", 1).text, "two");
        assert_eq!(lyrics.current(29.0, "T", 99).text, "three");
        assert!(lyrics.current(10.0, "T", 1).note.contains("2 / 3"));
    }

    #[test]
    fn folder_lrc_matches_the_track_title() {
        let dir = std::env::temp_dir().join(format!("tmus-lrc-folder-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mp3 = dir.join("01 - Girls, Guns & Dynamite.mp3");
        fs::write(&mp3, b"").unwrap();
        fs::write(
            dir.join("Girls, Guns & Dynamite.lrc"),
            "[00:01.00]hello\n",
        )
        .unwrap();
        let track = Track {
            path: mp3,
            artist: "Her Words Kill".into(),
            title: "Girls, Guns & Dynamite".into(),
            album: "Load My Revolver, Baby".into(),
            duration: 30.0,
            track_no: Some(1),
            year: None,
            genre: String::new(),
        };
        assert_eq!(local_lrc(&track).as_deref(), Some("[00:01.00]hello\n"));

        let saved = save_lrc(&track, &load_blocking(&track).unwrap()).unwrap();
        assert_eq!(saved.file_name().unwrap(), "Girls, Guns & Dynamite.lrc");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remote_lyrics_replace_a_plain_local_file() {
        let dir = std::env::temp_dir().join(format!("tmus-lrc-replace-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mp3 = dir.join("01 - Song.mp3");
        fs::write(&mp3, b"").unwrap();
        fs::write(dir.join("Song.lrc"), "plain\n").unwrap();
        let track = Track {
            path: mp3,
            artist: "Artist".into(),
            title: "Song".into(),
            album: "Album".into(),
            duration: 30.0,
            track_no: Some(1),
            year: None,
            genre: String::new(),
        };
        let lyrics = build(
            Some("[00:01.00]timed\n".into()),
            None,
            30.0,
            Source::Lrclib,
            false,
        )
        .unwrap();
        let saved = save_lrc(&track, &lyrics).unwrap();
        assert_eq!(saved.file_name().unwrap(), "Song.lrc");
        assert_eq!(fs::read_to_string(saved).unwrap(), "[00:01.00]timed\n");
        fs::remove_dir_all(&dir).ok();
    }
}
