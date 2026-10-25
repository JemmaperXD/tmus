use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::banner;
use crate::cava::Cava;
use crate::keys::{self, Action, Binding, Keymap};
use crate::library::{self, Persisted, Track};
use crate::lyrics::{self, CurrentLyric, Lyrics};
use crate::mpris::{self, Command as MprisCommand, Mpris, Snapshot};
use crate::mpv::{Mpv, MpvEvent};
use crate::tags::{self, TagData};
use crate::util;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Library,
    Playlist,
    Browser,
    Settings,
}

impl View {
    pub const ALL: [View; 4] = [
        View::Library,
        View::Playlist,
        View::Browser,
        View::Settings,
    ];

    pub fn idx(self) -> usize {
        match self {
            View::Library => 0,
            View::Playlist => 1,
            View::Browser => 2,
            View::Settings => 3,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            View::Library => "Library",
            View::Playlist => "Playlist",
            View::Browser => "Browser",
            View::Settings => "Settings",
        }
    }

    pub fn action(self) -> Action {
        match self {
            View::Library => Action::ViewLibrary,
            View::Playlist => Action::ViewPlaylist,
            View::Browser => Action::ViewBrowser,
            View::Settings => Action::ViewSettings,
        }
    }

    /// Shorter label for terminals where the full tab row does not fit.
    pub fn short_title(self) -> &'static str {
        match self {
            View::Library => "Lib",
            View::Playlist => "List",
            View::Browser => "Files",
            View::Settings => "Setup",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Album,
    Track,
    Dir,
    File,
    Parent,
    Setting,
}

/// Library pane that owns the keyboard.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LibraryFocus {
    Artists,
    Tracks,
}

#[derive(Clone, Default)]
pub struct LibraryArtist {
    pub name: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone)]
pub struct Row {
    pub indent: usize,
    pub text: String,
    pub kind: RowKind,
    pub track: Option<Track>,
    pub group: Option<Vec<Track>>,
    pub path: Option<PathBuf>,
    pub setting: Option<usize>,
}

impl Row {
    fn album(name: String, year: Option<i32>, group: Vec<Track>) -> Row {
        let text = match year {
            Some(y) => format!("{name}  ({y})"),
            None => name,
        };
        Row {
            indent: 1,
            text,
            kind: RowKind::Album,
            track: None,
            group: Some(group),
            path: None,
            setting: None,
        }
    }

    fn track(track: Track, indent: usize, group: Option<Vec<Track>>) -> Row {
        Row {
            text: track.list_text(),
            kind: RowKind::Track,
            track: Some(track),
            group,
            indent,
            path: None,
            setting: None,
        }
    }

    fn numbered(track: Track, index: usize) -> Row {
        Row {
            text: format!("{index:>3}. {}", track.label()),
            kind: RowKind::Track,
            track: Some(track),
            group: None,
            indent: 0,
            path: None,
            setting: None,
        }
    }

    fn dir(path: PathBuf, name: String) -> Row {
        Row {
            indent: 0,
            text: format!("{name}/"),
            kind: RowKind::Dir,
            track: None,
            group: None,
            path: Some(path),
            setting: None,
        }
    }

    fn file(path: PathBuf, name: String) -> Row {
        Row {
            indent: 0,
            text: name,
            kind: RowKind::File,
            track: None,
            group: None,
            path: Some(path),
            setting: None,
        }
    }

    fn parent() -> Row {
        Row {
            indent: 0,
            text: "..".to_string(),
            kind: RowKind::Parent,
            track: None,
            group: None,
            path: None,
            setting: None,
        }
    }

