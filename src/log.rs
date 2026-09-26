use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

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
    if slot.is_none() {
        *slot = open();
    }
    if let Some(file) = slot.as_mut() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

fn open() -> Option<std::fs::File> {
    let dir = state_dir();
    fs::create_dir_all(&dir).ok()?;
    OpenOptions::new().create(true).append(true).open(dir.join("rust-commander.log")).ok()
}

fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(dir).join("rust-commander");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/state/rust-commander")
}

fn stamp() -> String {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "--:--:--".into();
        }
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}
