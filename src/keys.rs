use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key combination stored without SHIFT: shifted characters are kept as
/// their uppercase form, so `B` and `b` are distinct bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl Binding {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Binding {
        Binding {
            code,
            modifiers: modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT),
        }
    }

    pub fn from_event(event: &KeyEvent) -> Option<Binding> {
        let code = match event.code {
            KeyCode::Char(c) if !c.is_control() => KeyCode::Char(c),
            KeyCode::F(n) => KeyCode::F(n),
            KeyCode::Left => KeyCode::Left,
            KeyCode::Right => KeyCode::Right,
            KeyCode::Up => KeyCode::Up,
            KeyCode::Down => KeyCode::Down,
            KeyCode::Home => KeyCode::Home,
            KeyCode::End => KeyCode::End,
            KeyCode::PageUp => KeyCode::PageUp,
            KeyCode::PageDown => KeyCode::PageDown,
            KeyCode::Enter => KeyCode::Enter,
            KeyCode::Tab => KeyCode::Tab,
            KeyCode::Esc => KeyCode::Esc,
            KeyCode::Backspace => KeyCode::Backspace,
            KeyCode::Delete => KeyCode::Delete,
            _ => return None,
        };
        Some(Binding::new(code, event.modifiers))
    }

    pub fn parse(raw: &str) -> Option<Binding> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let (key, modifiers) = if raw == "+" {
            ("+", KeyModifiers::NONE)
        } else if let Some((prefix, key)) = raw.rsplit_once('+') {
            if key.is_empty() {
                return None;
            }
            let mut modifiers = KeyModifiers::NONE;
            for part in prefix.split('+') {
                match part.trim().to_lowercase().as_str() {
                    "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                    "alt" => modifiers |= KeyModifiers::ALT,
                    "shift" => {}
                    "" => {}
                    _ => return None,
                }
            }
            (key, modifiers)
        } else {
            (raw, KeyModifiers::NONE)
        };
        let code = key_code(key)?;
        Some(Binding::new(code, modifiers))
    }

    pub fn matches(&self, event: &KeyEvent) -> bool {
        if self.code != event.code {
            return false;
        }
        let required = self.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
        let actual = event.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
        required == actual
    }

    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            out.push_str("Ctrl+");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        out.push_str(&key_name(self.code));
        out
    }
}

fn key_code(key: &str) -> Option<KeyCode> {
    let lower = key.to_lowercase();
    let code = match lower.as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "left" | "←" => KeyCode::Left,
        "right" | "→" => KeyCode::Right,
        "up" | "↑" => KeyCode::Up,
        "down" | "↓" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        _ => {
            if let Some(rest) = lower.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u8>() {
                    if (1..=12).contains(&n) {
                        return Some(KeyCode::F(n));
                    }
                }
            }
            let mut chars = key.chars();
            let first = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(first)
        }
    };
    Some(code)
}

fn key_name(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        _ => "?".to_string(),
    }
}

/// Keys the interface needs for itself. They cannot be assigned in the editor.
pub fn is_reserved(binding: Binding) -> bool {
    if binding.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = binding.code {
            if matches!(
                c.to_ascii_lowercase(),
                'c' | 'u' | 'f' | 'b' | 'y' | 'e' | 'l'
            ) {
                return true;
            }
        }
    }
    match binding.code {
        KeyCode::Enter
        | KeyCode::Tab
        | KeyCode::Esc
        | KeyCode::Backspace
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::Insert
        | KeyCode::F(1) => true,
        KeyCode::Char(c) => matches!(
            c,
            ' ' | '?' | '!' | 'g' | 'G' | 'j' | 'k' | 'u' | 'i' | '0' | '<' | '>' | ','
                | '.' | ']' | '[' | '}' | '{'
        ),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    PlayPause,
    Play,
    Stop,
    Next,
    Prev,
    NextAlbum,
    PrevAlbum,
    SeekBack,
    SeekForward,
    Shuffle,
    Repeat,
    RepeatCurrent,
    Continue,
    PlayLibrary,
    Follow,
    Remaining,
    ViewLibrary,
    ViewPlaylist,
    ViewBrowser,
    ViewSettings,
    ToggleLyrics,
    Command,
    Search,
    SearchNext,
    SearchPrev,
    Rescan,
    Info,
    AddToLibrary,
    AddToPlaylist,
    Remove,
    MoveUp,
    MoveDown,
    PlaylistBuilder,
    DeleteFile,
    SaveLyrics,
    Quit,
}

