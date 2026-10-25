use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::util;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub album: String,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub track_no: Option<u32>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub genre: String,
}

impl Track {
    pub fn artist_or(&self) -> &str {
        if self.artist.trim().is_empty() {
            "Unknown artist"
        } else {
            &self.artist
        }
    }

    pub fn title_or(&self) -> &str {
        if self.title.trim().is_empty() {
            "Untitled"
        } else {
            &self.title
        }
    }

    pub fn album_or(&self) -> &str {
        if self.album.trim().is_empty() {
            "Unknown album"
        } else {
            &self.album
        }
    }

    pub fn label(&self) -> String {
        format!("{} — {}", self.artist_or(), self.title_or())
    }

    pub fn list_text(&self) -> String {
        let num = self.track_no.unwrap_or(0);
        let dur = util::fmt_time(self.duration);
        if num > 0 {
            format!("{num:>2}. {}   {dur}", self.title_or())
        } else {
            format!("{}. {}   {dur}", "  ", self.title_or())
        }
    }

    pub fn sort_key(&self) -> (String, i32, String, u32, String) {
        (
            self.artist_or().to_lowercase(),
            self.year.unwrap_or(0),
            self.album_or().to_lowercase(),
            self.track_no.unwrap_or(0),
            self.title_or().to_lowercase(),
        )
    }
}

const AUDIO_EXT: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "m4b", "aac", "wav", "wv", "ape", "mpc",
    "aiff", "aif", "wma", "mka", "webm", "mp4",
];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn is_playlist(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_lowercase().as_str(), "m3u" | "m3u8" | "pls"))
        .unwrap_or(false)
}

/// Read m3u/m3u8/pls; relative entries resolve against the playlist. Missing
/// files are skipped.
pub fn read_playlist(path: &Path) -> Vec<Track> {
    let text = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return Vec::new(),
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let pls = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pls"))
        .unwrap_or(false);

    let mut entries = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let value = if pls {
            // File1=/path/to/track.mp3
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if !key.trim().to_lowercase().starts_with("file") {
                continue;
            }
            value.trim()
        } else {
            line
        };
        if value.is_empty() {
            continue;
        }
        let (value, is_uri) = match value.strip_prefix("file://") {
            Some(rest) => (rest, true),
            None => (value, false),
        };
        let decoded = if is_uri {
            percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .into_owned()
        } else {
            value.to_string()
        };
        // Playlist entries are relative to the file, not the process cwd.
        let candidate = if decoded == "~" {
            util::home()
        } else if let Some(rest) = decoded.strip_prefix("~/") {
            util::home().join(rest)
        } else {
            PathBuf::from(&decoded)
        };
        let full = if candidate.is_absolute() {
            candidate
        } else {
            parent.join(candidate)
        };
        if full.is_file() && is_audio(&full) {
            entries.push(read_track(&full));
        }
    }
    entries
}

/// Write m3u/m3u8, with `#EXTINF` in extended mode.
pub fn write_playlist(path: &Path, tracks: &[Track], extended: bool) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let m3u = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_lowercase().as_str(), "m3u" | "m3u8"))
        .unwrap_or(false);
    let mut out = String::new();
    if m3u {
        out.push_str("#EXTM3U\n");
    }
    for track in tracks {
        if extended && m3u {
            out.push_str(&format!(
                "#EXTINF:{},{}\n",
                track.duration.round() as i64,
                track.label()
            ));
        }
        out.push_str(&track.path.to_string_lossy());
        out.push('\n');
    }
    let mut file = fs::File::create(path)?;
    file.write_all(out.as_bytes())
}

pub fn read_track(path: &Path) -> Track {
    let mut track = Track {
        path: path.to_path_buf(),
        artist: String::new(),
        title: path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        album: String::new(),
        duration: 0.0,
        track_no: None,
        year: None,
        genre: String::new(),
    };

    if let Ok(tagged) = lofty::probe::Probe::open(path).and_then(|p| p.read()) {
        use lofty::prelude::*;
        track.duration = tagged.properties().duration().as_secs_f64();
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            if let Some(v) = tag.artist() {
                track.artist = v.to_string();
            }
            if let Some(v) = tag.title() {
                track.title = v.to_string();
            }
            if let Some(v) = tag.album() {
                track.album = v.to_string();
            }
            track.track_no = tag.track();
            track.year = tag
                .get_string(ItemKey::Year)
                .or_else(|| tag.get_string(ItemKey::RecordingDate))
                .and_then(|value| value.get(..4).unwrap_or(value).parse::<i32>().ok());
            if let Some(value) = tag.genre() {
                track.genre = value.to_string();
            }
        }
    }

    // "Artist - Title" is a very common naming scheme for untagged files.
    if track.artist.trim().is_empty() {
        if let Some((artist, title)) = track.title.split_once(" - ") {
            track.artist = artist.trim().to_string();
            track.title = title.trim().to_string();
        }
    }
    track
}

