mod app;
mod banner;
mod cava;
mod keys;
mod library;
mod lyrics;
mod mpris;
mod mpv;
mod tags;
mod ui;
mod util;

use std::io::IsTerminal;

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.first().is_some_and(|arg| arg == "help") {
        let state = library::load_state();
        let keymap = keys::Keymap::from_persisted(&state.keybindings);
        keys::print_table(&keymap);
        return Ok(());
    }

    // Debug helper: print one banner frame without touching the terminal.
    if let Some(pos) = argv.iter().position(|arg| arg == "--banner") {
        let text = argv.get(pos + 1).cloned().unwrap_or_default();
        let width = flag_value(&argv, "--width").unwrap_or(80);
        let height = flag_value(&argv, "--height").unwrap_or(20);
        for line in banner::render(&text, width, height, 0.0) {
            println!("{line}");
        }
        return Ok(());
    }

    let mut paths = Vec::new();
    for arg in argv {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "-V" | "--version" => {
                println!("tmus {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => paths.push(arg),
        }
    }

    if !std::io::stdout().is_terminal() {
        eprintln!("tmus: a terminal is required (this is an interactive TUI player)");
        std::process::exit(1);
    }

    let _lock = match util::InstanceLock::acquire() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            eprintln!("tmus is already running");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("tmus: failed to acquire lock: {error}");
            std::process::exit(1);
        }
    };

    let mut app = app::App::new(paths);
    app.run()
}

fn flag_value(argv: &[String], flag: &str) -> Option<usize> {
    let pos = argv.iter().position(|arg| arg == flag)?;
    argv.get(pos + 1)?.parse().ok()
}

fn print_help() {
    println!(
        "tmus {} — a cmus-inspired TUI player

Usage:
  tmus [FILE|DIR ...]
  tmus help                                      table of all keys
  tmus --banner TEXT [--width N] [--height N]   render one banner frame

Only one tmus instance can run at a time; a second instance exits
immediately.

Views:
  1 Library   2 Playlist   3 Browser   4 Settings
  Tab — switch column in the library / go to the next view

Playback:
  Enter / x — play selected      c / space — pause      v — stop
  b / z — next / previous        B / Z — albums
  ←/→ / h — ±5 s                 , / . — ±1 min
  s — shuffle                    r — repeat list
  Ctrl+R — repeat one track      t — remaining time
  f — follow the current track

Lists:
  j / k — down / up              g / G — first / last
  Ctrl+D — delete a file in the library (with confirmation)
  Ctrl+U — half page up          space — mark
  a — add to library             y — add to playlist
  D / Delete — remove            p / P — move
  i — jump to the playing track
  Ctrl+N — build a playlist (playlist view only)
  in the builder: Tab — column, Enter — add/remove, Shift+↑/↓ — reorder,
  Ctrl+S — save to ~/.local/share/tmus/playlists/NAME.m3u

Browser:
  Enter / space — enter or play  Backspace — go up
  i — hidden files               u — refresh

Settings:
  Enter — change                 D — reset a value

Lyrics:
  l — open/close the full-screen lyrics (lrclib.net + local .lrc)
  while open: l/Esc — back, space — pause, ←/→ — ±5 s,
  ↑/↓ or j/k — scroll unsynced lyrics, Ctrl+S — save .lrc,
  < / > — offset, 0 — reset, r — refetch from lrclib,
  Shift+L — hide the text but keep cava; cava draws the background;
  bracketed text is drawn one line below the main line

Command line:
  :add [-l|-p] PATH         :save [-e] [-l|-p] FILE
  :load [-l] FILE           :set KEY=VALUE
  :play :pause :next :prev :stop :seek ±SEC
  :view 1-4 :rescan :save :quit
  :edit — tags of the selected file (Ctrl+S save, Esc close)
  / — search, n / N — next / previous; // and ?? — by tags
  ? or F1 — key table; :help — the same

MPRIS:
  playerctl -p tmus play|pause|next|previous|metadata
  (bus name org.mpris.MediaPlayer2.tmus; if another player owns the name,
  tmus runs without MPRIS and retries later)

Files:
  ~/.local/share/tmus/library.json   library
  ~/.local/share/tmus/playlists/     named .m3u playlists
  ~/.config/tmus/state.json          playlist, settings and keys
  ~/.cache/tmus/lyrics/              lyrics cache
  ~/.cache/tmus/cava.conf            visualizer config

Differences from cmus: l opens lyrics (cmus seeks +5 s),
aaa_mode is set with :set aaa_mode=on, Ctrl+C quits.
",
        env!("CARGO_PKG_VERSION")
    );
}
