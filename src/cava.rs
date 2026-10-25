//! cava spectrum visualizer.
//!
//! cava only emits frames when stdin is a terminal, so stdin gets a pty and
//! stdout stays a pipe. A frame is a line of `;`-separated values from 0 to 100.

use std::fs;
use std::io::Read;
use std::os::fd::{FromRawFd, RawFd};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::util;

pub const BARS: u32 = 60;

const CONFIG: &str = "\
[general]
framerate = 30
bars = {bars}
[output]
method = raw
raw_target = /dev/stdout
data_format = ascii
ascii_max_range = 100
bar_delimiter = 59
frame_delimiter = 10
";

pub struct Cava {
    child: Option<Child>,
    master: Option<RawFd>,
    values: Arc<Mutex<Vec<u32>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Cava {
    pub fn available() -> bool {
        find_cava().is_some()
    }

    pub fn start() -> Option<Cava> {
        let exe = find_cava()?;
        let config = util::cache_dir().join("cava.conf");
        if let Some(parent) = config.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        fs::write(&config, CONFIG.replace("{bars}", &BARS.to_string())).ok()?;

        let (master, slave) = open_pty()?;
        let mut cmd = Command::new(exe);
        cmd.arg("-p")
            .arg(&config)
            // SAFETY: `slave` was just created by openpty and ownership is
            // handed to the child's stdin. The parent's copy is closed when
            // the Command (and its Stdio) is dropped below.
            .stdin(unsafe { Stdio::from_raw_fd(slave) })
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(_) => {
                unsafe { libc::close(master) };
                return None;
            }
        };
        drop(cmd);

        let stdout = child.stdout.take();
        let values = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = stdout.map(|stdout| {
            let values = values.clone();
            let stop = stop.clone();
            thread::spawn(move || reader(stdout, values, stop))
        });

        Some(Cava {
            child: Some(child),
            master: Some(master),
            values,
            stop,
            thread,
        })
    }

    /// Latest frame, or an empty list if cava is not running any more.
    pub fn values(&mut self) -> Vec<u32> {
        if let Some(child) = &mut self.child {
            if let Ok(Some(_)) = child.try_wait() {
                return Vec::new();
            }
        }
        self.values.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(master) = self.master.take() {
            // SAFETY: the descriptor is owned by us and closed only once.
            unsafe { libc::close(master) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Cava {
    fn drop(&mut self) {
        self.stop();
    }
}

fn find_cava() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("cava");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn open_pty() -> Option<(RawFd, RawFd)> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: all pointers are valid for the duration of the call.
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &mut size,
        )
    };
    if result == 0 && master >= 0 && slave >= 0 {
        Some((master, slave))
    } else {
        None
    }
}

fn reader(mut stdout: std::process::ChildStdout, values: Arc<Mutex<Vec<u32>>>, stop: Arc<AtomicBool>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    while !stop.load(Ordering::Relaxed) {
        let read = match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        buf.extend_from_slice(&chunk[..read]);
        while let Some(pos) = buf.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = &line[..line.len().saturating_sub(1)];
            let line = trim(line);
            if line.is_empty() || !line[0].is_ascii_digit() {
                continue;
            }
            let parsed: Option<Vec<u32>> = line
                .split(|byte| *byte == b';')
                .filter(|part| !part.is_empty())
                .map(|part| std::str::from_utf8(part).ok()?.trim().parse::<u32>().ok())
                .collect();
            if let Some(mut parsed) = parsed {
                for value in &mut parsed {
                    *value = (*value).min(100);
                }
                if !parsed.is_empty() {
                    if let Ok(mut current) = values.lock() {
                        *current = parsed;
                    }
                }
            }
        }
    }
}

fn trim(mut line: &[u8]) -> &[u8] {
    while let Some((first, rest)) = line.split_first() {
        if first.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = line.split_last() {
        if last.is_ascii_whitespace() || *last == b';' {
            line = rest;
        } else {
            break;
        }
    }
    line
}

/// Spectrum bars as text. Bars are centred and one or two columns thick; a
/// narrow screen is resampled instead of dropping the high frequencies.
pub fn canvas(values: &[u32], rows: usize, cols: usize) -> Vec<String> {
    if rows == 0 || cols == 0 || values.is_empty() {
        return vec![" ".repeat(cols); rows];
    }
    let bar_w = if cols >= 56 { 2 } else { 1 };
    let gap = 1;
    let count = ((cols + gap) / (bar_w + gap)).max(1).min(values.len());
    let values: Vec<u32> = if count < values.len() {
        let step = values.len() as f64 / count as f64;
        (0..count)
            .map(|index| values[((index as f64 * step) as usize).min(values.len() - 1)])
            .collect()
    } else {
        values.to_vec()
    };
    let span = count * bar_w + count.saturating_sub(1) * gap;
    let left = cols.saturating_sub(span) / 2;
    let mut grid = vec![vec![' '; cols]; rows];
    for (index, raw) in values.iter().enumerate() {
        let level = (*raw).min(100) as f64 / 100.0;
        let filled = (level * rows as f64).round() as usize;
        let x0 = left + index * (bar_w + gap);
        for x in x0..(x0 + bar_w).min(cols) {
            for y in 0..rows {
                if rows - 1 - y < filled {
                    grid[y][x] = '█';
                }
            }
        }
    }
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_grow_from_the_bottom_and_stay_inside() {
        let values = vec![100, 50, 0, 25];
        let rows = canvas(&values, 10, 40);
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|line| line.chars().count() == 40));
        assert!(rows[9].contains('█'));
        assert!(rows[0].chars().filter(|c| *c == '█').count() <= 1);
    }

    #[test]
    fn empty_input_is_blank() {
        let rows = canvas(&[], 3, 20);
        assert_eq!(rows, vec![" ".repeat(20); 3]);
    }

    #[test]
    fn narrow_screens_resample() {
        let values: Vec<u32> = (0..60).collect();
        let rows = canvas(&values, 5, 20);
        assert!(rows.iter().all(|line| line.chars().count() == 20));
        assert!(rows[4].contains('█'));
    }
}