impl Action {
    pub const ALL: &'static [Action] = &[
        Action::PlayPause,
        Action::Play,
        Action::Stop,
        Action::Next,
        Action::Prev,
        Action::NextAlbum,
        Action::PrevAlbum,
        Action::SeekBack,
        Action::SeekForward,
        Action::Shuffle,
        Action::Repeat,
        Action::RepeatCurrent,
        Action::Continue,
        Action::PlayLibrary,
        Action::Follow,
        Action::Remaining,
        Action::ViewLibrary,
        Action::ViewPlaylist,
        Action::ViewBrowser,
        Action::ViewSettings,
        Action::ToggleLyrics,
        Action::Command,
        Action::Search,
        Action::SearchNext,
        Action::SearchPrev,
        Action::Rescan,
        Action::Info,
        Action::AddToLibrary,
        Action::AddToPlaylist,
        Action::Remove,
        Action::MoveUp,
        Action::MoveDown,
        Action::PlaylistBuilder,
        Action::DeleteFile,
        Action::SaveLyrics,
        Action::Quit,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Action::PlayPause => "play_pause",
            Action::Play => "play",
            Action::Stop => "stop",
            Action::Next => "next",
            Action::Prev => "prev",
            Action::NextAlbum => "next_album",
            Action::PrevAlbum => "prev_album",
            Action::SeekBack => "seek_back",
            Action::SeekForward => "seek_forward",
            Action::Shuffle => "shuffle",
            Action::Repeat => "repeat",
            Action::RepeatCurrent => "repeat_current",
            Action::Continue => "continue",
            Action::PlayLibrary => "play_library",
            Action::Follow => "follow",
            Action::Remaining => "remaining",
            Action::ViewLibrary => "view_library",
            Action::ViewPlaylist => "view_playlist",
            Action::ViewBrowser => "view_browser",
            Action::ViewSettings => "view_settings",
            Action::ToggleLyrics => "toggle_lyrics",
            Action::Command => "command",
            Action::Search => "search",
            Action::SearchNext => "search_next",
            Action::SearchPrev => "search_prev",
            Action::Rescan => "rescan",
            Action::Info => "info",
            Action::AddToLibrary => "add_to_library",
            Action::AddToPlaylist => "add_to_playlist",
            Action::Remove => "remove",
            Action::MoveUp => "move_up",
            Action::MoveDown => "move_down",
            Action::PlaylistBuilder => "playlist_builder",
            Action::DeleteFile => "delete_file",
            Action::SaveLyrics => "save_lyrics",
            Action::Quit => "quit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::PlayPause => "Pause / resume",
            Action::Play => "Play selected",
            Action::Stop => "Stop",
            Action::Next => "Next track",
            Action::Prev => "Previous track",
            Action::NextAlbum => "Next album",
            Action::PrevAlbum => "Previous album",
            Action::SeekBack => "Seek back 5 seconds",
            Action::SeekForward => "Seek forward 5 seconds",
            Action::Shuffle => "Shuffle",
            Action::Repeat => "Repeat list",
            Action::RepeatCurrent => "Repeat one track",
            Action::Continue => "Continue after end",
            Action::PlayLibrary => "Play from library",
            Action::Follow => "Follow current track",
            Action::Remaining => "Remaining time",
            Action::ViewLibrary => "Library view",
            Action::ViewPlaylist => "Playlist view",
            Action::ViewBrowser => "Browser view",
            Action::ViewSettings => "Settings view",
            Action::ToggleLyrics => "Lyrics",
            Action::Command => "Command line",
            Action::Search => "Search",
            Action::SearchNext => "Next match",
            Action::SearchPrev => "Previous match",
            Action::Rescan => "Rescan library",
            Action::Info => "Show tags",
            Action::AddToLibrary => "Add to library",
            Action::AddToPlaylist => "Add to playlist",
            Action::Remove => "Remove from list",
            Action::MoveUp => "Move up",
            Action::MoveDown => "Move down",
            Action::PlaylistBuilder => "Build playlist",
            Action::DeleteFile => "Delete file",
            Action::SaveLyrics => "Save lyrics",
            Action::Quit => "Quit",
        }
    }

    /// Playback and a few global actions stay usable while the lyrics screen
    /// is open; list and view actions are left for the normal screens.
    pub fn works_in_lyrics(self) -> bool {
        matches!(
            self,
            Action::Quit
                | Action::Command
                | Action::ToggleLyrics
                | Action::PlayPause
                | Action::Play
                | Action::Stop
                | Action::Next
                | Action::Prev
                | Action::NextAlbum
                | Action::PrevAlbum
                | Action::SeekBack
                | Action::SeekForward
                | Action::RepeatCurrent
                | Action::Follow
                | Action::Remaining
                | Action::SaveLyrics
        )
    }

    pub fn is_view(self) -> bool {
        matches!(
            self,
            Action::ViewLibrary
                | Action::ViewPlaylist
                | Action::ViewBrowser
                | Action::ViewSettings
        )
    }

    pub fn default_keys(self) -> &'static [&'static str] {
        match self {
            Action::PlayPause => &["c"],
            Action::Play => &["x"],
            Action::Stop => &["v"],
            Action::Next => &["b"],
            Action::Prev => &["z"],
            Action::NextAlbum => &["B"],
            Action::PrevAlbum => &["Z"],
            Action::SeekBack => &["h", "left"],
            Action::SeekForward => &["right"],
            Action::Shuffle => &["s"],
            Action::Repeat => &["r"],
            Action::RepeatCurrent => &["ctrl+r"],
            Action::Continue => &["C"],
            Action::PlayLibrary => &["M"],
            Action::Follow => &["f"],
            Action::Remaining => &["t"],
            Action::ViewLibrary => &["1"],
            Action::ViewPlaylist => &["2"],
            Action::ViewBrowser => &["3"],
            Action::ViewSettings => &["4"],
            Action::ToggleLyrics => &["l"],
            Action::Command => &[":"],
            Action::Search => &["/"],
            Action::SearchNext => &["n"],
            Action::SearchPrev => &["N"],
            Action::Rescan => &["U"],
            Action::Info => &["I"],
            Action::AddToLibrary => &["a"],
            Action::AddToPlaylist => &["y"],
            Action::Remove => &["D", "delete"],
            Action::MoveUp => &["P"],
            Action::MoveDown => &["p"],
            Action::PlaylistBuilder => &["ctrl+n"],
            Action::DeleteFile => &["ctrl+d"],
            Action::SaveLyrics => &["ctrl+s"],
            Action::Quit => &["q"],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: HashMap<Action, Vec<Binding>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new()
    }
}

