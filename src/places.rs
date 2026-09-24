use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Bookmark;

const REAL_FS: &[&str] = &[
    "ext4", "ext3", "ext2", "btrfs", "xfs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "zfs",
    "f2fs", "bcachefs",
];

pub fn places(home: &Path, bookmarks: &[Bookmark]) -> Vec<(String, PathBuf)> {
    let mut out = vec![
        ("Home".into(), home.to_path_buf()),
        ("/".into(), PathBuf::from("/")),
    ];
    for mark in bookmarks {
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
