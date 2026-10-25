# tmus

[![ci](https://github.com/JemmaperXD/tmus/actions/workflows/ci.yml/badge.svg)](https://github.com/JemmaperXD/tmus/actions/workflows/ci.yml)

![tmus library screen](assets/library.png)

A TUI music player for Linux, roughly in the spirit of [cmus](https://cmus.github.io/).
Playback goes through `mpv`, MPRIS is exposed for playerctl and desktop widgets,
and the lyrics screen draws the current line in large block letters over an
optional cava spectrum.

There are four screens: Library, Playlist, Browser and Settings.

## Install

Prebuilt packages are on the [releases page](https://github.com/JemmaperXD/tmus/releases).

Debian/Ubuntu:

```sh
sudo apt install ./tmus_0.1.0_amd64.deb
```

Any Linux, from the tarball:

```sh
tar -xzf tmus-v0.1.0-x86_64-linux.tar.gz
./tmus
```

From source you need stable Rust, `mpv` and the D-Bus headers (`dbus` on Arch,
`libdbus-1-dev` on Debian/Ubuntu). `cava` is optional.

```sh
cargo build --release --offline
install -Dm755 target/release/tmus ~/.local/bin/tmus
```

Dependencies are vendored in `vendor/`, so the build works without network.

## Run

```sh
tmus
tmus ~/Music
tmus ~/Music/album/*.flac
```

Without arguments it opens `~/Music` (or `~/music`). A single banner frame can be
rendered without starting the TUI:

```sh
tmus --banner "When you were here before" --width 100 --height 28
```

## Keys

| Key | Action |
| --- | --- |
| `1` … `4` | Library, playlist, browser, settings |
| `Tab` | Next library column; on other screens, next screen |
| `j` / `k`, `↓` / `↑` | Down / up |
| `g` / `G`, `Home` / `End` | Top / bottom |
| `Enter`, `x` | Play the selection |
| `c`, `space` | Pause / resume |
| `v` | Stop |
| `b` / `z` | Next / previous track |
| `B` / `Z` | Next / previous album |
| `←` / `→`, `h` | Seek ±5 seconds |
| `,` / `.` | Seek ±1 minute |
| `s` / `r` | Shuffle / repeat |
| `Ctrl+R` | Repeat the current track |
| `/` | Search |
| `l` | Lyrics screen |
| `F1`, `?` | Full key table |
| `q` | Quit |

Playback and list bindings can be changed in Settings. Navigation keys stay
fixed. The rest, including the playlist builder (`Ctrl+N`), tag editor (`:edit`)
and browser keys, is in `tmus help`.

Search has two prefixes: `//word` searches artists and albums, `??word` searches
titles.

## Lyrics

![Lyrics screen](assets/lyrics.png)

tmus first looks for a `.lrc` next to the track, matching either the full
filename or the song title and artist. It reads `[mm:ss.xx]`, multiple stamps per
line, `[offset:…]` and enhanced `<mm:ss.xx>` tags. Plain text works too, but it
does not scroll by itself.

If there is no local file, embedded lyrics are used, then [lrclib.net](https://lrclib.net/)
is asked and the answer is cached. `r` skips the local file and cache and asks
again. `Ctrl+S` saves the current lyrics as a `.lrc` file, `Shift+L` hides the
text but keeps the cava background.

The banner uses `smblock` for Latin text and a built-in 5×7 font for Cyrillic and
other characters. The lyric color matches the tab bar.

## Commands

Press `:` for the command line. Unambiguous prefixes work, so `:a` is `:add` and
`:w` is `:save`.

| Command | Action |
| --- | --- |
| `:add [-l\|-p] PATH` | Add a file, directory or playlist |
| `:cd [DIR]` | Change the browser directory |
| `:save [-e] [-l\|-p] FILE` | Save a playlist |
| `:load [-l] FILE` | Load a playlist |
| `:edit` | Edit tags and embedded lyrics |
| `:set KEY=VALUE` | Modes, lyric offset, cava |
| `:seek N` | Seek to N seconds; `+N` / `-N` are relative |
| `:view 1` … `:view 4` | Switch screens |
| `:rescan` | Add new files, drop missing ones |
| `:help` | Key table |
| `:quit` | Quit |

`:set` and `:toggle` accept `repeat`, `shuffle`, `follow`, `continue`,
`repeat_current`, `aaa_mode`, `play_library`, `show_remaining_time` and `cava`.
The offset is set with `offset=N`.

## MPRIS

tmus takes `org.mpris.MediaPlayer2.tmus` and exposes metadata, play/pause/stop,
next/previous, seek, shuffle and loop.

```sh
playerctl -p tmus status
playerctl -p tmus metadata
playerctl -p tmus play-pause
```

Volume is left to mpv and the system mixer. The MPRIS `Volume` property reports
100% and ignores writes. If another program holds the name, tmus keeps running
without MPRIS and retries later.

## Files

| Path | Contents |
| --- | --- |
| `~/.local/share/tmus/library.json` | Library |
| `~/.local/share/tmus/playlists/` | Named `.m3u` playlists |
| `~/.config/tmus/state.json` | Roots, playlist, modes and keys |
| `~/.cache/tmus/lyrics/` | Lyrics cache |
| `~/.cache/tmus/cava.conf` | cava config |

Paths respect `XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_CACHE_HOME`.

Only one instance can run at a time. The lock lives in `$XDG_RUNTIME_DIR` (or
`~/.cache/tmus`) and is released when the process exits.

## Credits

Playback is handled by [mpv](https://mpv.io/). The TUI uses
[ratatui](https://ratatui.rs/) and [crossterm](https://github.com/crossterm-rs/crossterm).
Lyrics come from [lrclib.net](https://lrclib.net/). The `smblock` banner font is
by Sam Hocevar and is distributed under the [WTFPL v2](https://www.wtfpl.net/).

## License

MIT.