pub fn scan_dir(root: &Path) -> Vec<Track> {
    let mut out = Vec::new();
    walk(root, &mut out, 0);
    out
}

fn walk(dir: &Path, out: &mut Vec<Track>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, out, depth + 1);
        } else if kind.is_file() && is_audio(&path) {
            out.push(read_track(&path));
        }
    }
}

pub fn audio_files_in(dir: &Path, hidden: bool) -> Vec<Track> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Track> = entries
        .flatten()
        .filter(|e| e.file_type().map(|k| k.is_file()).unwrap_or(false))
        .filter(|e| hidden || !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .filter(|p| is_audio(p))
        .map(|p| read_track(&p))
        .collect();
    out.sort_by_key(|t| t.sort_key());
    out
}

pub fn merge(base: &mut Vec<Track>, new: Vec<Track>) {
    let mut known: HashSet<PathBuf> = base.iter().map(|t| t.path.clone()).collect();
    for track in new {
        if known.insert(track.path.clone()) {
            base.push(track);
        }
    }
    sort_library(base);
}

pub fn sort_library(tracks: &mut [Track]) {
    tracks.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
}

pub fn load_library() -> Vec<Track> {
    let path = util::data_dir().join("library.json");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_library(tracks: &[Track]) {
    let dir = util::data_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(tracks) {
        let _ = fs::write(dir.join("library.json"), text);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Persisted {
    #[serde(default)]
    pub roots: Vec<PathBuf>,
    #[serde(default)]
    pub playlist: Vec<PathBuf>,
    #[serde(default)]
    pub repeat: bool,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub follow: bool,
    #[serde(default)]
    pub continue_play: bool,
    #[serde(default)]
    pub repeat_current: bool,
    #[serde(default)]
    pub aaa_mode: bool,
    #[serde(default)]
    pub play_library: bool,
    #[serde(default)]
    pub show_remaining: bool,
    #[serde(default = "default_cava")]
    pub cava: bool,
    #[serde(default)]
    pub lyrics_offset: f64,
    #[serde(default)]
    pub keybindings: HashMap<String, Vec<String>>,
}

fn default_cava() -> bool {
    true
}

pub fn load_state() -> Persisted {
    let path = util::config_dir().join("state.json");
    let Ok(text) = fs::read_to_string(path) else {
        return Persisted {
            cava: true,
            ..Default::default()
        };
    };
    serde_json::from_str(&text).unwrap_or(Persisted {
        cava: true,
        ..Default::default()
    })
}

pub fn save_state(state: &Persisted) {
    let dir = util::config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(state) {
        let _ = fs::write(dir.join("state.json"), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tmus-playlist-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn playlist_roundtrip_plain_m3u_and_pls() {
        let dir = temp_dir("roundtrip");
        let audio = dir.join("a.mp3");
        fs::write(&audio, b"").unwrap();
        let track = Track {
            path: audio.clone(),
            artist: "A".into(),
            title: "T".into(),
            album: "L".into(),
            duration: 42.0,
            track_no: None,
            year: None,
            genre: String::new(),
        };

        let m3u = dir.join("list.m3u");
        write_playlist(&m3u, &[track.clone()], true).unwrap();
        let text = fs::read_to_string(&m3u).unwrap();
        assert!(text.starts_with("#EXTM3U\n#EXTINF:42,A — T\n"));
        let loaded = read_playlist(&m3u);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].path, audio);

        // Relative entries are resolved against the playlist directory.
        let relative = dir.join("relative.m3u");
        fs::write(&relative, "a.mp3\n# comment\nmissing.mp3\n").unwrap();
        assert_eq!(read_playlist(&relative).len(), 1);

        let pls = dir.join("list.pls");
        fs::write(
            &pls,
            format!("[playlist]\nFile1={}\n", audio.display()),
        )
        .unwrap();
        assert_eq!(read_playlist(&pls).len(), 1);

        // file:// URIs are percent-decoded.
        let spaced = dir.join("b c.mp3");
        fs::write(&spaced, b"").unwrap();
        let uri = dir.join("uri.m3u");
        fs::write(
            &uri,
            format!("file://{}\n", spaced.to_string_lossy().replace(' ', "%20")),
        )
        .unwrap();
        let from_uri = read_playlist(&uri);
        assert_eq!(from_uri.len(), 1);
        assert_eq!(from_uri[0].path, spaced);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_playlist_is_empty() {
        let dir = temp_dir("missing");
        assert!(read_playlist(&dir.join("nope.m3u")).is_empty());
        fs::remove_dir_all(&dir).ok();
    }
}
