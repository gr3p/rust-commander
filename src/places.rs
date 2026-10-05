use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Bookmark;

const REAL_FS: &[&str] = &[
    "ext4", "ext3", "ext2", "btrfs", "xfs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "zfs",
    "f2fs", "bcachefs", "cifs", "smb3", "nfs", "nfs4", "fuse.sshfs", "iso9660", "udf",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub label: String,
    pub path: PathBuf,
    pub device: String,
    pub removable: bool,
    pub network: bool,
    pub optical: bool,
}

pub fn mounts() -> Vec<Mount> {
    let Ok(text) = fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    parse_mounts(&text)
}

pub fn current_mount(mounts: &[&Path], dir: &Path) -> Option<usize> {
    mounts
        .iter()
        .enumerate()
        .filter(|(_, mount)| dir.starts_with(mount))
        .max_by_key(|(_, mount)| mount.components().count())
        .map(|(index, _)| index)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInfo {
    pub mount: PathBuf,
    pub device: String,
    pub fstype: String,
    pub options: String,
    pub label: Option<String>,
    pub uuid: Option<String>,
    pub total: u64,
    pub free: u64,
    // Free space an unprivileged user can use; smaller than `free` on ext4's reserved blocks.
    pub avail: u64,
    pub files: u64,
    pub files_free: u64,
}

impl DiskInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    // Share of the space taken, as the percentage `df` reports (used / (used + avail)).
    pub fn used_percent(&self) -> u8 {
        let used = self.used();
        let base = used + self.avail;
        if base == 0 { 0 } else { ((used as f64 / base as f64) * 100.0).ceil().min(100.0) as u8 }
    }
}

// One line for a tooltip: what is left and how full the disk is.
pub fn summary(info: &DiskInfo) -> String {
    use crate::listing::format_size;
    format!("{} free of {}  ({}% used)", format_size(info.avail), format_size(info.total), info.used_percent())
}

// The mount holding `dir`, read from /proc/mounts text: (mount point, device, fs type, options).
pub fn mount_entry(text: &str, dir: &Path) -> Option<(PathBuf, String, String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let dev = parts.next()?;
            let mnt = PathBuf::from(unescape_mount(parts.next()?));
            let fstype = parts.next()?;
            let opts = parts.next().unwrap_or("");
            dir.starts_with(&mnt).then(|| (mnt, dev.to_string(), fstype.to_string(), opts.to_string()))
        })
        .max_by_key(|(mnt, ..)| mnt.components().count())
}

pub fn disk_info(dir: &Path) -> std::io::Result<DiskInfo> {
    use std::os::unix::ffi::OsStrExt;
    let text = fs::read_to_string("/proc/mounts")?;
    let (mount, device, fstype, options) = mount_entry(&text, dir)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no mount found"))?;
    let c_path = std::ffi::CString::new(mount.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad path"))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let unit = (if st.f_frsize > 0 { st.f_frsize } else { st.f_bsize }) as u64;
    let (label, uuid) = (by_link("/dev/disk/by-label", &device), by_link("/dev/disk/by-uuid", &device));
    Ok(DiskInfo {
        mount,
        device,
        fstype,
        options,
        label,
        uuid,
        total: st.f_blocks as u64 * unit,
        free: st.f_bfree as u64 * unit,
        avail: st.f_bavail as u64 * unit,
        files: st.f_files as u64,
        files_free: st.f_ffree as u64,
    })
}

// udev names a device by label or UUID with a symlink to it; find the name that points at `device`.
fn by_link(dir: &str, device: &str) -> Option<String> {
    let target = fs::canonicalize(device).ok()?;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        if fs::canonicalize(entry.path()).ok().as_deref() == Some(target.as_path()) {
            return Some(unescape_udev(&entry.file_name().to_string_lossy()));
        }
    }
    None
}

