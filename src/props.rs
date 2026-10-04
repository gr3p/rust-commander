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
    let mode = mode & 0o7777;
    let dir = st.st_mode & libc::S_IFMT == libc::S_IFDIR;
    if unsafe { libc::chmod(proc.as_ptr(), mode as libc::mode_t) } != 0 {
        let err = io::Error::last_os_error();
        return Err(mode_error(path, err.kind(), &err.to_string(), errno_why(path, dir, &err)));
    }
    // CIFS nounix and FAT report success and keep the old mode; the kernel also drops SGID for a foreign group.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stored = (st.st_mode & 0o7777) as u32;
    if stored != mode {
        let lead = format!("The filesystem kept {} instead of {}", mode_text(stored), mode_text(mode));
        return Err(mode_error(path, io::ErrorKind::Other, &lead, mode_why(path, dir, Some(stored), mode)));
    }
    Ok(())
}

fn mode_error(path: &Path, kind: io::ErrorKind, lead: &str, why: String) -> io::Error {
    let message = format!("{lead}. {why}");
    crate::log::error(&format!("chmod {}: {message}", path.display()));
    io::Error::new(kind, message)
}

struct MountInfo {
    point: PathBuf,
    fstype: String,
    source: String,
    flags: String,
    opts: String,
}

fn errno_why(path: &Path, dir: bool, err: &io::Error) -> String {
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mounts = parse_mountinfo(&text);
    let mount = covering(&mounts, path);
    if err.raw_os_error() == Some(libc::EROFS) || mount.is_some_and(is_read_only) {
        return "The filesystem is mounted read-only, so the mode cannot change.".into();
    }
    if let Some(why) = mount.and_then(|mount| fixed_mode_why(mount, dir)) {
        return why;
    }
    if err.kind() == io::ErrorKind::PermissionDenied {
        return "Only the owner can change the mode. A parent directory without search permission, or an immutable file, causes this too.".into();
    }
    if matches!(err.raw_os_error(), Some(code) if code == libc::EOPNOTSUPP || code == libc::ENOTSUP || code == libc::ENOSYS) {
        return "This filesystem does not support chmod. Usual causes: SMB with nounix, or FAT, exFAT and NTFS.".into();
    }
    explain_mode(mount, dir, None, 0)
}

fn mode_why(path: &Path, dir: bool, stored: Option<u32>, wanted: u32) -> String {
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mounts = parse_mountinfo(&text);
    explain_mode(covering(&mounts, path), dir, stored, wanted)
}

// The sentence after the error. Specific when the mount is a known case, otherwise the usual causes.
fn explain_mode(mount: Option<&MountInfo>, dir: bool, stored: Option<u32>, wanted: u32) -> String {
    if let Some(mount) = mount {
        if is_read_only(mount) {
            return "The filesystem is mounted read-only, so the mode cannot change.".into();
        }
        if let Some(why) = fixed_mode_why(mount, dir) {
            return why;
        }
    }
    if let Some(stored) = stored {
        if stored & 0o777 == wanted & 0o777 {
            if let Some(why) = special_bit_why(stored, wanted) {
                return why;
            }
        }
        if mount.is_some_and(|mount| is_unix_fs(&mount.fstype)) {
            return "The mode did not stick. On this filesystem that is usually an immutable file, or the file changed while it was being saved.".into();
        }
    }
    "Usual causes: SMB mounted with nounix and a fixed file_mode or dir_mode, or FAT, exFAT and NTFS, which have no Unix mode per file. A mount option such as file_mode, fmask, dmask or umask then overrides chmod.".into()
}

fn fixed_mode_why(mount: &MountInfo, dir: bool) -> Option<String> {
    let kind = mount.fstype.as_str();
    if matches!(kind, "cifs" | "smb3" | "smb") {
        let nounix = has_opt(&mount.opts, "nounix");
        let key = if dir { "dir_mode" } else { "file_mode" };
        let fixed = opt_value(&mount.opts, key);
        let how = match (nounix, fixed) {
            (false, None) => {
                return Some("The SMB server did not keep this mode. That happens when the share is mounted without Unix extensions.".into());
            }
            (true, Some(fixed)) => format!("nounix and {key}={fixed}"),
            (true, None) => "nounix".into(),
            (false, Some(fixed)) => format!("{key}={fixed}"),
        };
        return Some(format!(
            "This SMB share ({}) does not store a permission on each file. It is mounted with {how}, so chmod cannot change this one. posix or modefromsid works only if the server supports it.",
            mount.source
        ));
    }
    if matches!(kind, "vfat" | "msdos" | "exfat") {
        return Some("FAT and exFAT have no Unix permissions. The mode you see comes from the mount (umask, fmask or dmask), not from the file.".into());
    }
    if matches!(kind, "ntfs" | "ntfs3") || kind.contains("ntfs") {
        return Some("This NTFS mount does not keep a Unix mode per file. It comes from the mount options (permissions, fmask, dmask or umask).".into());
    }
    if kind == "fuseblk" {
        return Some("This volume does not keep a Unix mode per file. fuseblk is usually NTFS or exFAT, and the mode comes from the mount (permissions, fmask, dmask or umask).".into());
    }
    if matches!(kind, "nfs" | "nfs4") {
        return Some("The NFS server did not keep this mode. That is common with root_squash, all_squash, or an export without ACL support.".into());
    }
    if kind == "fuse" || kind.starts_with("fuse.") {
        return Some("This FUSE filesystem did not keep the mode. Many of them only pretend to have Unix permissions.".into());
    }
    None
}

