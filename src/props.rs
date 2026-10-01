use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use std::os::unix::fs::MetadataExt;

// Owner, group, other, then SUID, SGID, sticky.
pub const BITS: [u32; 12] = [
    0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001, 0o4000, 0o2000, 0o1000,
];

pub struct Facts {
    pub kind: &'static str,
    pub target: Option<PathBuf>,
    pub len: u64,
    pub disk: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub modified: i64,
    pub accessed: i64,
    pub changed: i64,
    pub created: Option<i64>,
    pub symlink: bool,
}

pub fn read(path: &Path) -> io::Result<Facts> {
    let meta = fs::symlink_metadata(path)?;
    let symlink = meta.file_type().is_symlink();
    let kind = if symlink {
        "Link"
    } else if meta.is_dir() {
        "Directory"
    } else {
        "File"
    };
    let target = if symlink { fs::read_link(path).ok() } else { None };
    Ok(Facts {
        kind,
        target,
        len: meta.len(),
        // st_blocks counts 512-byte units.
        disk: meta.blocks().saturating_mul(512),
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        modified: unix_secs(meta.modified().ok()),
        accessed: unix_secs(meta.accessed().ok()),
        changed: meta.ctime(),
        created: meta.created().ok().and_then(|time| unix_opt(time)),
        symlink,
    })
}

pub fn apply_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
}

pub fn apply_owner(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad path"))?;
    let rc = unsafe { libc::chown(c.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path).ok()?.into_dimensions().ok()
}

pub fn mode_text(mode: u32) -> String {
    format!("{:o}", mode & 0o7777)
}

pub fn parse_mode(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mode = u32::from_str_radix(text, 8).ok()?;
    if mode > 0o7777 {
        return None;
    }
    Some(mode)
}

pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

pub fn size_line(n: u64) -> String {
    if n < 1024 {
        grouped(n)
    } else {
        format!("{} ({})", crate::listing::format_size(n), grouped(n))
    }
}

pub fn format_stamp(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
        "December",
    ];
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time = secs as libc::time_t;
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return String::new();
    }
    let Some(day) = DAYS.get(tm.tm_wday as usize) else { return String::new() };
    let Some(month) = MONTHS.get(tm.tm_mon as usize) else { return String::new() };
    let off = tm.tm_gmtoff;
    let sign = if off >= 0 { '+' } else { '-' };
    let off = off.abs();
    let hours = off / 3600;
    let mins = (off % 3600) / 60;
    format!(
        "{day}, {} {month} {:04} {:02}:{:02}:{:02} UT{sign}{hours:02}{mins:02}",
        tm.tm_mday,
        tm.tm_year + 1900,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

fn unix_secs(time: Option<SystemTime>) -> i64 {
    time.and_then(unix_opt).unwrap_or(0)
}

fn unix_opt(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(dur) => Some(dur.as_secs() as i64),
        Err(err) => Some(-(err.duration().as_secs() as i64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_round_trip() {
        assert_eq!(parse_mode("644"), Some(0o644));
        assert_eq!(parse_mode("0644"), Some(0o644));
        assert_eq!(parse_mode("4755"), Some(0o4755));
        assert_eq!(parse_mode("8"), None);
        assert_eq!(mode_text(0o644), "644");
        assert_eq!(mode_text(0o4755), "4755");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(grouped(10341483), "10,341,483");
        assert_eq!(grouped(12), "12");
    }

    #[test]
    fn mode_lands_on_the_file() {
        let dir = std::env::temp_dir().join(format!("rc-props-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        fs::write(&path, b"hi").unwrap();
        apply_mode(&path, 0o600).unwrap();
        let facts = read(&path).unwrap();
        assert_eq!(facts.mode & 0o777, 0o600);
        assert_eq!(facts.len, 2);
        assert_eq!(facts.kind, "File");
        assert!(!format_stamp(facts.modified).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