    fn setting(index: usize, name: &str, value: &str) -> Row {
        Row {
            indent: 0,
            text: format!("{name:<28} {value}"),
            kind: RowKind::Setting,
            track: None,
            group: None,
            path: None,
            setting: Some(index),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuilderFocus {
    Name,
    Left,
    Right,
}

pub struct PlaylistBuilder {
    pub name: String,
    pub left: Vec<Track>,
    pub right: Vec<Track>,
    pub left_state: ListState,
    pub right_state: ListState,
    pub focus: BuilderFocus,
}

pub struct KeymapEditor {
    pub state: ListState,
    pub capture: bool,
    pub message: String,
}

impl KeymapEditor {
    pub fn selected_action(&self) -> Action {
        let index = self.state.selected().unwrap_or(0);
        Action::ALL[index.min(Action::ALL.len() - 1)]
    }
}

pub struct HelpOverlay {
    pub state: ListState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TagField {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Year,
    Track,
    Genre,
    Lyrics,
}

impl TagField {
    pub const ALL: [TagField; 8] = [
        TagField::Title,
        TagField::Artist,
        TagField::Album,
        TagField::AlbumArtist,
        TagField::Year,
        TagField::Track,
        TagField::Genre,
        TagField::Lyrics,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TagField::Title => "Title",
            TagField::Artist => "Artist",
            TagField::Album => "Album",
            TagField::AlbumArtist => "Album artist",
            TagField::Year => "Year",
            TagField::Track => "Track number",
            TagField::Genre => "Genre",
            TagField::Lyrics => "Lyrics",
        }
    }
}

pub struct TagEditor {
    pub path: PathBuf,
    pub track: String,
    pub data: TagData,
    pub field: usize,
    pub cursor: usize,
    pub lyrics_mode: bool,
    pub lyrics_cursor: usize,
    pub lyrics_scroll: usize,
    pub lyrics_hscroll: usize,
    pub message: String,
    pub dirty: bool,
    pub close_armed: bool,
}

impl TagEditor {
    pub fn selected(&self) -> TagField {
        TagField::ALL[self.field.min(TagField::ALL.len() - 1)]
    }

    pub fn value(&self, field: TagField) -> &str {
        match field {
            TagField::Title => &self.data.title,
            TagField::Artist => &self.data.artist,
            TagField::Album => &self.data.album,
            TagField::AlbumArtist => &self.data.album_artist,
            TagField::Year => &self.data.year,
            TagField::Track => &self.data.track,
            TagField::Genre => &self.data.genre,
            TagField::Lyrics => &self.data.lyrics,
        }
    }

    fn value_mut(&mut self) -> Option<&mut String> {
        match self.selected() {
            TagField::Title => Some(&mut self.data.title),
            TagField::Artist => Some(&mut self.data.artist),
            TagField::Album => Some(&mut self.data.album),
            TagField::AlbumArtist => Some(&mut self.data.album_artist),
            TagField::Year => Some(&mut self.data.year),
            TagField::Track => Some(&mut self.data.track),
            TagField::Genre => Some(&mut self.data.genre),
            TagField::Lyrics => None,
        }
    }

    fn select_field(&mut self, index: usize) {
        self.field = index.min(TagField::ALL.len() - 1);
        self.cursor = self.value(self.selected()).len();
        self.close_armed = false;
    }

    fn move_field(&mut self, delta: isize) {
        let len = TagField::ALL.len() as isize;
        let next = (self.field as isize + delta).clamp(0, len - 1);
        self.select_field(next as usize);
    }

    fn edit_text(&mut self, key: KeyEvent) -> bool {
        let mut cursor = self.cursor;
        let Some(value) = self.value_mut() else {
            return false;
        };
        match key.code {
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                value.insert(cursor, ch);
                cursor += ch.len_utf8();
            }
            KeyCode::Backspace => {
                if cursor > 0 {
                    let previous = prev_boundary(value, cursor);
                    value.replace_range(previous..cursor, "");
                    cursor = previous;
                }
            }
            KeyCode::Delete => {
                if cursor < value.len() {
                    let next = next_boundary(value, cursor);
                    value.replace_range(cursor..next, "");
                }
            }
            KeyCode::Left => cursor = prev_boundary(value, cursor),
            KeyCode::Right => cursor = next_boundary(value, cursor),
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = value.len(),
            _ => return false,
        }
        self.cursor = cursor;
        true
    }

    fn edit_lyrics(&mut self, key: KeyEvent) -> bool {
        let value = &mut self.data.lyrics;
        match key.code {
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                value.insert(self.lyrics_cursor, ch);
                self.lyrics_cursor += ch.len_utf8();
            }
            KeyCode::Enter => {
                value.insert(self.lyrics_cursor, '\n');
                self.lyrics_cursor += 1;
            }
            KeyCode::Backspace => {
                if self.lyrics_cursor > 0 {
                    let previous = prev_boundary(value, self.lyrics_cursor);
                    value.replace_range(previous..self.lyrics_cursor, "");
                    self.lyrics_cursor = previous;
                }
            }
            KeyCode::Delete => {
                if self.lyrics_cursor < value.len() {
                    let next = next_boundary(value, self.lyrics_cursor);
                    value.replace_range(self.lyrics_cursor..next, "");
                }
            }
            KeyCode::Left => self.lyrics_cursor = prev_boundary(value, self.lyrics_cursor),
            KeyCode::Right => self.lyrics_cursor = next_boundary(value, self.lyrics_cursor),
            KeyCode::Home => self.lyrics_cursor = line_start(value, self.lyrics_cursor),
            KeyCode::End => self.lyrics_cursor = line_end(value, self.lyrics_cursor),
            KeyCode::Up => {
                let (line, column) = line_column(value, self.lyrics_cursor);
                if line > 0 {
                    self.lyrics_cursor = cursor_at(value, line - 1, column);
                }
            }
            KeyCode::Down => {
                let (line, column) = line_column(value, self.lyrics_cursor);
                if line < value.matches('\n').count() {
                    self.lyrics_cursor = cursor_at(value, line + 1, column);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn lyrics_position(&self) -> (usize, usize) {
        line_column(&self.data.lyrics, self.lyrics_cursor)
    }

    pub fn lyrics_line_count(&self) -> usize {
        self.data.lyrics.matches('\n').count() + 1
    }
}

fn prev_boundary(value: &str, index: usize) -> usize {
    value[..index]
        .char_indices()
        .next_back()
        .map(|(at, _)| at)
        .unwrap_or(0)
}

fn next_boundary(value: &str, index: usize) -> usize {
    value[index..]
        .chars()
        .next()
        .map(|ch| index + ch.len_utf8())
        .unwrap_or(value.len())
}

fn line_start(value: &str, index: usize) -> usize {
    value[..index].rfind('\n').map(|at| at + 1).unwrap_or(0)
}

fn line_end(value: &str, index: usize) -> usize {
    value[index..]
        .find('\n')
        .map(|at| index + at)
        .unwrap_or(value.len())
}

fn line_column(value: &str, index: usize) -> (usize, usize) {
    let before = &value[..index];
    let line = before.matches('\n').count();
    let column = before
        .rsplit('\n')
        .next()
        .map(|line| line.chars().count())
        .unwrap_or(0);
    (line, column)
}

fn cursor_at(value: &str, line: usize, column: usize) -> usize {
    let mut start = 0;
    for _ in 0..line {
        let Some(at) = value[start..].find('\n') else {
            return value.len();
        };
        start += at + 1;
    }
    let end = value[start..]
        .find('\n')
        .map(|at| start + at)
        .unwrap_or(value.len());
    let mut index = start;
    for _ in 0..column {
        let Some(ch) = value[index..end].chars().next() else {
            break;
        };
        index += ch.len_utf8();
    }
    index
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Input {
    None,
    Command,
    Search,
    Quit,
    DeleteFile,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LyricState {
    Idle,
    Loading,
    Ready,
    Missing,
}

pub struct App {
    pub view: View,
    pub states: [ListState; 4],
    pub rows: Vec<Row>,
    pub list_height: usize,
    pub library_artists: Vec<LibraryArtist>,
    pub library_artist_state: ListState,
    pub library_focus: LibraryFocus,

    pub lib: Vec<Track>,
    pub roots: Vec<PathBuf>,
    pub playlist: Vec<Track>,
    pub context: Vec<Track>,
    pub current: Option<Track>,
    last_playlist: Option<PathBuf>,

    pub mpv: Option<Mpv>,
    mpv_rx: Option<Receiver<Result<Mpv, String>>>,
    mpv_stop: Option<Arc<AtomicBool>>,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub repeat: bool,
    pub shuffle: bool,
    pub follow: bool,
    pub remaining: bool,
    pub continue_play: bool,
    pub repeat_current: bool,
    pub aaa_mode: bool,
    pub play_library: bool,

    pub marks: HashSet<PathBuf>,
    pub show_lyrics: bool,
    pub lyrics_hidden: bool,
    pub lyrics: Option<Lyrics>,
    pub lyric_state: LyricState,
    pub lyric_error: String,
    lyric_rx: Option<Receiver<Result<Lyrics, String>>>,
    lyric_started: Instant,
    lyric_retry_at: Option<Instant>,
    lyric_force: bool,
    pub offset: f64,
    pub plain_index: usize,

    pub builder: Option<PlaylistBuilder>,
    pub keymap: Keymap,
    pub keymap_editor: Option<KeymapEditor>,
    pub help: Option<HelpOverlay>,
    pub tag_editor: Option<TagEditor>,
    pub pending_delete: Option<PathBuf>,

    pub cava: Option<Cava>,
    pub cava_enabled: bool,
    pub cava_error: String,
    pub mpris: Option<Mpris>,
    pub mpris_error: String,
    mpris_retry_at: Option<Instant>,

    pub input: Input,
    pub buf: String,
    pub history: Vec<String>,
    hist_pos: usize,
    pub message: String,
    message_at: Instant,

    pub search: Option<String>,
    pub browser_dir: PathBuf,
    pub browser_hidden: bool,

    pub scanning: bool,
    scan_rx: Option<Receiver<Vec<Track>>>,
    scan_sync: bool,
    scan_paths: Vec<PathBuf>,
    pub quit: bool,
}

static SIGNAL_QUIT: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Reap mpv on SIGTERM/SIGHUP.
fn install_signal_handler() {
    let flag = SIGNAL_QUIT.get_or_init(|| Arc::new(AtomicBool::new(false)));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        let _ = signal_hook::flag::register(signal, flag.clone());
    }
}

fn signal_quit() -> bool {
    SIGNAL_QUIT
        .get()
        .map(|flag| flag.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn spawn_event_reader() -> Receiver<Event> {
    let (tx, rx) = channel();
    thread::spawn(move || loop {
        match ratatui::crossterm::event::read() {
            Ok(event) => {
                if tx.send(event).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    });
    rx
}

/// crossterm's reader can get stuck on a pty that the terminal emulator has
/// already closed. Watch the descriptors ourselves and stop the UI.
fn watch_tty() -> Arc<AtomicBool> {
    let lost = Arc::new(AtomicBool::new(false));
    let flag = lost.clone();
    let stdin_is_tty = std::io::stdin().is_terminal();
    thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(200));
            let gone = !std::io::stdout().is_terminal()
                || (stdin_is_tty && !std::io::stdin().is_terminal());
            if gone {
                flag.store(true, Ordering::Relaxed);
                break;
            }
        }
    });
    lost
}

impl App {
    pub fn new(args: Vec<String>) -> App {
        install_signal_handler();
        let persisted = library::load_state();
        let lib = library::load_library();
        let mut playlist = Vec::new();
        for path in &persisted.playlist {
            if let Some(track) = lib.iter().find(|t| &t.path == path) {
                playlist.push(track.clone());
            } else {
                playlist.push(library::read_track(path));
            }
        }

        let (mpris, mpris_error, mpris_retry_at) = match Mpris::spawn() {
            Ok(mpris) => (Some(mpris), String::new(), None),
            Err(error) => (
                None,
                error,
                Some(Instant::now() + Duration::from_secs(5)),
            ),
        };
        let keymap = Keymap::from_persisted(&persisted.keybindings);

        let mut app = App {
            view: View::Library,
            states: std::array::from_fn(|_| ListState::default()),
            rows: Vec::new(),
            list_height: 20,
            library_artists: Vec::new(),
            library_artist_state: ListState::default(),
            library_focus: LibraryFocus::Artists,
            lib,
            roots: persisted.roots.clone(),
            playlist,
            context: Vec::new(),
            current: None,
            last_playlist: None,
            mpv: None,
            mpv_rx: None,
            mpv_stop: None,
            position: 0.0,
            duration: 0.0,
            paused: false,
            repeat: persisted.repeat,
            shuffle: persisted.shuffle,
            follow: persisted.follow,
            remaining: persisted.show_remaining,
            continue_play: persisted.continue_play,
            repeat_current: persisted.repeat_current,
            aaa_mode: persisted.aaa_mode,
            play_library: persisted.play_library,
            marks: HashSet::new(),
            show_lyrics: false,
            lyrics_hidden: false,
            lyrics: None,
            lyric_state: LyricState::Idle,
            lyric_error: String::new(),
            lyric_rx: None,
            lyric_started: Instant::now(),
            lyric_retry_at: None,
            lyric_force: false,
            offset: persisted.lyrics_offset,
            plain_index: 0,
            builder: None,
            keymap,
            keymap_editor: None,
            help: None,
            tag_editor: None,
            pending_delete: None,
            cava: None,
            cava_enabled: persisted.cava,
            cava_error: String::new(),
            mpris,
            mpris_error,
            mpris_retry_at,
            input: Input::None,
            buf: String::new(),
            history: Vec::new(),
            hist_pos: 0,
            message: String::new(),
            message_at: Instant::now(),
            search: None,
            browser_dir: util::home(),
            browser_hidden: false,
            scanning: false,
            scan_rx: None,
            scan_sync: false,
            scan_paths: Vec::new(),
            quit: false,
        };

        if app.repeat && app.repeat_current {
            app.repeat_current = false;
        }

        for raw in args {
            app.add_path(&raw, None);
        }

        if app.roots.is_empty() {
            for candidate in ["Music", "music"] {
                let dir = util::home().join(candidate);
                if dir.is_dir() {
                    app.roots.push(dir);
                    break;
                }
            }
        }
        if !app.roots.is_empty() {
            app.start_scan(app.roots.clone(), true);
        }

        // mpv needs a moment to open its IPC socket; don't block the UI on it.
        app.ensure_mpv();

        app.refresh_rows();
        app
    }

    pub fn run(&mut self) -> std::io::Result<()> {
        let mut terminal = ratatui::init();
        let result = self.event_loop(&mut terminal);
        ratatui::restore();
        self.shutdown();
        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> std::io::Result<()> {
        let events = spawn_event_reader();
        let tty_lost = watch_tty();

        loop {
            if tty_lost.load(Ordering::Relaxed) {
                break;
            }
            if let Err(error) = terminal.draw(|frame| crate::ui::draw(frame, self)) {
                if tty_lost.load(Ordering::Relaxed) {
                    break;
                }
                return Err(error);
            }
            let timeout = if self.show_lyrics {
                Duration::from_millis(33)
            } else {
                Duration::from_millis(100)
            };
            match events.recv_timeout(timeout) {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => self.handle_key(key),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
            if self.quit {
                break;
            }
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        self.save_state();
        // Cancel a pending start and reap mpv.
        if let Some(stop) = self.mpv_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(rx) = self.mpv_rx.take() {
            if let Ok(Ok(mpv)) = rx.recv_timeout(Duration::from_secs(2)) {
                drop(mpv);
            }
        }
        self.mpv.take();
    }

    pub fn save_state(&self) {
        library::save_library(&self.lib);
        let state = Persisted {
            roots: self.roots.clone(),
            playlist: self.playlist.iter().map(|t| t.path.clone()).collect(),
            repeat: self.repeat,
            shuffle: self.shuffle,
            follow: self.follow,
            continue_play: self.continue_play,
            repeat_current: self.repeat_current,
            aaa_mode: self.aaa_mode,
            play_library: self.play_library,
            show_remaining: self.remaining,
            cava: self.cava_enabled,
            lyrics_offset: self.offset,
            keybindings: self.keymap.to_persisted(),
        };
        library::save_state(&state);
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.message_at = Instant::now();
    }

    pub fn selected_index(&self) -> usize {
        self.states[self.view.idx()].selected().unwrap_or(0)
    }

    pub fn selected_artist_index(&self) -> usize {
        self.library_artist_state
            .selected()
            .unwrap_or(0)
            .min(self.library_artists.len().saturating_sub(1))
    }

    fn row_is_selectable(row: &Row) -> bool {
        !matches!(row.kind, RowKind::Album)
    }

    fn selectable_index(rows: &[Row], index: usize, forward: bool) -> Option<usize> {
        if rows.is_empty() {
            return None;
        }
        let index = index.min(rows.len() - 1);
        if Self::row_is_selectable(&rows[index]) {
            return Some(index);
        }
        if forward {
            (index + 1..rows.len()).find(|&i| Self::row_is_selectable(&rows[i]))
                .or_else(|| (0..index).rev().find(|&i| Self::row_is_selectable(&rows[i])))
        } else {
            (0..index).rev().find(|&i| Self::row_is_selectable(&rows[i]))
                .or_else(|| (index + 1..rows.len()).find(|&i| Self::row_is_selectable(&rows[i])))
        }
    }

    pub fn set_selected(&mut self, index: usize) {
        if self.rows.is_empty() {
            return;
        }
        let index = index.min(self.rows.len() - 1);
        if let Some(index) = Self::selectable_index(&self.rows, index, true) {
            self.states[self.view.idx()].select(Some(index));
        }
    }

    pub fn set_artist_selected(&mut self, index: usize) {
        if self.library_artists.is_empty() {
            self.library_artist_state.select(None);
            return;
        }
        let index = index.min(self.library_artists.len() - 1);
        let changed = self.selected_artist_index() != index;
        self.library_artist_state.select(Some(index));
        if changed {
            self.refresh_rows();
            if let Some(first) = Self::selectable_index(&self.rows, 0, true) {
                self.states[View::Library.idx()].select(Some(first));
            }
        }
    }

    /// Rebuild the artist column.
    fn rebuild_library_artists(&mut self) {
        let mut sorted: Vec<Track> = self.lib.clone();
        sorted.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));

        let mut artists: Vec<LibraryArtist> = Vec::new();
        for track in sorted {
            let key = track.artist_or().to_lowercase();
            match artists.last_mut() {
                Some(artist) if artist.name.to_lowercase() == key => {
                    artist.tracks.push(track);
                }
                _ => artists.push(LibraryArtist {
                    name: track.artist_or().to_string(),
                    tracks: vec![track],
                }),
            }
        }
        self.library_artists = artists;
        if self.library_artists.is_empty() {
            self.library_artist_state.select(None);
        } else {
            let selected = self.selected_artist_index();
            self.library_artist_state.select(Some(selected));
        }
    }

    /// Track column: an album separator followed by its tracks.
    fn library_track_rows(&self) -> Vec<Row> {
        let Some(artist) = self.library_artists.get(self.selected_artist_index()) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut i = 0;
        while i < artist.tracks.len() {
            let key = (
                artist.tracks[i].year.unwrap_or(0),
                artist.tracks[i].album_or().to_lowercase(),
            );
            let name = artist.tracks[i].album_or().to_string();
            let year = artist.tracks[i].year;
            let start = i;
            while i < artist.tracks.len()
                && (
                    artist.tracks[i].year.unwrap_or(0),
                    artist.tracks[i].album_or().to_lowercase(),
                ) == key
            {
                i += 1;
            }
            let album = artist.tracks[start..i].to_vec();
            rows.push(Row::album(name, year, album.clone()));
            for track in &album {
                rows.push(Row::track(track.clone(), 0, Some(album.clone())));
            }
        }
        rows
    }

    pub fn refresh_rows(&mut self) {
        if self.view == View::Library {
            self.rebuild_library_artists();
        }
        self.rows = match self.view {
            View::Library => self.library_track_rows(),
            View::Playlist => self
                .playlist
                .iter()
                .enumerate()
                .map(|(i, t)| Row::numbered(t.clone(), i + 1))
                .collect(),
            View::Browser => self.browser_rows(),
            View::Settings => self.settings_rows(),
        };
        if self.rows.is_empty() {
            self.states[self.view.idx()].select(None);
        } else {
            let selected = self.selected_index().min(self.rows.len() - 1);
            if let Some(selected) = Self::selectable_index(&self.rows, selected, true) {
                self.states[self.view.idx()].select(Some(selected));
            }
        }
    }

    fn setting_value(&self, index: usize) -> String {
        let onoff = |value: bool| if value { "on" } else { "off" };
        match index {
            0 => onoff(self.remaining).to_string(),
            1 => onoff(self.cava_enabled).to_string(),
            2 => format!("{:+.2} s", self.offset),
            3 => format!("{} actions", Action::ALL.len()),
            _ => String::new(),
        }
    }

    fn settings_rows(&self) -> Vec<Row> {
        const NAMES: [&str; 4] = [
            "Remaining time",
            "Cava in lyrics",
            "Lyrics offset",
            "Keys",
        ];
        NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| Row::setting(index, name, &self.setting_value(index)))
            .collect()
    }

    fn browser_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.browser_dir.parent().is_some() {
            rows.push(Row::parent());
        }
        let Ok(entries) = std::fs::read_dir(&self.browser_dir) else {
            return rows;
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !self.browser_hidden && name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                dirs.push((name, entry.path()));
            } else if kind.is_file() && library::is_audio(&entry.path()) {
                files.push((name, entry.path()));
            }
        }
        dirs.sort_by_key(|(name, _)| name.to_lowercase());
        files.sort_by_key(|(name, _)| name.to_lowercase());
        for (name, path) in dirs {
            rows.push(Row::dir(path, name));
        }
        for (name, path) in files {
            rows.push(Row::file(path, name));
        }
        rows
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.input == Input::DeleteFile {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c'))
            {
                self.input = Input::None;
                self.pending_delete = None;
                self.set_message("deletion cancelled");
            } else {
                self.handle_input(key);
            }
            return;
        }

        if self.builder.is_some() {
            self.handle_builder_key(key);
            return;
        }

        if self.keymap_editor.is_some() {
            self.handle_keymap_editor_key(key);
            return;
        }

        if self.help.is_some() {
            self.handle_help_key(key);
            return;
        }

        if self.tag_editor.is_some() {
            self.handle_tag_editor_key(key);
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c'))
        {
            if self.input != Input::None {
                self.input = Input::None;
                self.buf.clear();
            } else {
                self.request_quit();
            }
            return;
        }

        if self.input != Input::None {
            self.handle_input(key);
            return;
        }

        // Shift+L hides the text but keeps the cava background and the bars.
        // It is checked before the keymap because terminals differ on whether
        // Shift+L arrives as `L` or as `l` with the SHIFT modifier.
        let hide_text = key.code == KeyCode::Char('L')
            || (key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::SHIFT));
        if self.show_lyrics && hide_text {
            self.toggle_lyrics_text();
            return;
        }

        if let Some(action) = self.keymap.action_for(&key) {
            if !self.show_lyrics || action.works_in_lyrics() {
                self.dispatch_action(action);
                return;
            }
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => {
                    let step = (self.list_height / 2).max(1) as isize;
                    self.move_selection(step);
                    return;
                }
                KeyCode::Char('u') => {
                    let step = (self.list_height / 2).max(1) as isize;
                    self.move_selection(-step);
                    return;
                }
                KeyCode::Char('f') => {
                    let step = self.list_height.max(1) as isize;
                    self.move_selection(step);
                    return;
                }
                KeyCode::Char('b') => {
                    let step = self.list_height.max(1) as isize;
                    self.move_selection(-step);
                    return;
                }
                KeyCode::Char('y') => {
                    self.move_selection(-1);
                    return;
                }
                KeyCode::Char('e') => {
                    self.move_selection(1);
                    return;
                }
                KeyCode::Char('l') => {
                    self.refresh_rows();
                    self.set_message("list refreshed");
                    return;
                }
                _ => {}
            }
        }

        if matches!(key.code, KeyCode::F(1)) || matches!(key.code, KeyCode::Char('?')) {
            self.open_help();
            return;
        }

        if self.show_lyrics {
            self.handle_lyrics_key(key);
            return;
        }

        match key.code {
            KeyCode::Tab => {
                if self.view == View::Library {
                    self.library_focus = match self.library_focus {
                        LibraryFocus::Artists => LibraryFocus::Tracks,
                        LibraryFocus::Tracks => LibraryFocus::Artists,
                    };
                    self.set_message(match self.library_focus {
                        LibraryFocus::Artists => "artist column",
                        LibraryFocus::Tracks => "track column",
                    });
                } else {
                    let next = (self.view.idx() + 1) % View::ALL.len();
                    self.switch_view(View::ALL[next]);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.select_edge(false),
            KeyCode::Char('G') | KeyCode::End => self.select_edge(true),
            KeyCode::PageDown => {
                let step = self.list_height.max(1) as isize;
                self.move_selection(step);
            }
            KeyCode::PageUp => {
                let step = self.list_height.max(1) as isize;
                self.move_selection(-step);
            }
            KeyCode::Enter => self.activate(),
            KeyCode::Char(',') => self.seek(-60.0),
            KeyCode::Char('.') => self.seek(60.0),
            KeyCode::Char(' ') => match self.view {
                View::Browser => self.activate(),
                View::Settings => self.activate(),
                _ => self.toggle_mark(),
            },
            KeyCode::Char('u') => {
                if self.view == View::Browser {
                    self.refresh_rows();
                    self.set_message("list refreshed");
                } else {
                    self.rescan();
                }
            }
            KeyCode::Char('i') => {
                if self.view == View::Browser {
                    self.browser_hidden = !self.browser_hidden;
                    self.refresh_rows();
                } else {
                    self.select_current();
                }
            }
            KeyCode::Char('!') => {
                self.input = Input::Command;
                self.buf = "shell ".to_string();
                self.hist_pos = self.history.len();
            }
            KeyCode::Backspace if self.view == View::Browser => self.browser_up(),
            KeyCode::Char('<') => {
                self.offset -= 0.25;
                self.set_message(format!("lyrics offset: {:+.2} s", self.offset));
            }
            KeyCode::Char('>') => {
                self.offset += 0.25;
                self.set_message(format!("lyrics offset: {:+.2} s", self.offset));
            }
            KeyCode::Char('0') => {
                self.offset = 0.0;
                self.set_message("lyrics offset reset");
            }
            _ => {}
        }
    }

    fn dispatch_action(&mut self, action: Action) {
        use Action::*;
        match action {
            Quit => self.request_quit(),
            Command => {
                self.input = Input::Command;
                self.buf.clear();
                self.hist_pos = self.history.len();
            }
            Search => {
                self.input = Input::Search;
                self.buf.clear();
            }
            SearchNext => self.search_next(true),
            SearchPrev => self.search_next(false),
            ToggleLyrics => self.toggle_lyrics(),
            PlayPause => self.toggle_pause(),
            Play => self.play_selected_or_resume(),
            Stop => self.stop(),
            Next => self.next(),
            Prev => self.prev(),
            NextAlbum => self.next_album(),
            PrevAlbum => self.prev_album(),
            SeekBack => self.seek(-5.0),
            SeekForward => self.seek(5.0),
            Shuffle => {
                self.shuffle = !self.shuffle;
                self.set_message(if self.shuffle {
                    "shuffle on"
                } else {
                    "shuffle off"
                });
            }
            Repeat => self.toggle_repeat(),
            RepeatCurrent => self.toggle_repeat_current(),
            Continue => {
                self.continue_play = !self.continue_play;
                self.set_message(if self.continue_play {
                    "continue on"
                } else {
                    "continue off"
                });
            }
            PlayLibrary => {
                self.play_library = !self.play_library;
                self.set_message(if self.play_library {
                    "play from library"
                } else {
                    "normal order"
                });
            }
            Follow => self.follow = !self.follow,
            Remaining => self.remaining = !self.remaining,
            ViewLibrary | ViewPlaylist | ViewBrowser | ViewSettings => {
                let view = match action {
                    ViewLibrary => View::Library,
                    ViewPlaylist => View::Playlist,
                    ViewBrowser => View::Browser,
                    _ => View::Settings,
                };
                self.switch_view(view);
            }
            Rescan => self.rescan(),
            Info => self.echo_selected(),
            AddToLibrary => self.add_selection_to_library(),
            AddToPlaylist => self.add_selection_to_playlist(),
            Remove => self.remove_selection(),
            MoveUp => self.move_selected(false),
            MoveDown => self.move_selected(true),
            PlaylistBuilder => self.open_builder(),
            DeleteFile => self.confirm_delete_file(),
            SaveLyrics => {
                if self.show_lyrics {
                    self.save_lyrics();
                } else {
                    self.set_message("Ctrl+S saves lyrics in lyrics view");
                }
            }
        }
    }

    fn handle_lyrics_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.set_lyrics(false),
            KeyCode::Char(' ') => self.toggle_pause(),
            KeyCode::Char(',') => self.seek(-60.0),
            KeyCode::Char('.') => self.seek(60.0),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_plain(-1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_plain(1),
            KeyCode::Char('r') => self.refetch_lyrics(),
            KeyCode::Char('<') => {
                self.offset -= 0.25;
                self.set_message(format!("lyrics offset: {:+.2} s", self.offset));
            }
            KeyCode::Char('>') => {
                self.offset += 0.25;
                self.set_message(format!("lyrics offset: {:+.2} s", self.offset));
            }
            KeyCode::Char('0') => self.offset = 0.0,
            _ => {}
        }
    }

    fn handle_input(&mut self, key: KeyEvent) {
        if self.input == Input::DeleteFile {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.delete_file(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.input = Input::None;
                    self.pending_delete = None;
                    self.set_message("deletion cancelled");
                }
                _ => {}
            }
            return;
        }

        if self.input == Input::Quit {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.quit = true,
                _ => {
                    self.input = Input::None;
                    self.set_message("quit cancelled");
                }
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                let _mode = self.input;
                self.input = Input::None;
                self.buf.clear();
            }
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.buf);
                let mode = self.input;
                self.input = Input::None;
                match mode {
                    Input::Command => self.run_command(&line),
                    Input::Search => self.run_search(&line, false),
                    _ => {}
                }
            }
            KeyCode::Backspace => {
                self.buf.pop();
            }
            KeyCode::Up if self.input == Input::Command => {
                if !self.history.is_empty() {
                    self.hist_pos = self.hist_pos.saturating_sub(1);
                    self.buf = self.history[self.hist_pos].clone();
                }
            }
            KeyCode::Down if self.input == Input::Command => {
                if self.hist_pos < self.history.len() {
                    self.hist_pos += 1;
                    self.buf = if self.hist_pos == self.history.len() {
                        String::new()
                    } else {
                        self.history[self.hist_pos].clone()
                    };
                }
            }
            KeyCode::Char(ch) => {
                self.buf.push(ch);
            }
            _ => {}
        }
    }