impl Keymap {
    pub fn new() -> Keymap {
        let mut bindings = HashMap::new();
        for &action in Action::ALL {
            bindings.insert(action, parse_all(action.default_keys()));
        }
        Keymap { bindings }
    }

    pub fn from_persisted(raw: &HashMap<String, Vec<String>>) -> Keymap {
        let mut keymap = Keymap::new();
        // The old six-tab layout numbered its views 1, 3, 5 and 7. Keep the
        // current 1-4 defaults instead of restoring those stale bindings.
        let legacy_tabs = single_key(raw, "view_playlist", "3")
            && single_key(raw, "view_browser", "5")
            && single_key(raw, "view_settings", "7");
        for &action in Action::ALL {
            if legacy_tabs && action.is_view() {
                continue;
            }
            if let Some(keys) = raw.get(action.id()) {
                let parsed: Vec<Binding> = keys.iter().filter_map(|key| Binding::parse(key)).collect();
                keymap.bindings.insert(action, parsed);
            }
        }
        keymap
    }

    pub fn to_persisted(&self) -> HashMap<String, Vec<String>> {
        self.bindings
            .iter()
            .map(|(action, keys)| {
                (
                    action.id().to_string(),
                    keys.iter().map(|key| key.display()).collect(),
                )
            })
            .collect()
    }