// udev writes unsafe characters in link names as \xNN.
fn unescape_udev(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'x'))
            .then(|| name.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mounts(text: &str) -> Vec<Mount> {
    let mut out = Vec::new();
    let mut labels = std::collections::HashSet::new();
    let mut automounts = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(dev) = parts.next() else { continue };
        let Some(mnt) = parts.next() else { continue };
        let Some(fstype) = parts.next() else { continue };
        let path = PathBuf::from(unescape_mount(mnt));
        if skip_mount(&path) {
            continue;
        }
        // Idle systemd automounts stay autofs until the directory is opened.
        if fstype == "autofs" {
            if user_automount(&path) {
                automounts.push((path, dev.to_string()));
            }
            continue;
        }
        if !REAL_FS.contains(&fstype) || out.iter().any(|m: &Mount| m.path == path) {
            continue;
        }
        out.push(mount_at(path, dev, fstype, &mut labels));
    }
    for (path, dev) in automounts {
        if out.iter().any(|m| m.path == path) {
            continue;
        }
        out.push(mount_at(path, &dev, "autofs", &mut labels));
    }
    out.sort_by(|a, b| (a.path != Path::new("/")).cmp(&(b.path != Path::new("/"))).then(a.path.cmp(&b.path)));
    out
}

fn mount_at(path: PathBuf, dev: &str, fstype: &str, labels: &mut std::collections::HashSet<String>) -> Mount {
    let optical = is_optical_device(dev);
    let removable = optical || path.starts_with("/media") || path.starts_with("/run/media");
    let network = matches!(fstype, "cifs" | "smb3" | "nfs" | "nfs4" | "fuse.sshfs");
    Mount {
        label: unique_label(&path, labels),
        path,
        device: dev.to_string(),
        removable,
        network,
        optical,
    }
}

pub fn is_optical_device(dev: &str) -> bool {
    dev.starts_with("/dev/sr") || dev.starts_with("/dev/scd")
}

fn user_automount(path: &Path) -> bool {
    path.starts_with("/mnt") || path.starts_with("/media") || path.starts_with("/run/media")
}

fn skip_mount(path: &Path) -> bool {
    path.starts_with("/proc")
        || path.starts_with("/sys")
        || path.starts_with("/dev")
        || path.starts_with("/boot")
        || (path.starts_with("/run") && !path.starts_with("/run/media"))
}

fn unique_label(path: &Path, used: &mut std::collections::HashSet<String>) -> String {
    let base = if path == Path::new("/") {
        "Root".into()
    } else {
        path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
    };
    if used.insert(base.clone()) {
        base
    } else {
        let full = path.display().to_string();
        used.insert(full.clone());
        full
    }
}

pub fn places(home: &Path, bookmarks: &[Bookmark]) -> Vec<(String, PathBuf)> {
    let mut out = vec![
        ("Home".into(), home.to_path_buf()),
        ("/".into(), PathBuf::from("/")),
    ];
    for mark in bookmarks {
        if mark.path.as_os_str().is_empty() || mark.kind == "menu" || mark.kind == "sep" || mark.kind == "end" {
            continue;
        }
        if !out.iter().any(|(_, path)| path == &mark.path) {
            out.push((mark.name.clone(), mark.path.clone()));
        }
    }
    let Ok(text) = fs::read_to_string("/proc/mounts") else {
        return out;
    };
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _dev = parts.next();
        let Some(mnt) = parts.next() else { continue };
        let Some(fstype) = parts.next() else { continue };
        if !REAL_FS.contains(&fstype) {
            continue;
        }
        let path = PathBuf::from(unescape_mount(mnt));
        if path.starts_with("/proc") || path.starts_with("/sys") || path.starts_with("/dev") || path.starts_with("/run") {
            continue;
        }
        if !out.iter().any(|(_, have)| have == &path) {
            out.push((path.display().to_string(), path));
        }
    }
    out
}

fn unescape_mount(raw: &str) -> String {
    raw.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}