fn special_bit_why(stored: u32, wanted: u32) -> Option<String> {
    let lost = wanted & !stored & 0o7000;
    if lost == 0 {
        return None;
    }
    let mut names = Vec::new();
    if lost & 0o4000 != 0 {
        names.push("setuid");
    }
    if lost & 0o2000 != 0 {
        names.push("setgid");
    }
    if lost & 0o1000 != 0 {
        names.push("the sticky bit");
    }
    let names = names.join(" and ");
    let group = if lost & 0o2000 != 0 {
        " Setgid stays only when you are a member of the file's group."
    } else {
        ""
    };
    Some(format!("The read, write and execute bits were stored. The kernel dropped {names}.{group}"))
}

fn is_unix_fs(kind: &str) -> bool {
    matches!(kind, "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "tmpfs" | "bcachefs" | "f2fs" | "zfs")
}

fn is_read_only(mount: &MountInfo) -> bool {
    has_opt(&mount.flags, "ro") || has_opt(&mount.opts, "ro")
}

fn has_opt(opts: &str, key: &str) -> bool {
    opts.split(',').any(|opt| opt == key)
}

fn opt_value<'a>(opts: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    opts.split(',').find_map(|opt| opt.strip_prefix(&prefix))
}

fn covering<'a>(mounts: &'a [MountInfo], path: &Path) -> Option<&'a MountInfo> {
    let mut best: Option<&MountInfo> = None;
    for mount in mounts {
        if !path.starts_with(&mount.point) {
            continue;
        }
        let take = match best {
            None => true,
            Some(cur) if mount.point.components().count() > cur.point.components().count() => true,
            Some(cur) if mount.point == cur.point && cur.fstype == "autofs" => true,
            _ => false,
        };
        if take {
            best = Some(mount);
        }
    }
    best
}

fn parse_mountinfo(text: &str) -> Vec<MountInfo> {
    text.lines().filter_map(parse_mount_line).collect()
}

fn parse_mount_line(line: &str) -> Option<MountInfo> {
    let mut parts = line.split(' ');
    let _id = parts.next()?;
    let _parent = parts.next()?;
    let _dev = parts.next()?;
    let _root = parts.next()?;
    let point = unescape_field(parts.next()?);
    let flags = (*parts.next()?).to_string();
    let rest: Vec<&str> = parts.collect();
    let dash = rest.iter().position(|part| *part == "-")?;
    let fstype = (*rest.get(dash + 1)?).to_string();
    let source = unescape_field(rest.get(dash + 2)?).to_string_lossy().into_owned();
    let opts = (*rest.get(dash + 3)?).to_string();
    Some(MountInfo { point, fstype, source, flags, opts })
}

// \040 is a space. Mountinfo escapes a few bytes as three octal digits.
fn unescape_field(field: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 4]).unwrap_or(""), 8) {
                out.push(value);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path).ok()?.into_dimensions().ok()
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

    #[test]
    fn stuck_mode_explains_the_common_causes() {
        let text = "\
74 33 0:40 / /mnt/seagate12tb rw - autofs systemd-1 rw,fd=58
478 74 0:56 / /mnt/seagate12tb rw - cifs //smb.lan/seagate12tb rw,nounix,file_mode=0755,dir_mode=0755
20 1 8:2 / /mnt/fat rw - vfat /dev/sdb1 rw,fmask=0111
21 1 8:3 / /mnt/ntfs rw - ntfs3 /dev/sdc1 rw
22 1 8:4 / /mnt/ro ro - ext4 /dev/sdd1 ro
10 1 8:1 / /mnt/other rw - ext4 /dev/sda1 rw
";
        let mounts = parse_mountinfo(text);
        let smb = explain_mode(covering(&mounts, Path::new("/mnt/seagate12tb/tmp/test.sh")), false, Some(0o755), 0o754);
        assert!(smb.contains("SMB"), "{smb}");
        assert!(smb.contains("nounix"), "{smb}");
        assert!(smb.contains("file_mode=0755"), "{smb}");
        let dir = explain_mode(covering(&mounts, Path::new("/mnt/seagate12tb/tmp")), true, Some(0o755), 0o700);
        assert!(dir.contains("dir_mode=0755"), "{dir}");
        let fat = explain_mode(covering(&mounts, Path::new("/mnt/fat/a")), false, Some(0o755), 0o644);
        assert!(fat.contains("FAT"), "{fat}");
        let ntfs = explain_mode(covering(&mounts, Path::new("/mnt/ntfs/a")), false, Some(0o777), 0o644);
        assert!(ntfs.contains("NTFS"), "{ntfs}");
        let ro = explain_mode(covering(&mounts, Path::new("/mnt/ro/a")), false, Some(0o644), 0o600);
        assert!(ro.contains("read-only"), "{ro}");
        let local = explain_mode(covering(&mounts, Path::new("/mnt/other/a")), false, Some(0o755), 0o2755);
        assert!(local.contains("setgid"), "{local}");
        let unknown = explain_mode(None, false, Some(0o755), 0o644);
        assert!(unknown.contains("FAT"), "{unknown}");
        assert!(unknown.contains("nounix"), "{unknown}");
        let spaced = parse_mount_line("1 0 0:1 / /mnt/my\\040share rw - cifs //srv/my\\040share rw,nounix,file_mode=0755").unwrap();
        assert_eq!(spaced.point, PathBuf::from("/mnt/my share"));
        assert!(spaced.source.contains("my share"));
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

    fn apply_test_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
}
