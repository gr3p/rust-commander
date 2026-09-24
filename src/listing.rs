use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::entry::{Entry, DIR, HIDDEN, LINK};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Ext,
    Time,
    Size,
    Attr,
}

impl SortKey {
    pub fn from_config(s: &str) -> Self {
        match s {
            "ext" => Self::Ext,
            "time" => Self::Time,
            "size" => Self::Size,
            "attr" => Self::Attr,
            _ => Self::Name,
        }
    }

    pub fn as_config(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Ext => "ext",
            Self::Time => "time",
            Self::Size => "size",
            Self::Attr => "attr",
        }
    }
}

pub fn read_dir(path: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    if path.parent().is_some() {
        out.push(Entry::dotdot());
    }
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name();
        let name = name.to_string_lossy();
        let full = item.path();
        let meta = fs::symlink_metadata(&full)?;
        let is_link = meta.file_type().is_symlink();
        let is_dir = if is_link {
            fs::metadata(&full).map(|m| m.is_dir()).unwrap_or(false)
        } else {
            meta.is_dir()
        };
        let mut flags = 0;
        if is_dir {
            flags |= DIR;
        }
        if is_link {
            flags |= LINK;
        }
        if name.starts_with('.') {
            flags |= HIDDEN;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(Entry {
            name: name.into(),
            size: if is_dir { 0 } else { meta.len() },
            mtime,
            mode: meta.permissions().mode(),
            flags,
        });
    }
    Ok(out)
}

pub fn visible_indices(
    entries: &[Entry],
    sort: SortKey,
    ascending: bool,
    show_hidden: bool,
    filter: &str,
) -> Vec<usize> {
    let filter = filter.to_lowercase();
    let mut idxs: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name.as_ref() != "..")
        .filter(|(_, e)| show_hidden || !e.is_hidden())
        .filter(|(_, e)| filter.is_empty() || e.name.to_lowercase().contains(&filter))
        .map(|(i, _)| i)
        .collect();
    idxs.sort_by(|&a, &b| cmp_entries(&entries[a], &entries[b], sort, ascending));
    if entries.first().is_some_and(|e| e.name.as_ref() == "..") {
        idxs.insert(0, 0);
    }
    idxs
}

fn cmp_entries(a: &Entry, b: &Entry, sort: SortKey, ascending: bool) -> std::cmp::Ordering {
    let dir = b.is_dir().cmp(&a.is_dir());
    if dir != std::cmp::Ordering::Equal {
        return dir;
    }
    let ord = match sort {
        SortKey::Name => cmp_name(&a.name, &b.name),
        SortKey::Ext => extension(&a.name).cmp(extension(&b.name)).then(cmp_name(&a.name, &b.name)),
        SortKey::Time => a.mtime.cmp(&b.mtime).then(cmp_name(&a.name, &b.name)),
        SortKey::Size => a.size.cmp(&b.size).then(cmp_name(&a.name, &b.name)),
        SortKey::Attr => (a.mode & 0o7777).cmp(&(b.mode & 0o7777)).then(cmp_name(&a.name, &b.name)),
    };
    if ascending { ord } else { ord.reverse() }
}

fn cmp_name(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

pub fn extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() => ext,
        _ => "",
    }
}

// st_mode: file type, then rwx for user, group and other.
pub fn format_attr(mode: u32) -> String {
    if mode == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(10);
    out.push(match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        _ => '-',
    });
    for (bit, ch) in [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ] {
        out.push(if mode & bit != 0 { ch } else { '-' });
    }
    out
}

pub fn format_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let n = n as f64;
    if n < K {
        format!("{}", n as u64)
    } else if n < K * K {
        format!("{:.1} K", n / K)
    } else if n < K * K * K {
        format!("{:.1} M", n / (K * K))
    } else {
        format!("{:.1} G", n / (K * K * K))
    }
}

pub fn format_mtime(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_unix_days(days);
    let h = tod / 3600;
    let min = (tod % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}")
}

// Howard Hinnant: days since Unix epoch to a civil date.
fn civil_from_unix_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

pub fn entry_path(dir: &Path, entry: &Entry) -> std::path::PathBuf {
    if entry.name.as_ref() == ".." {
        dir.parent().unwrap_or(dir).to_path_buf()
    } else {
        dir.join(entry.name.as_ref())
    }
}