    pub fn keys(&self, action: Action) -> &[Binding] {
        self.bindings
            .get(&action)
            .map(|keys| keys.as_slice())
            .unwrap_or(&[])
    }

    pub fn display(&self, action: Action) -> String {
        let keys = self.keys(action);
        if keys.is_empty() {
            "—".to_string()
        } else {
            keys.iter()
                .map(|key| key.display())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    pub fn action_for(&self, event: &KeyEvent) -> Option<Action> {
        for &action in Action::ALL {
            if self.keys(action).iter().any(|binding| binding.matches(event)) {
                return Some(action);
            }
        }
        None
    }

    pub fn set(&mut self, action: Action, binding: Binding) {
        self.bindings.insert(action, vec![binding]);
    }

    pub fn reset(&mut self, action: Action) {
        self.bindings
            .insert(action, parse_all(action.default_keys()));
    }

    pub fn conflict(&self, action: Action, binding: Binding) -> Option<Action> {
        for &other in Action::ALL {
            if other == action {
                continue;
            }
            if self.keys(other).iter().any(|existing| *existing == binding) {
                return Some(other);
            }
        }
        None
    }
}

fn parse_all(keys: &[&str]) -> Vec<Binding> {
    keys.iter().filter_map(|key| Binding::parse(key)).collect()
}

fn single_key(raw: &HashMap<String, Vec<String>>, action: &str, key: &str) -> bool {
    raw.get(action)
        .is_some_and(|keys| keys.len() == 1 && keys[0] == key)
}

pub struct Section {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

pub fn sections(keymap: &Keymap) -> Vec<Section> {
    let mut sections = Vec::new();
    let groups: &[(&'static str, &[Action])] = &[
        (
            "Playback",
            &[
                Action::PlayPause,
                Action::Play,
                Action::Stop,
                Action::Next,
                Action::Prev,
                Action::NextAlbum,
                Action::PrevAlbum,
                Action::SeekBack,
                Action::SeekForward,
                Action::Shuffle,
                Action::Repeat,
                Action::RepeatCurrent,
                Action::Continue,
                Action::PlayLibrary,
                Action::Follow,
                Action::Remaining,
            ],
        ),
        (
            "Views and search",
            &[
                Action::ViewLibrary,
                Action::ViewPlaylist,
                Action::ViewBrowser,
                Action::ViewSettings,
                Action::ToggleLyrics,
                Action::Command,
                Action::Search,
                Action::SearchNext,
                Action::SearchPrev,
                Action::Rescan,
                Action::Info,
            ],
        ),
        (
            "Lists",
            &[
                Action::AddToLibrary,
                Action::AddToPlaylist,
                Action::Remove,
                Action::MoveUp,
                Action::MoveDown,
            ],
        ),
        (
            "Special",
            &[
                Action::PlaylistBuilder,
                Action::DeleteFile,
                Action::SaveLyrics,
                Action::Quit,
            ],
        ),
    ];
    for (title, actions) in groups {
        sections.push(Section {
            title: (*title).to_string(),
            rows: actions
                .iter()
                .map(|action| (keymap.display(*action), action.label().to_string()))
                .collect(),
        });
    }

    sections.push(Section {
        title: "Navigation (fixed)".to_string(),
        rows: vec![
            ("Tab".into(), "column / next view".into()),
            ("Enter".into(), "play / open / edit".into()),
            ("Space".into(), "mark track / pause in lyrics".into()),
            ("↑ / ↓, j / k".into(), "up / down".into()),
            ("PgUp / PgDn".into(), "page up / down".into()),
            ("Home / End, g / G".into(), "first / last".into()),
            ("Backspace".into(), "up in browser".into()),
            ("Esc".into(), "close / cancel".into()),
        ],
    });
    sections.push(Section {
        title: format!(
            "Playlist builder ({})",
            keymap.display(Action::PlaylistBuilder)
        ),
        rows: vec![
            ("Tab".into(), "column: name / left / right".into()),
            ("Enter".into(), "add / remove track".into()),
            ("Shift+↑ / Shift+↓".into(), "reorder in playlist".into()),
            ("Ctrl+S".into(), "save .m3u".into()),
            ("Esc".into(), "close".into()),
        ],
    });
    sections.push(Section {
        title: "Lyrics".to_string(),
        rows: vec![
            ("↑ / ↓, j / k".into(), "scroll unsynced lyrics".into()),
            (
                keymap.display(Action::SaveLyrics),
                "save .lrc next to the track".into(),
            ),
            ("< / >".into(), "offset by 0.25 s".into()),
            ("0".into(), "reset offset".into()),
            ("r".into(), "refetch from lrclib".into()),
            ("Shift+L".into(), "hide text, keep cava".into()),
            (
                format!("{} / Esc", keymap.display(Action::ToggleLyrics)),
                "close lyrics".into(),
            ),
        ],
    });
    sections.push(Section {
        title: "Help".to_string(),
        rows: vec![
            ("F1 / ?".into(), "this table".into()),
            (":help".into(), "this table".into()),
            ("tmus help".into(), "table in the terminal".into()),
        ],
    });
    sections
}

pub struct HelpRow {
    pub header: bool,
    pub key: String,
    pub label: String,
}

pub fn rows(keymap: &Keymap) -> Vec<HelpRow> {
    let mut rows = Vec::new();
    for section in sections(keymap) {
        rows.push(HelpRow {
            header: true,
            key: String::new(),
            label: section.title.to_string(),
        });
        for (key, label) in section.rows {
            rows.push(HelpRow {
                header: false,
                key,
                label,
            });
        }
        rows.push(HelpRow {
            header: false,
            key: String::new(),
            label: String::new(),
        });
    }
    rows
}

pub fn print_table(keymap: &Keymap) {
    println!("tmus — keys\n");
    for section in sections(keymap) {
        println!("{}", section.title);
        for (keys, label) in section.rows {
            println!("  {:<18} {}", keys, label);
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_rows_follow_the_current_keymap() {
        let mut keymap = Keymap::new();
        keymap.set(Action::PlayPause, Binding::parse("p").unwrap());
        keymap.set(Action::SaveLyrics, Binding::parse("ctrl+w").unwrap());
        keymap.set(Action::PlaylistBuilder, Binding::parse("ctrl+b").unwrap());

        let rows = rows(&keymap);
        assert!(rows
            .iter()
            .any(|row| row.key == "p" && row.label == "Pause / resume"));
        assert!(rows
            .iter()
            .any(|row| row.key == "Ctrl+w" && row.label.contains("save .lrc")));
        assert!(rows
            .iter()
            .any(|row| row.header && row.label == "Playlist builder (Ctrl+b)"));
        assert!(!rows.iter().any(|row| row.key == "c"));
    }

    #[test]
    fn legacy_tab_numbers_are_replaced_by_current_defaults() {
        let raw: HashMap<String, Vec<String>> = [
            ("view_library", vec!["1".to_string()]),
            ("view_playlist", vec!["3".to_string()]),
            ("view_browser", vec!["5".to_string()]),
            ("view_settings", vec!["7".to_string()]),
            ("quit", vec!["x".to_string()]),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();

        let keymap = Keymap::from_persisted(&raw);
        assert_eq!(keymap.display(Action::ViewLibrary), "1");
        assert_eq!(keymap.display(Action::ViewPlaylist), "2");
        assert_eq!(keymap.display(Action::ViewBrowser), "3");
        assert_eq!(keymap.display(Action::ViewSettings), "4");
        // Unrelated custom bindings survive the migration.
        assert_eq!(keymap.display(Action::Quit), "x");
    }
}
