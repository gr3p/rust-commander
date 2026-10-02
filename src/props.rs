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
    // A regular file, or a link to one. Only these are opened for image details.
    pub regular: bool,
    pub dev: u64,
    pub ino: u64,
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
        regular: meta.is_file() || (symlink && fs::metadata(path).is_ok_and(|meta| meta.is_file())),
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

// Touches only this one inode, never children. It must still be the inode
// that `facts` was read from, and not a link, or nothing is written.
pub fn apply(path: &Path, facts: &Facts, mode: u32, uid: u32, gid: u32) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    if facts.symlink {
        return Err(io::Error::other("A link has no permissions of its own"));
    }
    let owner = uid != facts.uid || gid != facts.gid;
    if !owner && mode == facts.mode {
        return Ok(());
    }
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::other("Bad path"))?;
    // O_PATH|O_NOFOLLOW pins the inode without following a link swapped in after the dialog opened.
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if st.st_mode & libc::S_IFMT == libc::S_IFLNK || st.st_dev as u64 != facts.dev || st.st_ino as u64 != facts.ino {
        return Err(io::Error::other("The file changed since the dialog opened"));
    }
    if owner {
        // -1 leaves that id as it is.
        let uid = if uid == facts.uid { libc::uid_t::MAX } else { uid };
        let gid = if gid == facts.gid { libc::gid_t::MAX } else { gid };
        if unsafe { libc::fchownat(fd.as_raw_fd(), c"".as_ptr(), uid, gid, libc::AT_EMPTY_PATH) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // chown clears SUID/SGID, so the mode the dialog shows goes on last.
    // /proc/self/fd/N resolves to the pinned inode, not to a path that can be swapped.
    let proc = std::ffi::CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd())).map_err(|_| io::Error::other("Bad path"))?;
    if unsafe { libc::chmod(proc.as_ptr(), (mode & 0o7777) as libc::mode_t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// Runs on the UI thread. O_NONBLOCK and the type check after open keep a pipe or device from hanging it.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut reader = image::ImageReader::new(io::BufReader::new(file));
    match image::ImageFormat::from_path(path) {
        Ok(format) => reader.set_format(format),
        Err(_) => reader = reader.with_guessed_format().ok()?,
    }
    reader.into_dimensions().ok()
}

pub fn mode_text(mode: u32) -> String {
    format!("{:o}", mode & 0o7777)
}

// Three or four octal digits, so a half typed "64" is never taken as 0o064.
pub fn parse_mode(text: &str) -> Option<u32> {
    let text = text.trim();
    if !(3..=4).contains(&text.len()) || !text.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return None;
    }
    u32::from_str_radix(text, 8).ok()
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
        assert_eq!(parse_mode("64"), None);
        assert_eq!(parse_mode("1"), None);
        assert_eq!(parse_mode("648"), None);
        assert_eq!(parse_mode("+644"), None);
        assert_eq!(parse_mode("07777"), None);
        assert_eq!(mode_text(0o644), "644");
        assert_eq!(mode_text(0o4755), "4755");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(grouped(10341483), "10,341,483");
        assert_eq!(grouped(12), "12");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rc-props-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn mode_lands_on_the_file() {
        let dir = scratch("mode");
        let path = dir.join("a.txt");
        fs::write(&path, b"hi").unwrap();
        let facts = read(&path).unwrap();
        apply(&path, &facts, 0o600, facts.uid, facts.gid).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(facts.len, 2);
        assert_eq!(facts.kind, "File");
        assert!(!format_stamp(facts.modified).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_mode_leaves_children_alone() {
        let dir = scratch("tree");
        let sub = dir.join("sub");
        fs::create_dir(&sub).unwrap();
        let child = sub.join("child.txt");
        fs::write(&child, b"x").unwrap();
        apply_test_mode(&child, 0o644);
        let facts = read(&sub).unwrap();
        apply(&sub, &facts, 0o700, facts.uid, facts.gid).unwrap();
        assert_eq!(mode(&sub), 0o700);
        assert_eq!(mode(&child), 0o644);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn swapped_for_a_link_is_refused() {
        let dir = scratch("swap");
        let path = dir.join("a.txt");
        let secret = dir.join("secret");
        fs::write(&path, b"x").unwrap();
        fs::write(&secret, b"s").unwrap();
        apply_test_mode(&secret, 0o600);
        let facts = read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&secret, &path).unwrap();
        assert!(apply(&path, &facts, 0o666, facts.uid, facts.gid).is_err());
        assert_eq!(mode(&secret), 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replaced_file_is_refused() {
        let dir = scratch("replace");
        let path = dir.join("a.txt");
        fs::write(&path, b"x").unwrap();
        let facts = read(&path).unwrap();
        let other = dir.join("b.txt");
        fs::write(&other, b"y").unwrap();
        apply_test_mode(&other, 0o600);
        fs::rename(&other, &path).unwrap();
        assert!(apply(&path, &facts, 0o666, facts.uid, facts.gid).is_err());
        assert_eq!(mode(&path), 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_itself_is_never_written() {
        let dir = scratch("link");
        let target = dir.join("target");
        fs::write(&target, b"t").unwrap();
        apply_test_mode(&target, 0o600);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let facts = read(&link).unwrap();
        assert!(apply(&link, &facts, 0o666, facts.uid, facts.gid).is_err());
        assert_eq!(mode(&target), 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn setuid_survives_a_group_change() {
        let dir = scratch("suid");
        let path = dir.join("run");
        fs::write(&path, b"x").unwrap();
        let facts = read(&path).unwrap();
        let mut ids = [0 as libc::gid_t; 64];
        let n = unsafe { libc::getgroups(ids.len() as i32, ids.as_mut_ptr()) };
        let Some(&gid) = ids[..n.max(0) as usize].iter().find(|&&gid| gid != facts.gid) else {
            let _ = fs::remove_dir_all(&dir);
            return;
        };
        apply(&path, &facts, 0o4755, facts.uid, gid).unwrap();
        assert_eq!(mode(&path), 0o4755);
        assert_eq!(fs::metadata(&path).unwrap().gid(), gid);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pipe_named_like_an_image_does_not_block() {
        let dir = scratch("fifo");
        let path = dir.join("x.png");
        let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(!read(&path).unwrap().regular);
        assert_eq!(dimensions(&path), None);
        let _ = fs::remove_dir_all(&dir);
    }

    fn apply_test_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
}