    fn switch_view(&mut self, view: View) {
        self.view = view;
        self.refresh_rows();
    }

    fn select_edge(&mut self, last: bool) {
        if self.view == View::Library && self.library_focus == LibraryFocus::Artists {
            if self.library_artists.is_empty() {
                return;
            }
            let index = if last { self.library_artists.len() - 1 } else { 0 };
            self.set_artist_selected(index);
            return;
        }
        if self.rows.is_empty() {
            return;
        }
        let index = if last { self.rows.len() - 1 } else { 0 };
        self.set_selected(index);
    }

    fn move_selection(&mut self, delta: isize) {
        if self.view == View::Library && self.library_focus == LibraryFocus::Artists {
            if self.library_artists.is_empty() {
                return;
            }
            let current = self.selected_artist_index() as isize;
            let next = (current + delta).clamp(0, self.library_artists.len() as isize - 1);
            self.set_artist_selected(next as usize);
            return;
        }
        if self.rows.is_empty() {
            return;
        }
        let direction = delta.signum();
        let mut remaining = delta.abs();
        let mut index = self.selected_index() as isize;
        while remaining > 0 {
            let next = index + direction;
            if next < 0 || next >= self.rows.len() as isize {
                break;
            }
            index = next;
            if Self::row_is_selectable(&self.rows[index as usize]) {
                remaining -= 1;
            }
        }
        self.set_selected(index as usize);
    }

    fn toggle_mark(&mut self) {
        if self.view == View::Library && self.library_focus == LibraryFocus::Artists {
            // Space moves from the artist column into its tracks.
            self.library_focus = LibraryFocus::Tracks;
            if let Some(first) = Self::selectable_index(&self.rows, 0, true) {
                self.states[View::Library.idx()].select(Some(first));
            }
            return;
        }
        let Some(row) = self.rows.get(self.selected_index()) else {
            return;
        };
        let Some(track) = &row.track else {
            return;
        };
        if !self.marks.remove(&track.path) {
            self.marks.insert(track.path.clone());
        }
        if let Some(next) = self.selected_index().checked_add(1) {
            if next < self.rows.len() {
                self.set_selected(next);
            }
        }
    }

