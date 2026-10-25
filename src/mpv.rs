use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub enum MpvEvent {
    Time(f64),
    Duration(f64),
    Pause(bool),
    EndFile(String),
    FileLoaded,
    Gone,
}

pub struct Mpv {
    child: Child,
    writer: UnixStream,
    rx: Receiver<MpvEvent>,
    socket: PathBuf,
}

impl Mpv {
    pub fn start(stop: Arc<AtomicBool>) -> Result<Self, String> {
        let socket = std::env::temp_dir().join(format!("tmus-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);

        let mut child = Command::new("mpv")
            .args([
                "--no-config".to_string(),
                "--idle=yes".to_string(),
                "--no-video".to_string(),
                "--no-terminal".to_string(),
                "--really-quiet".to_string(),
                format!("--input-ipc-server={}", socket.to_string_lossy()),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start mpv: {e}"))?;

        let deadline = Instant::now() + Duration::from_secs(4);
        while !socket.exists() {
            if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
                let cancelled = stop.load(Ordering::Relaxed);
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&socket);
                return Err(if cancelled {
                    "mpv start cancelled".to_string()
                } else {
                    "mpv did not open the IPC socket".to_string()
                });
            }
            thread::sleep(Duration::from_millis(25));
        }

        let writer = match UnixStream::connect(&socket) {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&socket);
                return Err(format!("failed to connect to mpv: {error}"));
            }
        };
        let reader = match writer.try_clone() {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&socket);
                return Err(format!("failed to connect to mpv: {error}"));
            }
        };

        let (tx, rx) = channel();
        thread::spawn(move || {
            let reader = BufReader::new(reader);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(event) = value.get("event").and_then(|v| v.as_str()) else {
                    continue;
                };
                match event {
                    "property-change" => {
                        let name = value.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let data = &value["data"];
                        let event = match name {
                            "time-pos" => data.as_f64().map(MpvEvent::Time),
                            "duration" => data.as_f64().map(MpvEvent::Duration),
                            "pause" => data.as_bool().map(MpvEvent::Pause),
                            _ => None,
                        };
                        if let Some(event) = event {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                    "file-loaded" => {
                        if tx.send(MpvEvent::FileLoaded).is_err() {
                            break;
                        }
                    }
                    "end-file" => {
                        let reason = value
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if tx.send(MpvEvent::EndFile(reason)).is_err() {
                            break;
                        }
                    }
                    "shutdown" => {
                        let _ = tx.send(MpvEvent::Gone);
                        break;
                    }
                    _ => {}
                }
            }
            let _ = tx.send(MpvEvent::Gone);
        });

        let mut mpv = Mpv {
            child,
            writer,
            rx,
            socket,
        };
        if stop.load(Ordering::Relaxed) {
            return Err("mpv start cancelled".to_string());
        }
        for (id, prop) in [
            (1, "time-pos"),
            (2, "duration"),
            (3, "pause"),
            (4, "media-title"),
        ] {
            mpv.send(json!({"command": ["observe_property", id, prop]}));
        }
        Ok(mpv)
    }

    fn send(&mut self, value: Value) {
        let _ = writeln!(self.writer, "{value}");
        let _ = self.writer.flush();
    }

    pub fn play(&mut self, path: &Path) {
        self.send(json!({"command": ["loadfile", path.to_string_lossy(), "replace"]}));
        self.send(json!({"command": ["set", "pause", "no"]}));
    }

    pub fn pause(&mut self, paused: bool) {
        // mpv's `set` command only accepts strings here: JSON booleans are
        // rejected as an invalid parameter.
        self.send(json!({"command": ["set", "pause", if paused { "yes" } else { "no" }]}));
    }

    pub fn stop(&mut self) {
        self.send(json!({"command": ["stop"]}));
    }

    pub fn seek_abs(&mut self, secs: f64) {
        self.send(json!({"command": ["seek", secs.max(0.0), "absolute"]}));
    }

    pub fn seek_rel(&mut self, secs: f64) {
        self.send(json!({"command": ["seek", secs, "relative"]}));
    }

    pub fn poll(&mut self) -> Vec<MpvEvent> {
        let mut events = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(event) => events.push(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        events
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        self.send(json!({"command": ["quit"]}));
        thread::sleep(Duration::from_millis(40));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}
