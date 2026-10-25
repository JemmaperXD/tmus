//! MPRIS 2 server: makes tmus visible to playerctl, desktop media widgets and
//! other D-Bus clients.
//!
//! The D-Bus thread owns the connection and the object tree; the player keeps
//! a shared snapshot up to date and receives commands through a channel.

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use dbus::arg::{PropMap, RefArg, Variant};
use dbus::blocking::Connection;
use dbus::channel::MatchingReceiver;
use dbus::message::{MatchRule, Message};
use dbus::Path;
use dbus_crossroads::{Context, Crossroads};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.tmus";
pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

#[derive(Clone, Debug)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek(i64),
    SetPosition(i64),
    SetShuffle(bool),
    SetLoop(String),
    OpenUri(String),
    Raise,
    Quit,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub has_track: bool,
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub url: String,
    pub duration_us: i64,
    pub position_us: i64,
    pub paused: bool,
    pub shuffle: bool,
    pub loop_status: String,
}

impl Snapshot {
    pub fn playback_status(&self) -> &'static str {
        if !self.has_track {
            "Stopped"
        } else if self.paused {
            "Paused"
        } else {
            "Playing"
        }
    }
}

struct Data {
    state: Arc<Mutex<Snapshot>>,
    cmd: Sender<Command>,
}

impl Data {
    fn send(&self, command: Command) {
        let _ = self.cmd.send(command);
    }

    fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }
}

pub struct Mpris {
    pub rx: Receiver<Command>,
    state: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
}