    fn selected_tracks(&self) -> Vec<Track> {
        let marked: Vec<Track> = self
            .rows
            .iter()
            .filter_map(|row| row.track.clone())
            .filter(|track| self.marks.contains(&track.path))
            .collect();
        if !marked.is_empty() {
            return marked;
        }
        if self.view == View::Library && self.library_focus == LibraryFocus::Artists {
            if let Some(artist) = self.library_artists.get(self.selected_artist_index()) {
                return artist.tracks.clone();
            }
            return Vec::new();
        }
        let Some(row) = self.rows.get(self.selected_index()) else {
            return Vec::new();
        };
        if let Some(group) = &row.group {
            return group.clone();
        }
        if let Some(track) = &row.track {
            return vec![track.clone()];
        }
        if row.kind == RowKind::Dir {
            if let Some(path) = &row.path {
                return library::audio_files_in(path, self.browser_hidden);
            }
        }
        Vec::new()
    }

    fn activate(&mut self) {
        if self.view == View::Library && self.library_focus == LibraryFocus::Artists {
            let Some(artist) = self.library_artists.get(self.selected_artist_index()).cloned()
            else {
                return;
            };
            self.context = artist.tracks.clone();
            self.library_focus = LibraryFocus::Tracks;
            if let Some(first) = Self::selectable_index(&self.rows, 0, true) {
                self.states[View::Library.idx()].select(Some(first));
            }
            if let Some(first) = artist.tracks.first() {
                self.play_track(first.clone());
            }
            return;
        }
        let Some(row) = self.rows.get(self.selected_index()).cloned() else {
            return;
        };
        match self.view {
            View::Library => {
                if let Some(track) = &row.track {
                    let track = track.clone();
                    if self.play_library {
                        self.context = self.library_context();
                    } else if self.aaa_mode {
                        let album = track.album_or().to_string();
                        let artist = track.artist_or().to_string();
                        let group: Vec<Track> = self
                            .lib
                            .iter()
                            .filter(|candidate| {
                                candidate.album_or() == album && candidate.artist_or() == artist
                            })
                            .cloned()
                            .collect();
                        self.context = if group.len() > 1 {
                            group
                        } else {
                            row.group.clone().unwrap_or_else(|| vec![track.clone()])
                        };
                    } else if let Some(group) = &row.group {
                        self.context = group.clone();
                    } else {
                        self.context = self.rows.iter().filter_map(|r| r.track.clone()).collect();
                    }
                    self.play_track(track);
                } else if let Some(group) = &row.group {
                    self.context = group.clone();
                    if let Some(first) = group.first() {
                        self.play_track(first.clone());
                    }
                }
            }
            View::Playlist => {
                if let Some(track) = &row.track {
                    self.context = self.playlist.clone();
                    self.play_track(track.clone());
                }
            }
            View::Browser => {
                if row.kind == RowKind::Dir {
                    if let Some(path) = row.path {
                        self.browser_dir = path;
                        self.refresh_rows();
                        self.set_selected(0);
                    }
                } else if row.kind == RowKind::File {
                    if let Some(path) = row.path {
                        let files = library::audio_files_in(&self.browser_dir, self.browser_hidden);
                        let index = files.iter().position(|t| t.path == path).unwrap_or(0);
                        if files.is_empty() {
                            return;
                        }
                        self.context = files;
                        self.play_track(self.context[index].clone());
                    }
                } else if row.kind == RowKind::Parent {
                    self.browser_up();
                }
            }
            View::Settings => {
                if let Some(index) = row.setting {
                    self.activate_setting(index);
                }
            }
        }
    }

    fn library_context(&self) -> Vec<Track> {
        let mut tracks: Vec<Track> = self.lib.clone();
        tracks.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        tracks
    }

    fn activate_setting(&mut self, index: usize) {
        match index {
            0 => self.remaining = !self.remaining,
            1 => {
                self.cava_enabled = !self.cava_enabled;
                if self.cava_enabled && self.show_lyrics {
                    self.start_cava();
                } else if !self.cava_enabled {
                    self.stop_cava();
                }
            }
            2 => {
                self.input = Input::Command;
                self.buf = format!("set offset={:.2}", self.offset);
                self.hist_pos = self.history.len();
            }
            3 => {
                self.open_keymap_editor();
                return;
            }
            _ => {}
        }
        self.refresh_rows();
    }

    fn reset_setting(&mut self, index: usize) {
        match index {
            0 => self.remaining = false,
            1 => {
                self.cava_enabled = true;
                if self.show_lyrics {
                    self.start_cava();
                }
            }
            2 => self.offset = 0.0,
            3 => {
                self.keymap = Keymap::new();
                self.save_state();
                self.set_message("key bindings reset");
                return;
            }
            _ => {}
        }
        self.set_message("value reset");
    }

    fn browser_up(&mut self) {
        if let Some(parent) = self.browser_dir.parent() {
            self.browser_dir = parent.to_path_buf();
            self.refresh_rows();
            self.set_selected(0);
        }
    }

    fn play_selected_or_resume(&mut self) {
        if self.current.is_some() && self.paused {
            self.toggle_pause();
            return;
        }
        if let Some(row) = self.rows.get(self.selected_index()) {
            if row.track.is_some() || row.group.is_some() {
                self.activate();
                return;
            }
        }
        if self.current.is_some() {
            self.toggle_pause();
        }
    }

    /// Restart mpv if it died.
    fn ensure_mpv(&mut self) {
        if self.mpv.is_some() || self.mpv_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        self.mpv_stop = Some(stop);
        thread::spawn(move || {
            let _ = tx.send(Mpv::start(thread_stop));
        });
        self.mpv_rx = Some(rx);
    }

    pub fn play_track(&mut self, track: Track) {
        self.ensure_mpv();
        self.current = Some(track.clone());
        self.position = 0.0;
        self.duration = track.duration;
        self.paused = false;
        self.lyrics = None;
        self.lyric_state = LyricState::Idle;
        self.lyric_error.clear();
        if let Some(mpv) = &mut self.mpv {
            mpv.play(&track.path);
        }
        self.start_lyrics(track);
        if self.follow {
            self.reveal_current();
        }
    }

    fn reveal_current(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        let path = current.path.clone();
        if self.view == View::Library && self.select_library_track(&path) {
            return;
        }
        if let Some(index) = self
            .rows
            .iter()
            .position(|row| row.track.as_ref().is_some_and(|track| track.path == path))
        {
            self.set_selected(index);
        }
    }

    /// Select a track and switch the artist column to its artist.
    fn select_library_track(&mut self, path: &std::path::Path) -> bool {
        let Some(artist_index) = self
            .library_artists
            .iter()
            .position(|artist| artist.tracks.iter().any(|track| track.path == path))
        else {
            return false;
        };
        self.library_artist_state.select(Some(artist_index));
        self.refresh_rows();
        let Some(index) = self
            .rows
            .iter()
            .position(|row| row.track.as_ref().is_some_and(|track| track.path == path))
        else {
            return false;
        };
        self.library_focus = LibraryFocus::Tracks;
        self.set_selected(index);
        true
    }

    fn start_lyrics(&mut self, track: Track) {
        self.lyric_state = LyricState::Loading;
        self.lyrics = None;
        self.lyric_error.clear();
        self.lyric_retry_at = None;
        self.lyric_started = Instant::now();
        self.plain_index = 0;
        self.lyric_force = false;
        self.lyric_rx = Some(lyrics::spawn(track));
    }

    fn toggle_lyrics(&mut self) {
        self.set_lyrics(!self.show_lyrics);
    }

    fn toggle_lyrics_text(&mut self) {
        self.lyrics_hidden = !self.lyrics_hidden;
        if self.lyrics_hidden {
            self.set_message("lyrics hidden · Shift+L to show");
        } else {
            self.set_message("lyrics visible again");
        }
    }

    fn set_lyrics(&mut self, show: bool) {
        if self.show_lyrics == show {
            return;
        }
        self.show_lyrics = show;
        self.lyrics_hidden = false;
        if show {
            self.start_cava();
            if self.lyrics.is_none() && self.lyric_state != LyricState::Loading {
                if let Some(track) = &self.current {
                    self.start_lyrics(track.clone());
                }
            }
        } else {
            self.stop_cava();
        }
    }

    fn start_cava(&mut self) {
        if !self.cava_enabled || self.cava.is_some() {
            return;
        }
        if !Cava::available() {
            self.cava_error = "cava not found in PATH".to_string();
            return;
        }
        self.cava = Cava::start();
        if self.cava.is_none() {
            self.cava_error = "failed to start cava".to_string();
        }
    }

    fn stop_cava(&mut self) {
        if let Some(mut cava) = self.cava.take() {
            cava.stop();
        }
    }

    /// Current cava frame.
    pub fn cava_values(&mut self) -> Vec<u32> {
        self.cava
            .as_mut()
            .map(|cava| cava.values())
            .unwrap_or_default()
    }

    pub fn toggle_pause(&mut self) {
        if self.current.is_none() {
            return;
        }
        self.paused = !self.paused;
        if let Some(mpv) = &mut self.mpv {
            mpv.pause(self.paused);
        }
    }

    fn stop(&mut self) {
        if let Some(mpv) = &mut self.mpv {
            mpv.stop();
        }
        self.current = None;
        self.paused = false;
        self.position = 0.0;
        self.duration = 0.0;
        self.lyrics = None;
        self.lyric_state = LyricState::Idle;
        self.lyric_rx = None;
    }

    fn seek(&mut self, delta: f64) {
        if self.current.is_none() {
            return;
        }
        if let Some(mpv) = &mut self.mpv {
            mpv.seek_rel(delta);
        }
        let upper = if self.duration > 0.0 {
            self.duration
        } else {
            f64::MAX
        };
        self.position = (self.position + delta).clamp(0.0, upper);
    }

