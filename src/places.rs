use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::config::Bookmark;

const REAL_FS: &[&str] = &[
    "ext4", "ext3", "ext2", "btrfs", "xfs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "zfs",
    "f2fs", "bcachefs", "cifs", "smb3", "nfs", "nfs4", "fuse.sshfs",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub label: String,
    pub path: PathBuf,
    pub device: String,
    pub removable: bool,
    pub network: bool,
}

// Bytes: one mount point that is not UTF-8 must not hide all the others.
pub fn mounts() -> Vec<Mount> {
    let Ok(raw) = fs::read("/proc/mounts") else {
        return Vec::new();
    };
    parse_mount_bytes(&raw)
}

pub fn current_mount(mounts: &[&Path], dir: &Path) -> Option<usize> {
    mounts
        .iter()
        .enumerate()
        .filter(|(_, mount)| dir.starts_with(mount))
        .max_by_key(|(_, mount)| mount.components().count())
        .map(|(index, _)| index)
}

pub fn parse_mounts(text: &str) -> Vec<Mount> {
    parse_mount_bytes(text.as_bytes())
}

pub fn parse_mount_bytes(raw: &[u8]) -> Vec<Mount> {
    let mut out = Vec::new();
    let mut labels = std::collections::HashSet::new();
    let mut automounts = Vec::new();
    for line in raw.split(|&b| b == b'\n') {
        let Some((dev, path, fstype)) = fields(line) else { continue };
        if skip_mount(&path) {
            continue;
        }
        // Idle systemd automounts stay autofs until the directory is opened.
        if fstype == "autofs" {
            if user_automount(&path) {
                automounts.push((path, dev));
            }
            continue;
        }
        if !REAL_FS.contains(&fstype) || out.iter().any(|m: &Mount| m.path == path) {
            continue;
        }
        out.push(mount_at(path, &dev, fstype, &mut labels));
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
    let removable = path.starts_with("/media") || path.starts_with("/run/media");
    let network = matches!(fstype, "cifs" | "smb3" | "nfs" | "nfs4" | "fuse.sshfs");
    Mount {
        label: unique_label(&path, labels),
        path,
        device: dev.to_string(),
        removable,
        network,
    }
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
    let Ok(raw) = fs::read("/proc/mounts") else {
        return out;
    };
    for line in raw.split(|&b| b == b'\n') {
        let Some((_, path, fstype)) = fields(line) else { continue };
        if !REAL_FS.contains(&fstype) {
            continue;
        }
        if path.starts_with("/proc") || path.starts_with("/sys") || path.starts_with("/dev") || path.starts_with("/run") {
            continue;
        }
        if !out.iter().any(|(_, have)| have == &path) {
            out.push((path.display().to_string(), path));
        }
    }
    out
}

// Device, mount point and type of one /proc/mounts line.
fn fields(line: &[u8]) -> Option<(String, PathBuf, &str)> {
    let mut parts = line.split(u8::is_ascii_whitespace).filter(|part| !part.is_empty());
    let dev = String::from_utf8_lossy(parts.next()?).into_owned();
    let path = PathBuf::from(OsString::from_vec(unescape_mount(parts.next()?)));
    let fstype = std::str::from_utf8(parts.next()?).ok()?;
    Some((dev, path, fstype))
}

// The kernel writes space, tab, newline and backslash as \ooo.
fn unescape_mount(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let octal = raw.get(i + 1..i + 4).filter(|d| raw[i] == b'\\' && d.iter().all(|b| (b'0'..=b'7').contains(b)));
        match octal {
            Some(d) => {
                out.push(d.iter().fold(0u8, |n, b| n.wrapping_mul(8).wrapping_add(b - b'0')));
                i += 4;
            }
            None => {
                out.push(raw[i]);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn mount_point_that_is_not_utf8() {
        let mounts = parse_mount_bytes(b"/dev/sda1 /media/u/\xff\\040x vfat rw 0 0\n/dev/sdb1 /mnt/b ext4 rw 0 0\n");
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].path.as_os_str().as_bytes(), b"/media/u/\xff x");
        assert_eq!(mounts[1].path, Path::new("/mnt/b"));
    }
}
