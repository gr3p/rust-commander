use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// Rotated like logrotate: rust-commander.log, .log.1 (newest old) up to .log.3.
const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: u32 = 3;

struct Sink {
    file: std::fs::File,
    size: u64,
}

static LOG: Mutex<Option<Sink>> = Mutex::new(None);

pub fn info(msg: &str) {
    write("info", msg, false);
}

pub fn error(msg: &str) {
    write("error", msg, true);
}

pub fn path() -> PathBuf {
    state_dir().join("rust-commander.log")
}

fn write(level: &str, msg: &str, echo: bool) {
    let line = format!("{} {level} {msg}\n", stamp());
    if echo {
        eprint!("{line}");
    }
    let Ok(mut slot) = LOG.lock() else {
        return;
    };
    if slot.as_ref().is_some_and(|sink| sink.size >= MAX_BYTES) {
        *slot = None;
    }
    if slot.is_none() {
        *slot = open(&path());
    }
    if let Some(sink) = slot.as_mut() {
        if sink.file.write_all(line.as_bytes()).is_ok() {
            sink.size += line.len() as u64;
        }
        let _ = sink.file.flush();
    }
}

fn open(path: &Path) -> Option<Sink> {
    fs::create_dir_all(path.parent()?).ok()?;
    // Another window may have rotated already; then the file is small again and is only reopened.
    if fs::metadata(path).is_ok_and(|meta| meta.len() >= MAX_BYTES) {
        rotate(path);
    }
    let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    Some(Sink { file, size })
}

fn rotate(path: &Path) {
    for n in (1..KEEP).rev() {
        let _ = fs::rename(numbered(path, n), numbered(path, n + 1));
    }
    let _ = fs::rename(path, numbered(path, 1));
}

fn numbered(path: &Path, n: u32) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

// No home at all is reported on stderr: this is the log.
fn state_dir() -> PathBuf {
    let dir = crate::config::xdg_dir("XDG_STATE_HOME", ".local/state").unwrap_or_else(|| {
        static WARN: std::sync::Once = std::sync::Once::new();
        WARN.call_once(|| eprintln!("rust-commander: no home directory; the log is kept in ./.local/state"));
        PathBuf::from(".local/state")
    });
    dir.join("rust-commander")
}

fn stamp() -> String {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "--:--:--".into();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_log_rotates_and_keeps_three() {
        let dir = std::env::temp_dir().join(format!("rust-commander-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("rust-commander.log");
        for round in 0..5u8 {
            let mut sink = open(&path).unwrap();
            assert!(sink.size < MAX_BYTES);
            sink.file.write_all(&vec![b'0' + round; MAX_BYTES as usize]).unwrap();
        }
        let sink = open(&path).unwrap();
        assert_eq!(sink.size, 0);
        assert_eq!(fs::read(numbered(&path, 1)).unwrap()[0], b'4');
        assert_eq!(fs::read(numbered(&path, 3)).unwrap()[0], b'2');
        assert!(!numbered(&path, 4).exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