    fn next(&mut self) {
        if self.play_library {
            self.context = self.library_context();
        }
        if self.context.is_empty() {
            if self.continue_play {
                self.context = self.library_context();
            }
            if self.context.is_empty() {
                return;
            }
        }
        let current = self
            .current
            .as_ref()
            .and_then(|t| self.context.iter().position(|c| c.path == t.path));
        let index = match current {
            Some(index) if self.shuffle => {
                let mut pick = (index + 1) % self.context.len();
                if self.context.len() > 1 {
                    let seed = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() as usize)
                        .unwrap_or(0);
                    pick = seed % self.context.len();
                    if pick == index {
                        pick = (pick + 1) % self.context.len();
                    }
                }
                pick
            }
            Some(index) => index + 1,
            None => 0,
        };
        if index < self.context.len() {
            self.play_track(self.context[index].clone());
        } else if self.repeat {
            self.play_track(self.context[0].clone());
        } else if self.continue_play {
            let library = self.library_context();
            if let Some(track) = &self.current {
                if let Some(pos) = library.iter().position(|c| c.path == track.path) {
                    if pos + 1 < library.len() {
                        self.context = library;
                        self.play_track(self.context[pos + 1].clone());
                        return;
                    }
                }
            }
            self.stop();
        } else {
            self.stop();
        }
    }

    fn prev(&mut self) {
        if self.play_library {
            self.context = self.library_context();
        }
        if self.context.is_empty() {
            if self.continue_play {
                self.context = self.library_context();
            }
            if self.context.is_empty() {
                return;
            }
        }
        let current = self
            .current
            .as_ref()
            .and_then(|t| self.context.iter().position(|c| c.path == t.path))
            .unwrap_or(0);
        let index = if current == 0 {
            if self.repeat {
                self.context.len() - 1
            } else {
                0
            }
        } else {
            current - 1
        };
        self.play_track(self.context[index].clone());
    }

    fn next_album(&mut self) {
        if self.context.is_empty() {
            self.context = self.library_context();
        }
        if self.context.is_empty() {
            return;
        }
        let Some(current) = self.current.clone() else {
            self.play_track(self.context[0].clone());
            return;
        };
        let Some(position) = self
            .context
            .iter()
            .position(|track| track.path == current.path)
        else {
            self.play_track(self.context[0].clone());
            return;
        };
        let key = (current.artist_or().to_string(), current.album_or().to_string());
        if let Some(index) = self.context[position + 1..]
            .iter()
            .position(|track| {
                (track.artist_or(), track.album_or()) != (key.0.as_str(), key.1.as_str())
            })
            .map(|offset| position + 1 + offset)
        {
            self.play_track(self.context[index].clone());
        }
    }

    fn prev_album(&mut self) {
        if self.context.is_empty() {
            self.context = self.library_context();
        }
        if self.context.is_empty() {
            return;
        }
        let Some(current) = self.current.clone() else {
            self.play_track(self.context[0].clone());
            return;
        };
        let Some(position) = self
            .context
            .iter()
            .position(|track| track.path == current.path)
        else {
            return;
        };
        let key = (current.artist_or().to_string(), current.album_or().to_string());
        // Step back one track, then to the start of that album.
        let mut start = position;
        while start > 0
            && (self.context[start - 1].artist_or(), self.context[start - 1].album_or())
                == (key.0.as_str(), key.1.as_str())
        {
            start -= 1;
        }
        if start == 0 {
            return;
        }
        let previous_key = (
            self.context[start - 1].artist_or().to_string(),
            self.context[start - 1].album_or().to_string(),
        );
        let mut index = start - 1;
        while index > 0
            && (self.context[index - 1].artist_or(), self.context[index - 1].album_or())
                == (previous_key.0.as_str(), previous_key.1.as_str())
        {
            index -= 1;
        }
        self.play_track(self.context[index].clone());
    }

    fn toggle_repeat(&mut self) {
        self.repeat = !self.repeat;
        if self.repeat {
            self.repeat_current = false;
        }
        self.set_message(if self.repeat {
            "repeat on"
        } else {
            "repeat off"
        });
    }

    fn toggle_repeat_current(&mut self) {
        self.repeat_current = !self.repeat_current;
        if self.repeat_current {
            self.repeat = false;
        }
        self.set_message(if self.repeat_current {
            "repeat one on"
        } else {
            "repeat one off"
        });
    }

    fn select_current(&mut self) {
        let Some(current) = &self.current else {
            self.set_message("nothing is playing");
            return;
        };
        let path = current.path.clone();
        if self.view == View::Library {
            if self.select_library_track(&path) {
                return;
            }
            self.set_message("track not visible in the library");
            return;
        }
        if let Some(index) = self
            .rows
            .iter()
            .position(|row| row.track.as_ref().is_some_and(|track| track.path == path))
        {
            self.set_selected(index);
            return;
        }
        self.switch_view(View::Library);
        if self.select_library_track(&path) {
            return;
        }
        self.set_message("track not visible in the library");
    }

    fn echo_selected(&mut self) {
        let Some(row) = self.rows.get(self.selected_index()) else {
            self.set_message("empty");
            return;
        };
        if let Some(track) = &row.track {
            let genre = if track.genre.is_empty() {
                "—"
            } else {
                track.genre.as_str()
            };
            self.set_message(format!(
                "{} — {} · {} ({}) · {} · {} · {}",
                track.artist_or(),
                track.title_or(),
                track.album_or(),
                track.year.map(|year| year.to_string()).unwrap_or_else(|| "—".to_string()),
                genre,
                util::fmt_time(track.duration),
                track.path.display()
            ));
        } else {
            self.set_message(row.text.clone());
        }
    }

    fn request_quit(&mut self) {
        if self.current.is_some() {
            self.input = Input::Quit;
            self.set_message("Music is playing. Quit? (y/n)");
        } else {
            self.quit = true;
        }
    }

    fn run_command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.history.push(line.to_string());
        let (command, rest) = match line.split_once(char::is_whitespace) {
            Some((command, rest)) => (command, rest.trim()),
            None => (line, ""),
        };
        let command = resolve_command(command);
        match command {
            "add" | "a" => self.command_add(rest),
            "cd" => {
                let dir = if rest.is_empty() {
                    util::home()
                } else {
                    util::expand_path(rest)
                };
                if dir.is_dir() {
                    self.browser_dir = dir;
                    self.switch_view(View::Browser);
                    self.set_selected(0);
                } else {
                    self.set_message(format!("no such folder: {}", dir.display()));
                }
            }
            "clear" => match self.view {
                View::Library => {
                    self.lib.clear();
                    library::save_library(&self.lib);
                    self.refresh_rows();
                    self.set_message("library cleared");
                }
                View::Playlist => {
                    self.playlist.clear();
                    self.refresh_rows();
                    self.set_message("playlist cleared");
                }
                _ => self.set_message("clear works in library and playlist"),
            },
            "view" => self.command_view(rest),
            "play" | "player-play" => self.play_selected_or_resume(),
            "pause" | "player-pause" => self.toggle_pause(),
            "next" | "player-next" => self.next(),
            "prev" | "player-prev" => self.prev(),
            "player-next-album" => self.next_album(),
            "player-prev-album" => self.prev_album(),
            "stop" | "player-stop" => self.stop(),
            "shell" => self.command_shell(rest),
            "echo" => self.set_message(rest.to_string()),
            "refresh" => {
                self.refresh_rows();
                self.set_message("list refreshed");
            }
            "toggle" => self.command_toggle(rest),
            "seek" => match rest.parse::<f64>() {
                Ok(value) if rest.starts_with('+') || rest.starts_with('-') => {
                    self.seek(value)
                }
                Ok(value) => {
                    if let Some(mpv) = &mut self.mpv {
                        mpv.seek_abs(value);
                    }
                    self.position = value.max(0.0);
                }
                Err(_) => self.set_message("seek: a number of seconds is required"),
            },
            "set" => self.command_set(rest),
            "lyrics" | "l" => self.toggle_lyrics(),
            "search" => self.run_search(rest, false),
            "rescan" | "update-cache" => self.rescan(),
            "save" | "w" => self.command_save(rest),
            "load" => self.command_load(rest),
            "edit" => self.open_tag_editor(),
            "help" => self.open_help(),
            "quit" | "q" => self.request_quit(),
            other => self.set_message(format!("unknown command: {other}")),
        }
    }

    fn command_view(&mut self, rest: &str) {
        let view = match rest.trim().to_lowercase().as_str() {
            "1" | "tree" | "library" => View::Library,
            "2" | "playlist" => View::Playlist,
            "3" | "browser" => View::Browser,
            "4" | "settings" => View::Settings,
            other => {
                self.set_message(format!("no such view: {other}"));
                return;
            }
        };
        self.switch_view(view);
    }

    fn command_toggle(&mut self, rest: &str) {
        match rest.trim() {
            "repeat" => self.toggle_repeat(),
            "shuffle" => self.shuffle = !self.shuffle,
            "follow" => self.follow = !self.follow,
            "continue" => self.continue_play = !self.continue_play,
            "repeat_current" | "repeat-current" => self.toggle_repeat_current(),
            "aaa_mode" | "aaa-mode" => self.aaa_mode = !self.aaa_mode,
            "play_library" | "play-library" => self.play_library = !self.play_library,
            "show_remaining_time" | "remaining" => self.remaining = !self.remaining,
            "cava" => {
                self.cava_enabled = !self.cava_enabled;
                if self.cava_enabled && self.show_lyrics {
                    self.start_cava();
                } else if !self.cava_enabled {
                    self.stop_cava();
                }
            }
            other => self.set_message(format!("nothing to toggle: {other}")),
        }
    }

    fn command_shell(&mut self, rest: &str) {
        if rest.trim().is_empty() {
            self.set_message(":shell COMMAND");
            return;
        }
        match std::process::Command::new("sh").arg("-c").arg(rest).output() {
            Ok(output) => {
                let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if text.is_empty() {
                    text = String::from_utf8_lossy(&output.stderr).trim().to_string();
                }
                if text.is_empty() {
                    text = format!("exit code {}", output.status.code().unwrap_or(-1));
                }
                self.set_message(text);
            }
            Err(error) => self.set_message(format!("shell: {error}")),
        }
    }

    fn command_set(&mut self, rest: &str) {
        let Some((key, value)) = rest.split_once('=') else {
            self.set_message("format: :set repeat=on");
            return;
        };
        let flag = matches!(
            value.trim().to_lowercase().as_str(),
            "on" | "1" | "true" | "yes"
        );
        match key.trim() {
            "repeat" => {
                self.repeat = flag;
                if flag {
                    self.repeat_current = false;
                }
            }
            "shuffle" => self.shuffle = flag,
            "follow" => self.follow = flag,
            "continue" => self.continue_play = flag,
            "repeat_current" | "repeat-current" => {
                self.repeat_current = flag;
                if flag {
                    self.repeat = false;
                }
            }
            "aaa_mode" | "aaa-mode" => self.aaa_mode = flag,
            "play_library" | "play-library" => self.play_library = flag,
            "show_remaining_time" | "remaining" => self.remaining = flag,
            "cava" => {
                self.cava_enabled = flag;
                if self.cava_enabled && self.show_lyrics {
                    self.start_cava();
                } else if !self.cava_enabled {
                    self.stop_cava();
                }
            }
            "offset" => {
                if let Ok(v) = value.trim().parse::<f64>() {
                    self.offset = v;
                    self.set_message(format!("lyrics offset: {:+.2} s", self.offset));
                }
            }
            other => self.set_message(format!("unknown option: {other}")),
        }
        if self.view == View::Settings {
            self.refresh_rows();
        }
    }

    fn command_add(&mut self, rest: &str) {
        let mut rest = rest.trim();
        let mut target: Option<View> = None;
        loop {
            if take_flag(&mut rest, "-l") {
                target = Some(View::Library);
            } else if take_flag(&mut rest, "-p") {
                target = Some(View::Playlist);
            } else {
                break;
            }
        }
        if rest.is_empty() {
            if self.view == View::Browser {
                let dir = self.browser_dir.clone();
                self.add_path(&dir.to_string_lossy(), target);
            } else {
                self.set_message(":add [-l|-p] PATH");
            }
            return;
        }
        self.add_path(rest, target);
    }

    fn add_path(&mut self, raw: &str, target: Option<View>) {
        if raw.trim().is_empty() {
            self.set_message(":add PATH");
            return;
        }
        let path = util::expand_path(raw);
        if path.is_dir() {
            // Directories go to the library or, with -p, to the playlist.
            let dest = target.unwrap_or(self.view);
            if matches!(dest, View::Playlist) {
                let tracks = library::scan_dir(&path);
                if tracks.is_empty() {
                    self.set_message(format!("no music in folder: {}", path.display()));
                } else {
                    self.add_tracks_to(tracks, Some(dest));
                }
                return;
            }
            if !self.roots.contains(&path) {
                self.roots.push(path.clone());
            }
            self.start_scan(vec![path], false);
        } else if path.is_file() && library::is_playlist(&path) {
            let tracks = library::read_playlist(&path);
            if tracks.is_empty() {
                self.set_message(format!("playlist has no available tracks: {}", path.display()));
            } else {
                self.last_playlist = Some(path);
                self.add_tracks_to(tracks, target);
            }
        } else if path.is_file() {
            let track = library::read_track(&path);
            self.add_tracks_to(vec![track], target);
        } else {
            self.set_message(format!("path not found: {}", path.display()));
        }
    }

    /// Add tracks to the chosen view, or to the current one.
    fn add_tracks_to(&mut self, tracks: Vec<Track>, target: Option<View>) {
        if tracks.is_empty() {
            return;
        }
        let count = tracks.len();
        match target.unwrap_or(self.view) {
            View::Playlist => {
                self.playlist.extend(tracks);
                if self.view == View::Playlist {
                    self.refresh_rows();
                }
                self.set_message(format!("added to playlist: {count}"));
            }
            _ => {
                library::merge(&mut self.lib, tracks);
                library::save_library(&self.lib);
                self.refresh_rows();
                self.set_message(format!("added to library: {count}"));
            }
        }
    }

    fn command_save(&mut self, rest: &str) {
        let mut rest = rest.trim();
        let mut extended = false;
        let mut target: Option<View> = None;
        loop {
            if take_flag(&mut rest, "-e") {
                extended = true;
            } else if take_flag(&mut rest, "-l") {
                target = Some(View::Library);
            } else if take_flag(&mut rest, "-p") {
                target = Some(View::Playlist);
            } else {
                break;
            }
        }
        let file = if rest.is_empty() {
            match &self.last_playlist {
                Some(path) => path.clone(),
                None => {
                    self.set_message(":save [-e] [-l|-p] FILE");
                    return;
                }
            }
        } else if rest == "-" {
            self.set_message("cannot write to stdout in the TUI — specify a file");
            return;
        } else {
            util::expand_path(rest)
        };
        let view = target.unwrap_or(self.view);
        let tracks = match view {
            View::Library => {
                let mut tracks = self.lib.clone();
                tracks.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
                tracks
            }
            View::Playlist => self.playlist.clone(),
            _ => {
                self.set_message("can save from library and playlist only");
                return;
            }
        };
        if tracks.is_empty() {
            self.set_message("nothing to save");
            return;
        }
        match library::write_playlist(&file, &tracks, extended) {
            Ok(()) => {
                self.last_playlist = Some(file.clone());
                self.set_message(format!(
                    "saved {} tracks: {}",
                    tracks.len(),
                    file.display()
                ));
            }
            Err(error) => self.set_message(format!("failed to save: {error}")),
        }
    }

    fn command_load(&mut self, rest: &str) {
        let mut rest = rest.trim();
        let mut to_library = false;
        if take_flag(&mut rest, "-l") {
            to_library = true;
        }
        if rest.is_empty() {
            self.set_message(":load [-l] FILE");
            return;
        }
        let path = util::expand_path(rest);
        if !path.is_file() {
            self.set_message(format!("no such file: {}", path.display()));
            return;
        }
        let tracks = library::read_playlist(&path);
        if tracks.is_empty() {
            self.set_message(format!("playlist has no available tracks: {}", path.display()));
            return;
        }
        self.last_playlist = Some(path);
        let count = tracks.len();
        match self.view {
            View::Playlist if !to_library => {
                self.playlist = tracks;
                self.refresh_rows();
                self.set_message(format!("playlist loaded: {count}"));
            }
            _ => {
                library::merge(&mut self.lib, tracks);
                library::save_library(&self.lib);
                self.refresh_rows();
                self.set_message(format!("added to library: {count}"));
            }
        }
    }

    fn add_selection_to_library(&mut self) {
        let tracks = self.selected_tracks();
        if tracks.is_empty() {
            return;
        }
        let count = tracks.len();
        library::merge(&mut self.lib, tracks);
        library::save_library(&self.lib);
        self.refresh_rows();
        self.set_message(format!("added to library: {count}"));
    }

    fn add_selection_to_playlist(&mut self) {
        let tracks = self.selected_tracks();
        if tracks.is_empty() {
            return;
        }
        let count = tracks.len();
        self.playlist.extend(tracks);
        if self.view == View::Playlist {
            self.refresh_rows();
        }
        self.set_message(format!("added to playlist: {count}"));
    }

    fn remove_selection(&mut self) {
        if self.view == View::Settings {
            let Some(index) = self
                .rows
                .get(self.selected_index())
                .and_then(|row| row.setting)
            else {
                return;
            };
            self.reset_setting(index);
            self.refresh_rows();
            return;
        }
        if self.view != View::Playlist {
            self.set_message("can remove only in the playlist");
            return;
        }
        let selected = self
            .rows
            .get(self.selected_index())
            .and_then(|row| row.track.as_ref())
            .map(|track| track.path.clone());
        let marked: HashSet<PathBuf> = self
            .rows
            .iter()
            .filter_map(|row| row.track.clone())
            .filter(|track| self.marks.contains(&track.path))
            .map(|track| track.path)
            .collect();
        let targets = if marked.is_empty() {
            selected.into_iter().collect()
        } else {
            marked
        };
        if self.view == View::Playlist {
            self.playlist.retain(|track| !targets.contains(&track.path));
        }
        for path in &targets {
            self.marks.remove(path);
        }
        self.refresh_rows();
    }

    fn move_selected(&mut self, down: bool) {
        if self.view != View::Playlist {
            return;
        }
        let index = self.selected_index();
        let len = self.playlist.len();
        if len < 2 {
            return;
        }
        let other = if down {
            if index + 1 >= len {
                return;
            }
            index + 1
        } else {
            if index == 0 {
                return;
            }
            index - 1
        };
        self.playlist.swap(index, other);
        self.refresh_rows();
        self.set_selected(other);
    }

    fn run_search(&mut self, query: &str, backward: bool) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        self.search = Some(query.to_lowercase());
        self.search_jump(backward, true);
    }

    fn search_next(&mut self, forward: bool) {
        let Some(query) = self.search.clone() else {
            self.set_message("no search query — press /");
            return;
        };
        let _ = query;
        self.search_jump(!forward, false);
    }

    fn search_jump(&mut self, backward: bool, include_current: bool) {
        let Some(query) = &self.search else {
            return;
        };
        // cmus prefixes: // artist/album, ?? title.
        let (mode, needle) = if let Some(rest) = query.strip_prefix("//") {
            (1u8, rest)
        } else if let Some(rest) = query.strip_prefix("??") {
            (2u8, rest)
        } else {
            (0u8, query.as_str())
        };
        if needle.is_empty() || self.rows.is_empty() {
            return;
        }
        let len = self.rows.len();
        let current = self.selected_index();
        let start = if include_current {
            current
        } else if backward {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        for step in 0..len {
            let index = if backward {
                (start + len - step) % len
            } else {
                (start + step) % len
            };
            if row_matches(&self.rows[index], needle, mode) {
                self.set_selected(index);
                return;
            }
        }
        self.set_message("not found");
    }

    fn rescan(&mut self) {
        if self.roots.is_empty() {
            self.set_message("no folders: :add ~/Music");
            return;
        }
        self.start_scan(self.roots.clone(), true);
    }

    fn start_scan(&mut self, paths: Vec<PathBuf>, sync: bool) {
        if paths.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.scan_rx = Some(rx);
        self.scanning = true;
        self.scan_sync = sync;
        self.scan_paths = paths.clone();
        self.set_message("scanning library…");
        thread::spawn(move || {
            let mut tracks = Vec::new();
            for path in paths {
                tracks.extend(library::scan_dir(&path));
            }
            let _ = tx.send(tracks);
        });
    }

    fn tick(&mut self) {
        if signal_quit() {
            self.quit = true;
        }
        if let Some(rx) = &self.mpv_rx {
            match rx.try_recv() {
                Ok(Ok(mut mpv)) => {
                    if let Some(track) = &self.current {
                        mpv.play(&track.path);
                        if self.position > 0.5 {
                            mpv.seek_abs(self.position);
                        }
                        if self.paused {
                            mpv.pause(true);
                        }
                    }
                    self.mpv = Some(mpv);
                    self.mpv_rx = None;
                    self.mpv_stop = None;
                    if self.message == "mpv restarting…" {
                        self.message.clear();
                    }
                }
                Ok(Err(err)) => {
                    self.set_message(format!("{err} — audio unavailable"));
                    self.mpv_rx = None;
                    self.mpv_stop = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.set_message("failed to start mpv");
                    self.mpv_rx = None;
                    self.mpv_stop = None;
                }
            }
        }

        let events = self
            .mpv
            .as_mut()
            .map(|mpv| mpv.poll())
            .unwrap_or_default();
        for event in events {
            match event {
                MpvEvent::Time(time) => self.position = time.max(0.0),
                MpvEvent::Duration(duration) => {
                    self.duration = duration;
                    if let Some(track) = &mut self.current {
                        if track.duration <= 0.0 {
                            track.duration = duration;
                        }
                    }
                }
                MpvEvent::Pause(paused) => self.paused = paused,
                MpvEvent::FileLoaded => {}
                MpvEvent::EndFile(reason) => {
                    if reason == "eof" {
                        if self.repeat_current {
                            if let Some(track) = self.current.clone() {
                                self.play_track(track);
                            }
                        } else {
                            self.next();
                        }
                    }
                }
                MpvEvent::Gone => {
                    self.mpv = None;
                    if self.current.is_some() {
                        // Bring mpv back and resume.
                        self.ensure_mpv();
                        self.set_message("mpv restarting…");
                    } else {
                        self.set_message("mpv exited");
                    }
                }
            }
        }

        if let Some(rx) = self.lyric_rx.take() {
            match rx.try_recv() {
                Ok(Ok(lyrics)) => {
                    let forced = std::mem::take(&mut self.lyric_force);
                    let current_synced = self.lyrics.as_ref().is_some_and(|old| old.synced);
                    if forced && !lyrics.synced && current_synced {
                        let reason = if lyrics.instrumental {
                            "lrclib considers the track instrumental"
                        } else {
                            "lrclib has unsynced lyrics only"
                        };
                        self.set_message(format!("{reason} — kept the current lyrics"));
                        self.lyric_state = LyricState::Ready;
                    } else {
                        if forced && !lyrics.instrumental {
                            if let Some(track) = self.current.clone() {
                                match lyrics::save_lrc(&track, &lyrics) {
                                    Ok(path) => self.set_message(format!(
                                        "lyrics from lrclib saved: {}",
                                        path.display()
                                    )),
                                    Err(error) => self.set_message(format!(
                                        "lyrics from lrclib not saved: {error}"
                                    )),
                                }
                            }
                        }
                        self.lyrics = Some(lyrics);
                        self.lyric_state = LyricState::Ready;
                    }
                }
                Ok(Err(error)) => {
                    let forced = std::mem::take(&mut self.lyric_force);
                    if forced && self.lyrics.is_some() {
                        self.set_message(format!("lrclib: {error}"));
                        self.lyric_state = LyricState::Ready;
                    } else {
                        let retryable = !error.contains("not found")
                            && !error.contains("no lyrics")
                            && !error.contains("instrumental");
                        self.lyric_error = error;
                        self.lyric_state = LyricState::Missing;
                        self.lyric_retry_at =
                            retryable.then(|| Instant::now() + Duration::from_secs(30));
                    }
                }
                Err(TryRecvError::Empty) => self.lyric_rx = Some(rx),
                Err(TryRecvError::Disconnected) => {
                    self.lyric_force = false;
                    self.lyric_state = LyricState::Missing;
                }
            }
        }

        if let Some(rx) = &self.scan_rx {
            match rx.try_recv() {
                Ok(tracks) => {
                    let sync = self.scan_sync;
                    let roots = std::mem::take(&mut self.scan_paths);
                    let before: HashSet<PathBuf> =
                        self.lib.iter().map(|track| track.path.clone()).collect();
                    if sync {
                        // A full re-check of saved roots drops entries whose
                        // files are gone and picks up newly added music.
                        self.lib.retain(|track| {
                            !roots.iter().any(|root| track.path.starts_with(root))
                                || track.path.is_file()
                        });
                    }
                    library::merge(&mut self.lib, tracks);
                    library::save_library(&self.lib);
                    let after: HashSet<PathBuf> =
                        self.lib.iter().map(|track| track.path.clone()).collect();
                    let added = after.difference(&before).count();
                    let removed = before.difference(&after).count();
                    self.scan_rx = None;
                    self.scanning = false;
                    self.scan_sync = false;
                    self.refresh_rows();
                    if sync {
                        self.set_message(format!(
                            "library: {} tracks (+{added} −{removed})",
                            self.lib.len()
                        ));
                    } else {
                        self.set_message(format!(
                            "library: {} tracks (+{added})",
                            self.lib.len()
                        ));
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.scan_rx = None;
                    self.scanning = false;
                    self.scan_sync = false;
                    self.scan_paths.clear();
                }
            }
        }

        if self.lyric_state == LyricState::Missing && self.lyric_rx.is_none() {
            if let Some(at) = self.lyric_retry_at {
                if Instant::now() >= at {
                    if let Some(track) = self.current.clone() {
                        self.start_lyrics(track);
                    }
                }
            }
        }

        self.retry_mpris();
        let commands = self.mpris.as_ref().map(|mpris| mpris.drain()).unwrap_or_default();
        for command in commands {
            self.handle_mpris(command);
        }
        self.update_mpris();

        if !self.message.is_empty() && self.message_at.elapsed() > Duration::from_secs(6) {
            self.message.clear();
        }
    }

    fn retry_mpris(&mut self) {
        if self.mpris.is_some() {
            return;
        }
        let Some(at) = self.mpris_retry_at else {
            return;
        };
        if Instant::now() < at {
            return;
        }
        match Mpris::spawn() {
            Ok(mpris) => {
                self.mpris = Some(mpris);
                self.mpris_error.clear();
                self.mpris_retry_at = None;
                self.set_message("MPRIS connected");
            }
            Err(error) => {
                self.mpris_error = error;
                self.mpris_retry_at = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    fn handle_mpris(&mut self, command: MprisCommand) {
        match command {
            MprisCommand::Play => {
                if self.current.is_none() {
                    self.play_selected_or_resume();
                } else if self.paused {
                    self.toggle_pause();
                }
            }
            MprisCommand::Pause => {
                if self.current.is_some() && !self.paused {
                    self.toggle_pause();
                }
            }
            MprisCommand::PlayPause => {
                if self.current.is_none() {
                    self.play_selected_or_resume();
                } else {
                    self.toggle_pause();
                }
            }
            MprisCommand::Stop => self.stop(),
            MprisCommand::Next => self.next(),
            MprisCommand::Previous => self.prev(),
            MprisCommand::Seek(offset) => self.seek(offset as f64 / 1_000_000.0),
            MprisCommand::SetPosition(position) => {
                let seconds = (position as f64 / 1_000_000.0).max(0.0);
                if let Some(mpv) = &mut self.mpv {
                    mpv.seek_abs(seconds);
                }
                self.position = seconds;
            }
            MprisCommand::SetShuffle(shuffle) => self.shuffle = shuffle,
            MprisCommand::SetLoop(status) => match status.as_str() {
                "Track" => {
                    self.repeat_current = true;
                    self.repeat = false;
                }
                "Playlist" => {
                    self.repeat = true;
                    self.repeat_current = false;
                }
                _ => {
                    self.repeat = false;
                    self.repeat_current = false;
                }
            },
            MprisCommand::OpenUri(uri) => {
                if let Some(rest) = uri.strip_prefix("file://") {
                    let decoded =
                        percent_encoding::percent_decode_str(rest).decode_utf8_lossy();
                    self.add_path(&decoded, Some(View::Library));
                    let path = util::expand_path(&decoded);
                    if let Some(track) = self.lib.iter().find(|track| track.path == path).cloned() {
                        self.context = vec![track.clone()];
                        self.play_track(track);
                    }
                } else {
                    self.set_message(format!("MPRIS: only file:// is supported — {uri}"));
                }
            }
            MprisCommand::Raise => self.set_message("tmus is already open"),
            MprisCommand::Quit => self.request_quit(),
        }
    }

    fn update_mpris(&self) {
        let Some(mpris) = &self.mpris else {
            return;
        };
        let snapshot = if let Some(track) = &self.current {
            Snapshot {
                has_track: true,
                track_id: mpris::track_id(&track.path),
                title: track.title_or().to_string(),
                artist: track.artist_or().to_string(),
                album: track.album_or().to_string(),
                url: mpris::file_uri(&track.path),
                duration_us: (self.duration.max(track.duration) * 1_000_000.0).round() as i64,
                position_us: (self.position.max(0.0) * 1_000_000.0).round() as i64,
                paused: self.paused,
                shuffle: self.shuffle,
                loop_status: if self.repeat_current {
                    "Track".to_string()
                } else if self.repeat {
                    "Playlist".to_string()
                } else {
                    "None".to_string()
                },
            }
        } else {
            Snapshot {
                shuffle: self.shuffle,
                loop_status: if self.repeat_current {
                    "Track".to_string()
                } else if self.repeat {
                    "Playlist".to_string()
                } else {
                    "None".to_string()
                },
                ..Default::default()
            }
        };
        mpris.update(snapshot);
    }

    pub fn current_lyric(&self) -> Option<CurrentLyric> {
        let lyrics = self.lyrics.as_ref()?;
        let title = self
            .current
            .as_ref()
            .map(|track| track.title_or().to_string())
            .unwrap_or_default();
        Some(lyrics.current(self.position + self.offset, &title, self.plain_index))
    }

    pub fn banner_lines(&self, cols: usize, avail: usize) -> Vec<String> {
        match self.current_lyric() {
            Some(current) => banner::render_compact(
                &current.text,
                cols,
                avail,
                current.progress(self.position + self.offset),
            ),
            None => vec![String::new(); avail],
        }
    }

    pub fn lyric_note(&self) -> String {
        let mut notes = Vec::new();
        if self.lyrics_hidden {
            notes.push("lyrics hidden".to_string());
        }
        if let Some(current) = self.current_lyric() {
            if !current.note.is_empty() {
                notes.push(current.note);
            }
        }
        if self.offset.abs() > 0.001 {
            notes.push(format!("offset {:+.2} s", self.offset));
        }
        if let Some(lyrics) = &self.lyrics {
            notes.push(lyrics.source.note().to_string());
        }
        notes.join(" · ")
    }

    pub fn lyric_wait(&self) -> u64 {
        self.lyric_started.elapsed().as_secs()
    }

    pub fn lyric_retry_in(&self) -> Option<u64> {
        let at = self.lyric_retry_at?;
        let now = Instant::now();
        (at > now).then(|| (at - now).as_secs() + 1)
    }

    pub fn refetch_lyrics(&mut self) {
        let Some(track) = self.current.clone() else {
            return;
        };
        // Keep the current text until lrclib answers; if it has nothing
        // better, the screen should not go blank.
        self.lyric_state = LyricState::Loading;
        self.lyric_error.clear();
        self.lyric_retry_at = None;
        self.lyric_started = Instant::now();
        self.plain_index = 0;
        self.lyric_force = true;
        self.lyric_rx = Some(lyrics::spawn_remote(track));
    }

    fn scroll_plain(&mut self, delta: isize) {
        let Some(lyrics) = &self.lyrics else {
            return;
        };
        if lyrics.synced {
            self.set_message("lyrics are synced — no need to scroll");
            return;
        }
        let len = lyrics.lines.len();
        if len == 0 {
            return;
        }
        let index = (self.plain_index.min(len - 1) as isize + delta).clamp(0, len as isize - 1);
        self.plain_index = index as usize;
    }

    fn save_lyrics(&mut self) {
        let Some(track) = self.current.clone() else {
            self.set_message("no active track");
            return;
        };
        let Some(lyrics) = self.lyrics.as_ref() else {
            self.set_message("no lyrics to save");
            return;
        };
        match lyrics::save_lrc(&track, lyrics) {
            Ok(path) => {
                if let Some(saved) = &mut self.lyrics {
                    saved.source = lyrics::Source::Local;
                }
                self.set_message(format!("lyrics saved: {}", path.display()));
            }
            Err(error) => self.set_message(format!("failed to save lyrics: {error}")),
        }
    }

    fn confirm_delete_file(&mut self) {
        if self.view != View::Library {
            self.set_message("delete file: library view only");
            return;
        }
        if self.library_focus == LibraryFocus::Artists {
            self.set_message("select a track in the right column");
            return;
        }
        let Some(track) = self
            .rows
            .get(self.selected_index())
            .and_then(|row| row.track.clone())
        else {
            self.set_message("no track selected");
            return;
        };
        self.pending_delete = Some(track.path.clone());
        self.input = Input::DeleteFile;
    }

    fn delete_file(&mut self) {
        let Some(path) = self.pending_delete.take() else {
            self.input = Input::None;
            return;
        };
        self.input = Input::None;
        match std::fs::remove_file(&path) {
            Ok(()) => {
                let playing = self
                    .current
                    .as_ref()
                    .is_some_and(|track| track.path == path);
                self.lib.retain(|track| track.path != path);
                self.playlist.retain(|track| track.path != path);
                self.context.retain(|track| track.path != path);
                self.marks.remove(&path);
                if playing {
                    self.stop();
                }
                library::save_library(&self.lib);
                self.refresh_rows();
                self.set_message(format!("file deleted: {}", path.display()));
            }
            Err(error) => self.set_message(format!("failed to delete file: {error}")),
        }
    }

    fn track_for_edit(&self) -> Option<Track> {
        if self.view == View::Library && self.library_focus == LibraryFocus::Artists {
            return self
                .library_artists
                .get(self.selected_artist_index())
                .and_then(|artist| artist.tracks.first().cloned());
        }
        let row = self.rows.get(self.selected_index())?;
        if let Some(track) = &row.track {
            return Some(track.clone());
        }
        if row.kind == RowKind::File {
            if let Some(path) = &row.path {
                return Some(library::read_track(path));
            }
        }
        None
    }

    fn open_tag_editor(&mut self) {
        let Some(track) = self.track_for_edit() else {
            self.set_message("editor: select a track");
            return;
        };
        let mut data = match tags::read(&track.path) {
            Ok(data) => data,
            Err(error) => {
                self.set_message(format!("editor: {error}"));
                return;
            }
        };
        if data.title.trim().is_empty() {
            data.title = track.title.clone();
        }
        self.tag_editor = Some(TagEditor {
            path: track.path.clone(),
            track: track.label(),
            data,
            field: 0,
            cursor: 0,
            lyrics_mode: false,
            lyrics_cursor: 0,
            lyrics_scroll: 0,
            lyrics_hscroll: 0,
            message: String::new(),
            dirty: false,
            close_armed: false,
        });
        if let Some(editor) = self.tag_editor.as_mut() {
            editor.cursor = editor.data.title.len();
        }
    }

    fn handle_tag_editor_key(&mut self, key: KeyEvent) {
        let Some(mut editor) = self.tag_editor.take() else {
            return;
        };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);

        if control && matches!(key.code, KeyCode::Char('s')) {
            self.tag_editor = Some(editor);
            self.save_tag_editor();
            return;
        }
        if control && matches!(key.code, KeyCode::Char('c')) {
            self.set_message("editor closed");
            return;
        }

        if editor.lyrics_mode {
            if matches!(key.code, KeyCode::Esc) {
                editor.lyrics_mode = false;
                editor.close_armed = false;
                self.tag_editor = Some(editor);
                return;
            }
            if editor.edit_lyrics(key) {
                editor.dirty = true;
                editor.close_armed = false;
            }
            self.tag_editor = Some(editor);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                if editor.dirty && !editor.close_armed {
                    editor.close_armed = true;
                    editor.message =
                        "unsaved changes: press Esc again to close without saving".to_string();
                } else {
                    self.set_message("editor closed");
                    return;
                }
            }
            KeyCode::Up => editor.move_field(-1),
            KeyCode::Down => editor.move_field(1),
            KeyCode::Tab => {
                let next = (editor.field + 1) % TagField::ALL.len();
                editor.select_field(next);
            }
            KeyCode::Enter => {
                if editor.selected() == TagField::Lyrics {
                    editor.lyrics_mode = true;
                    editor.lyrics_cursor = editor.data.lyrics.len();
                    editor.lyrics_scroll = 0;
                    editor.lyrics_hscroll = 0;
                    editor.message.clear();
                } else if editor.field + 1 < TagField::ALL.len() {
                    editor.select_field(editor.field + 1);
                }
            }
            KeyCode::Char('u') if control => {
                if let Some(value) = editor.value_mut() {
                    value.clear();
                    editor.cursor = 0;
                    editor.dirty = true;
                    editor.close_armed = false;
                }
            }
            _ => {
                if editor.selected() != TagField::Lyrics && editor.edit_text(key) {
                    editor.dirty = true;
                    editor.close_armed = false;
                    editor.message.clear();
                }
            }
        }
        self.tag_editor = Some(editor);
    }

    fn save_tag_editor(&mut self) {
        let Some(editor) = self.tag_editor.as_ref() else {
            return;
        };
        let path = editor.path.clone();
        let data = editor.data.clone();
        match tags::save(&path, &data) {
            Ok(()) => {
                self.tag_editor = None;
                self.reload_track(&path);
                self.set_message(format!("tags saved: {}", path.display()));
            }
            Err(error) => {
                if let Some(editor) = self.tag_editor.as_mut() {
                    editor.message = format!("not saved: {error}");
                }
            }
        }
    }

    fn reload_track(&mut self, path: &std::path::Path) {
        let fresh = library::read_track(path);
        let mut in_library = false;
        for track in &mut self.lib {
            if track.path == path {
                *track = fresh.clone();
                in_library = true;
            }
        }
        for track in &mut self.playlist {
            if track.path == path {
                *track = fresh.clone();
            }
        }
        for track in &mut self.context {
            if track.path == path {
                *track = fresh.clone();
            }
        }
        if let Some(current) = &mut self.current {
            if current.path == path {
                *current = fresh;
            }
        }
        if in_library {
            library::save_library(&self.lib);
        }
        self.refresh_rows();
        self.update_mpris();
    }

    fn open_builder(&mut self) {
        if self.view != View::Playlist {
            self.set_message("playlist builder: playlist view");
            return;
        }
        let mut left = self.lib.clone();
        left.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        let mut left_state = ListState::default();
        if !left.is_empty() {
            left_state.select(Some(0));
        }
        self.builder = Some(PlaylistBuilder {
            name: String::new(),
            left,
            right: Vec::new(),
            left_state,
            right_state: ListState::default(),
            focus: BuilderFocus::Name,
        });
    }

    fn handle_builder_key(&mut self, key: KeyEvent) {
        let Some(mut builder) = self.builder.take() else {
            return;
        };

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('s'))
        {
            self.builder = Some(builder);
            self.save_builder();
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.builder = None;
                self.set_message("playlist builder closed");
                return;
            }
            KeyCode::Tab => {
                builder.focus = match builder.focus {
                    BuilderFocus::Name => BuilderFocus::Left,
                    BuilderFocus::Left => BuilderFocus::Right,
                    BuilderFocus::Right => BuilderFocus::Name,
                };
            }
            KeyCode::Backspace if builder.focus == BuilderFocus::Name => {
                builder.name.pop();
            }
            KeyCode::Char(ch)
                if builder.focus == BuilderFocus::Name
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                builder.name.push(ch);
            }
            KeyCode::Up | KeyCode::Down => {
                let delta = if key.code == KeyCode::Up { -1 } else { 1 };
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                if builder.focus == BuilderFocus::Right && shift {
                    reorder_state(&mut builder.right, &mut builder.right_state, delta);
                } else {
                    let state = match builder.focus {
                        BuilderFocus::Left => Some(&mut builder.left_state),
                        BuilderFocus::Right => Some(&mut builder.right_state),
                        BuilderFocus::Name => None,
                    };
                    if let Some(state) = state {
                        let len = if builder.focus == BuilderFocus::Left {
                            builder.left.len()
                        } else {
                            builder.right.len()
                        };
                        shift_state(state, len, delta);
                    }
                }
            }
            KeyCode::Enter => match builder.focus {
                BuilderFocus::Name => builder.focus = BuilderFocus::Left,
                BuilderFocus::Left => {
                    let index = builder.left_state.selected().unwrap_or(0);
                    if index < builder.left.len() {
                        let track = builder.left.remove(index);
                        builder.right.push(track);
                        let next = index.min(builder.left.len().saturating_sub(1));
                        builder.left_state.select((!builder.left.is_empty()).then_some(next));
                        builder.right_state.select(Some(builder.right.len() - 1));
                    }
                }
                BuilderFocus::Right => {
                    let index = builder.right_state.selected().unwrap_or(0);
                    if index < builder.right.len() {
                        let track = builder.right.remove(index);
                        builder.left.push(track);
                        builder.left.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
                        let next = index.min(builder.right.len().saturating_sub(1));
                        builder.right_state.select((!builder.right.is_empty()).then_some(next));
                    }
                }
            },
            _ => {}
        }

        self.builder = Some(builder);
    }

    fn save_builder(&mut self) {
        let Some(builder) = &self.builder else {
            return;
        };
        let name = playlist_name(&builder.name);
        if name.is_empty() {
            self.set_message("enter a playlist name");
            return;
        }
        if builder.right.is_empty() {
            self.set_message("playlist has no tracks");
            return;
        }
        let path = util::data_dir()
            .join("playlists")
            .join(format!("{name}.m3u"));
        match library::write_playlist(&path, &builder.right, true) {
            Ok(()) => {
                let count = builder.right.len();
                self.playlist = builder.right.clone();
                self.last_playlist = Some(path.clone());
                self.builder = None;
                self.view = View::Playlist;
                self.refresh_rows();
                self.set_message(format!(
                    "playlist saved: {count} tracks, {}",
                    path.display()
                ));
            }
            Err(error) => self.set_message(format!("failed to save playlist: {error}")),
        }
    }

    fn open_keymap_editor(&mut self) {
        let mut state = ListState::default();
        state.select(Some(0));
        self.keymap_editor = Some(KeymapEditor {
            state,
            capture: false,
            message: String::new(),
        });
    }

    fn handle_keymap_editor_key(&mut self, key: KeyEvent) {
        let Some(mut editor) = self.keymap_editor.take() else {
            return;
        };

        if editor.capture {
            if matches!(key.code, KeyCode::Esc) {
                editor.capture = false;
                editor.message = "input cancelled".to_string();
                self.keymap_editor = Some(editor);
                return;
            }
            if let Some(binding) = Binding::from_event(&key) {
                let action = editor.selected_action();
                if keys::is_reserved(binding) {
                    editor.message = format!(
                        "{} is reserved by the interface — choose another",
                        binding.display()
                    );
                } else if let Some(other) = self.keymap.conflict(action, binding) {
                    editor.message = format!(
                        "{} is already bound to: {}",
                        binding.display(),
                        other.label()
                    );
                } else {
                    self.keymap.set(action, binding);
                    self.save_state();
                    editor.message = format!("{}: {}", action.label(), binding.display());
                }
                editor.capture = false;
            }
            self.keymap_editor = Some(editor);
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.keymap_editor = None;
                self.set_message("keys saved");
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                shift_state(&mut editor.state, Action::ALL.len(), -1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                shift_state(&mut editor.state, Action::ALL.len(), 1);
            }
            KeyCode::PageUp => {
                shift_state(&mut editor.state, Action::ALL.len(), -10);
            }
            KeyCode::PageDown => {
                shift_state(&mut editor.state, Action::ALL.len(), 10);
            }
            KeyCode::Home => editor.state.select(Some(0)),
            KeyCode::End => editor.state.select(Some(Action::ALL.len() - 1)),
            KeyCode::Enter | KeyCode::Char(' ') => {
                editor.capture = true;
                editor.message = format!(
                    "new key for {} — Esc cancels",
                    editor.selected_action().label()
                );
            }
            KeyCode::Char('r') | KeyCode::Delete => {
                let action = editor.selected_action();
                self.keymap.reset(action);
                self.save_state();
                editor.message = format!("reset: {}", action.label());
            }
            _ => {}
        }
        self.keymap_editor = Some(editor);
    }

    fn open_help(&mut self) {
        let mut state = ListState::default();
        state.select(Some(0));
        self.help = Some(HelpOverlay { state });
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        let Some(mut help) = self.help.take() else {
            return;
        };
        let len = keys::rows(&self.keymap).len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::F(1) => {
                self.help = None;
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => shift_state(&mut help.state, len, -1),
            KeyCode::Down | KeyCode::Char('j') => shift_state(&mut help.state, len, 1),
            KeyCode::PageUp => shift_state(&mut help.state, len, -10),
            KeyCode::PageDown => shift_state(&mut help.state, len, 10),
            KeyCode::Home => help.state.select(Some(0)),
            KeyCode::End => {
                let last = len.saturating_sub(1);
                help.state.select(Some(last));
            }
            _ => {}
        }
        self.help = Some(help);
    }
}

