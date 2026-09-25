use std::fs;
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

pub fn mounts() -> Vec<Mount> {
    let Ok(text) = fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    parse_mounts(&text)
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
