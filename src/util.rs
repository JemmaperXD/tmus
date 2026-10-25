use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn env_path(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn data_dir() -> PathBuf {
    env_path("XDG_DATA_HOME", ".local/share").join("tmus")
}

pub fn config_dir() -> PathBuf {
    env_path("XDG_CONFIG_HOME", ".config").join("tmus")
}

pub fn cache_dir() -> PathBuf {
    env_path("XDG_CACHE_HOME", ".cache").join("tmus")
}

/// An advisory lock held for as long as the process runs. The kernel drops it
/// on exit, so a killed instance never leaves a stale lock behind.
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// `Ok(None)` means another tmus already holds the lock.
    pub fn acquire() -> std::io::Result<Option<Self>> {
        let path = match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("tmus.lock"),
            _ => cache_dir().join("tmus.lock"),
        };
        Self::acquire_at(&path)
    }

    fn acquire_at(path: &Path) -> std::io::Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(Some(Self { _file: file }));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Ok(None);
        }
        Err(error)
    }
}

/// Expand a leading `~` and make the path absolute.
pub fn expand_path(raw: &str) -> PathBuf {
    let raw = raw.trim();
    let expanded = if raw == "~" {
        home()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(raw)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| home())
            .join(expanded)
    }
}

pub fn fmt_time(sec: f64) -> String {
    if !sec.is_finite() || sec < 0.0 {
        return "--:--".to_string();
    }
    let sec = sec as u64;
    let (h, m, s) = (sec / 3600, (sec % 3600) / 60, sec % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn truncate(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('…');
    out
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::InstanceLock;

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let path = std::env::temp_dir().join(format!("tmus-lock-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let first = InstanceLock::acquire_at(&path).unwrap();
        assert!(first.is_some());
        assert!(InstanceLock::acquire_at(&path).unwrap().is_none());

        drop(first);
        assert!(InstanceLock::acquire_at(&path).unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }
}