fn shift_state(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let current = state.selected().unwrap_or(0) as isize;
    let next = (current + delta).clamp(0, len as isize - 1) as usize;
    state.select(Some(next));
}

fn reorder_state(items: &mut [Track], state: &mut ListState, delta: isize) {
    if items.len() < 2 {
        return;
    }
    let current = state.selected().unwrap_or(0);
    let target = current as isize + delta;
    if target < 0 || target >= items.len() as isize {
        return;
    }
    items.swap(current, target as usize);
    state.select(Some(target as usize));
}

fn playlist_name(raw: &str) -> String {
    let mut name = raw.trim().to_string();
    for extension in [".m3u8", ".m3u"] {
        if name.to_lowercase().ends_with(extension) {
            name.truncate(name.len() - extension.len());
            break;
        }
    }
    name = name
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | '\0') {
                '-'
            } else {
                ch
            }
        })
        .collect();
    let name = name.trim().to_string();
    if name == "." || name == ".." {
        String::new()
    } else {
        name
    }
}

/// All command names, used to expand unambiguous prefixes.
const COMMANDS: &[&str] = &[
    "add",
    "cd",
    "clear",
    "view",
    "play",
    "pause",
    "next",
    "prev",
    "player-play",
    "player-pause",
    "player-next",
    "player-prev",
    "player-next-album",
    "player-prev-album",
    "player-stop",
    "stop",
    "shell",
    "echo",
    "refresh",
    "toggle",
    "seek",
    "set",
    "lyrics",
    "search",
    "rescan",
    "update-cache",
    "save",
    "load",
    "edit",
    "help",
    "quit",
];