impl Mpris {
    pub fn spawn() -> Result<Mpris, String> {
        let connection = Connection::new_session()
            .map_err(|error| format!("no D-Bus connection: {error}"))?;
        // Another player may already own the name. The reply can still mean
        // "already taken" (Exists/InQueue), so inspect it.
        use dbus::blocking::stdintf::org_freedesktop_dbus::RequestNameReply;
        let reply = connection
            .request_name(BUS_NAME, false, false, true)
            .map_err(|error| format!("failed to acquire {BUS_NAME}: {error}"))?;
        if !matches!(
            reply,
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner
        ) {
            return Err(format!(
                "{BUS_NAME} is already taken — MPRIS is disabled for now"
            ));
        }

        let state = Arc::new(Mutex::new(Snapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = channel();
        let thread_state = state.clone();
        let thread_stop = stop.clone();
        thread::spawn(move || serve(connection, thread_state, cmd_tx, thread_stop));

        Ok(Mpris {
            rx: cmd_rx,
            state,
            stop,
        })
    }

    pub fn update(&self, snapshot: Snapshot) {
        if let Ok(mut state) = self.state.lock() {
            *state = snapshot;
        }
    }

    pub fn drain(&self) -> Vec<Command> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(command) => out.push(command),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for Mpris {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(
    connection: Connection,
    state: Arc<Mutex<Snapshot>>,
    cmd: Sender<Command>,
    stop: Arc<AtomicBool>,
) {
    let mut cr = Crossroads::new();

    let root = cr.register("org.mpris.MediaPlayer2", |b| {
        b.method("Raise", (), (), |_ctx: &mut Context, data: &mut Data, _: ()| {
            data.send(Command::Raise);
            Ok(())
        });
        b.method("Quit", (), (), |_ctx: &mut Context, data: &mut Data, _: ()| {
            data.send(Command::Quit);
            Ok(())
        });
        b.property::<bool, _>("CanQuit")
            .get(|_, _: &mut Data| Ok(true));
        b.property::<bool, _>("CanRaise")
            .get(|_, _: &mut Data| Ok(false));
        b.property::<bool, _>("HasTrackList")
            .get(|_, _: &mut Data| Ok(false));
        b.property::<String, _>("Identity")
            .get(|_, _: &mut Data| Ok("tmus".to_string()));
        b.property::<String, _>("DesktopEntry")
            .get(|_, _: &mut Data| Ok("tmus".to_string()));
        b.property::<Vec<String>, _>("SupportedUriSchemes")
            .get(|_, _: &mut Data| Ok(vec!["file".to_string()]));
        b.property::<Vec<String>, _>("SupportedMimeTypes")
            .get(|_, _: &mut Data| Ok(Vec::new()));
    });

    let player = cr.register(PLAYER_IFACE, |b| {
        b.method("Play", (), (), |_ctx: &mut Context, data: &mut Data, _: ()| {
            data.send(Command::Play);
            Ok(())
        });
        b.method("Pause", (), (), |_ctx: &mut Context, data: &mut Data, _: ()| {
            data.send(Command::Pause);
            Ok(())
        });
        b.method(
            "PlayPause",
            (),
            (),
            |_ctx: &mut Context, data: &mut Data, _: ()| {
                data.send(Command::PlayPause);
                Ok(())
            },
        );
        b.method("Stop", (), (), |_ctx: &mut Context, data: &mut Data, _: ()| {
            data.send(Command::Stop);
            Ok(())
        });
        b.method("Next", (), (), |_ctx: &mut Context, data: &mut Data, _: ()| {
            data.send(Command::Next);
            Ok(())
        });
        b.method(
            "Previous",
            (),
            (),
            |_ctx: &mut Context, data: &mut Data, _: ()| {
                data.send(Command::Previous);
                Ok(())
            },
        );
        b.method(
            "Seek",
            ("Offset",),
            (),
            |_ctx: &mut Context, data: &mut Data, (offset,): (i64,)| {
                data.send(Command::Seek(offset));
                Ok(())
            },
        );
        b.method(
            "SetPosition",
            ("TrackId", "Position"),
            (),
            |_ctx: &mut Context, data: &mut Data, (_, position): (Path<'static>, i64)| {
                data.send(Command::SetPosition(position));
                Ok(())
            },
        );
        b.method(
            "OpenUri",
            ("Uri",),
            (),
            |_ctx: &mut Context, data: &mut Data, (uri,): (String,)| {
                data.send(Command::OpenUri(uri));
                Ok(())
            },
        );

        b.property::<String, _>("PlaybackStatus")
            .get(|_, data: &mut Data| Ok(data.snapshot().playback_status().to_string()));
        b.property::<String, _>("LoopStatus")
            .get(|_, data: &mut Data| Ok(data.snapshot().loop_status))
            .set(|_, data: &mut Data, value: String| {
                data.send(Command::SetLoop(value));
                Ok(None)
            });
        b.property::<f64, _>("Rate")
            .get(|_, _: &mut Data| Ok(1.0))
            .set(|_, _: &mut Data, value: f64| {
                if (value - 1.0).abs() < 0.001 {
                    Ok(None)
                } else {
                    Err(dbus_crossroads::MethodErr::invalid_arg("tmus supports rate 1.0 only"))
                }
            });
        b.property::<bool, _>("Shuffle")
            .get(|_, data: &mut Data| Ok(data.snapshot().shuffle))
            .set(|_, data: &mut Data, value: bool| {
                data.send(Command::SetShuffle(value));
                Ok(None)
            });
        b.property::<PropMap, _>("Metadata")
            .get(|_, data: &mut Data| Ok(metadata(&data.snapshot())));
        b.property::<f64, _>("Volume")
            .get(|_, _: &mut Data| Ok(1.0))
            .set(|_, _: &mut Data, _: f64| Ok(None));
        b.property::<i64, _>("Position")
            .get(|_, data: &mut Data| Ok(data.snapshot().position_us));
        b.property::<f64, _>("MinimumRate")
            .get(|_, _: &mut Data| Ok(1.0));
        b.property::<f64, _>("MaximumRate")
            .get(|_, _: &mut Data| Ok(1.0));
        b.property::<bool, _>("CanGoNext")
            .get(|_, _: &mut Data| Ok(true));
        b.property::<bool, _>("CanGoPrevious")
            .get(|_, _: &mut Data| Ok(true));
        b.property::<bool, _>("CanPlay")
            .get(|_, _: &mut Data| Ok(true));
        b.property::<bool, _>("CanPause")
            .get(|_, _: &mut Data| Ok(true));
        b.property::<bool, _>("CanSeek")
            .get(|_, _: &mut Data| Ok(true));
        b.property::<bool, _>("CanControl")
            .get(|_, _: &mut Data| Ok(true));
    });

    cr.insert(
        OBJECT_PATH,
        &[root, player],
        Data {
            state: state.clone(),
            cmd,
        },
    );

    connection.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |message, connection| {
            if let Err(error) = cr.handle_message(message, connection) {
                eprintln!("tmus: MPRIS: {error:?}");
            }
            true
        }),
    );

    let mut last = snapshot(&state);
    while !stop.load(Ordering::Relaxed) {
        if connection.process(Duration::from_millis(100)).is_err() {
            break;
        }
        let now = snapshot(&state);
        if now != last {
            emit_changes(&connection, &last, &now);
            last = now;
        }
    }
}

fn snapshot(state: &Arc<Mutex<Snapshot>>) -> Snapshot {
    state
        .lock()
        .map(|state| state.clone())
        .unwrap_or_default()
}

fn metadata(state: &Snapshot) -> PropMap {
    let mut map = PropMap::new();
    if !state.has_track {
        return map;
    }
    let mut insert = |key: &str, value: Box<dyn RefArg>| {
        map.insert(key.to_string(), Variant(value));
    };
    insert("mpris:trackid", Box::new(Path::from(state.track_id.clone())));
    insert("mpris:length", Box::new(state.duration_us));
    insert("xesam:title", Box::new(state.title.clone()));
    insert(
        "xesam:artist",
        Box::new(vec![state.artist.clone()]),
    );
    insert("xesam:album", Box::new(state.album.clone()));
    insert("xesam:url", Box::new(state.url.clone()));
    map
}

fn emit_changes(connection: &Connection, old: &Snapshot, new: &Snapshot) {
    let path = Path::from(OBJECT_PATH);
    let iface: dbus::strings::Interface<'static> = PLAYER_IFACE.into();
    let mut changed: PropMap = HashMap::new();

    if old.playback_status() != new.playback_status() {
        changed.insert(
            "PlaybackStatus".to_string(),
            Variant(Box::new(new.playback_status().to_string())),
        );
    }
    if old.loop_status != new.loop_status {
        changed.insert(
            "LoopStatus".to_string(),
            Variant(Box::new(new.loop_status.clone())),
        );
    }
    if old.shuffle != new.shuffle {
        changed.insert("Shuffle".to_string(), Variant(Box::new(new.shuffle)));
    }
    if old.has_track != new.has_track
        || old.track_id != new.track_id
        || old.title != new.title
        || old.artist != new.artist
        || old.album != new.album
        || old.duration_us != new.duration_us
    {
        changed.insert("Metadata".to_string(), Variant(Box::new(metadata(new))));
    }

    if !changed.is_empty() {
        // PropertiesChanged lives on org.freedesktop.DBus.Properties.
        let member: dbus::strings::Member<'static> = "PropertiesChanged".into();
        let iface: dbus::strings::Interface<'static> =
            "org.freedesktop.DBus.Properties".into();
        let message = Message::signal(&path, &iface, &member).append3(
            PLAYER_IFACE.to_string(),
            changed,
            Vec::<String>::new(),
        );
        let _ = connection.channel().send(message);
    }

    let delta = (new.position_us - old.position_us).abs();
    // A track change resets the position; that is not a seek.
    if new.has_track && old.track_id == new.track_id && delta > 1_500_000 {
        let member: dbus::strings::Member<'static> = "Seeked".into();
        let message = Message::signal(&path, &iface, &member).append1(new.position_us);
        let _ = connection.channel().send(message);
    }
}

/// Build the `mpris:trackid` object path for a track file.
pub fn track_id(path: &FsPath) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("/org/tmus/track/{:016x}", hasher.finish())
}

/// `file://` URI for the track, suitable for `xesam:url`.
pub fn file_uri(path: &FsPath) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
    const ENCODE: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}');
    format!(
        "file://{}",
        utf8_percent_encode(&path.to_string_lossy(), ENCODE)
    )
}