fn resolve_command(command: &str) -> &str {
    if COMMANDS.contains(&command) {
        return command;
    }
    let mut found: Option<&str> = None;
    for candidate in COMMANDS {
        if candidate.starts_with(command) {
            if found.is_some() {
                // Ambiguous: let the dispatcher handle it.
                return command;
            }
            found = Some(candidate);
        }
    }
    found.unwrap_or(command)
}

/// Consume a leading `-x` flag. It must be a whole word.
fn take_flag<'a>(rest: &mut &'a str, flag: &str) -> bool {
    let trimmed = rest.trim_start();
    if trimmed == flag {
        *rest = "";
        return true;
    }
    if let Some(after) = trimmed.strip_prefix(flag) {
        if after.starts_with(char::is_whitespace) {
            *rest = after.trim_start();
            return true;
        }
    }
    false
}

/// Match a row. Mode 1 = artist/album, 2 = title, 0 = everything.
fn row_matches(row: &Row, needle: &str, mode: u8) -> bool {
    if let Some(track) = &row.track {
        let hay = match mode {
            1 => format!("{} {}", track.artist_or(), track.album_or()).to_lowercase(),
            2 => track.title_or().to_lowercase(),
            _ => track.label().to_lowercase(),
        };
        hay.contains(needle)
    } else {
        row.text.to_lowercase().contains(needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_flags_are_whole_words() {
        let mut rest = "-e -p /tmp/a b.m3u";
        assert!(take_flag(&mut rest, "-e"));
        assert!(take_flag(&mut rest, "-p"));
        assert!(!take_flag(&mut rest, "-l"));
        assert_eq!(rest, "/tmp/a b.m3u");

        let mut rest = "-list";
        assert!(!take_flag(&mut rest, "-l"));
        assert_eq!(rest, "-list");
    }

    #[test]
    fn commands_accept_unambiguous_prefixes() {
        assert_eq!(resolve_command("sa"), "save");
        assert_eq!(resolve_command("sear"), "search");
        assert_eq!(resolve_command("pau"), "pause");
        assert_eq!(resolve_command("vi"), "view");
        // `s` is ambiguous (save/search/set/seek/shell/stop).
        assert_eq!(resolve_command("s"), "s");
        // Short aliases stay untouched.
        assert_eq!(resolve_command("w"), "w");
        assert_eq!(resolve_command("l"), "l");
        assert_eq!(resolve_command("a"), "add");
    }

    #[test]
    fn lyrics_cursor_counts_characters_not_bytes() {
        let text = "Äpfel\nZweite Zeile";
        let first_end = "Äpfel".len();
        let second_start = first_end + 1;

        assert_eq!(line_column(text, first_end), (0, 5));
        assert_eq!(line_column(text, text.len()), (1, 12));
        assert_eq!(cursor_at(text, 0, 99), first_end);
        assert_eq!(cursor_at(text, 1, 6), second_start + "Zweite".len());
        assert_eq!(cursor_at(text, 1, 99), text.len());
        assert_eq!(line_start(text, text.len()), second_start);
        assert_eq!(line_end(text, second_start), text.len());
        assert_eq!(prev_boundary(text, first_end), "Äpfe".len());
        assert_eq!(next_boundary(text, first_end), second_start);
    }
}
