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
    read_dir_while(path, || false)
}

// `stop` closes the directory fd before the next name.
pub fn read_dir_while(path: &Path, stop: impl Fn() -> bool) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    if path.parent().is_some() {
        out.push(Entry::dotdot());
    }
    for item in fs::read_dir(path)? {
        if stop() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let item = item?;
        let (name, raw) = match item.file_name().into_string() {
            Ok(name) => (name, None),
            Err(raw) => (raw.to_string_lossy().into_owned(), Some(raw.into_boxed_os_str())),
        };
        // statx on the directory fd, not a path walk from /. A file deleted meanwhile is left out.
        let meta = match item.metadata() {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let is_link = meta.file_type().is_symlink();
        let is_dir = if is_link {
            fs::metadata(item.path()).map(|m| m.is_dir()).unwrap_or(false)
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
            raw,
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
    let pattern = (filter.contains('*') || filter.contains('?')).then(|| filter.chars().collect::<Vec<_>>());
    let mut idxs: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name.as_ref() != "..")
        .filter(|(_, e)| show_hidden || !e.is_hidden())
        .filter(|(_, e)| {
            let name = e.name.to_lowercase();
            pattern.as_ref().map_or_else(|| name.contains(&filter), |pattern| wildcard_match(pattern, &name))
        })
        .map(|(i, _)| i)
        .collect();
    idxs.sort_by(|&a, &b| cmp_entries(&entries[a], &entries[b], sort, ascending));
    if entries.first().is_some_and(|e| e.name.as_ref() == "..") {
        idxs.insert(0, 0);
    }
    idxs
}

fn wildcard_match(pattern: &[char], name: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let (mut pattern_pos, mut name_pos) = (0, 0);
    let (mut star, mut retry_pos) = (None, 0);
    while name_pos < name.len() {
        if pattern_pos < pattern.len() && (pattern[pattern_pos] == '?' || pattern[pattern_pos] == name[name_pos]) {
            pattern_pos += 1;
            name_pos += 1;
        } else if pattern.get(pattern_pos) == Some(&'*') {
            star = Some(pattern_pos);
            pattern_pos += 1;
            retry_pos = name_pos;
        } else if let Some(star_pos) = star {
            retry_pos += 1;
            name_pos = retry_pos;
            pattern_pos = star_pos + 1;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_pos) == Some(&'*') {
        pattern_pos += 1;
    }
    pattern_pos == pattern.len()
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

// GTK symbolic icon names, resolved by the active icon theme.
pub fn file_icon(name: &str, is_dir: bool, is_link: bool) -> &'static str {
    if name == ".." {
        return "";
    }
    if is_link {
        return "insert-link-symbolic";
    }
    if is_dir {
        return "folder-symbolic";
    }
    match extension(name).to_ascii_lowercase().as_str() {
        "txt" | "md" | "log" => "text-x-generic-symbolic",
        "mp3" | "flac" | "wav" | "ogg" => "audio-x-generic-symbolic",
        "jpg" | "png" | "gif" | "webp" => "image-x-generic-symbolic",
        "mp4" | "mkv" | "webm" => "video-x-generic-symbolic",
        "zip" | "7z" | "rar" | "tar" | "gz" => "package-x-generic-symbolic",
        "pdf" => "text-x-generic-symbolic",
        "rs" | "py" | "js" | "ts" | "go" | "c" | "h" => "text-x-generic-symbolic",
        _ => "text-x-generic-symbolic",
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
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time = secs as libc::time_t;
    if !unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min
        );
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

pub fn column_chars(entries: &[Entry]) -> [usize; 5] {
    let mut name = 4;
    let mut ext = 3;
    let mut size = 4;
    let mut date = 4;
    let mut attr = 4;
    for entry in entries {
        if entry.name.as_ref() == ".." {
            continue;
        }
        name = name.max(shown_name_chars(entry));
        ext = ext.max(extension(&entry.name).len());
        let size_len = if entry.is_dir() { 5 } else { format_size(entry.size).len() };
        size = size.max(size_len);
        date = date.max(format_mtime(entry.mtime).len());
        attr = attr.max(format_attr(entry.mode).len());
    }
    [name, ext, size, date, attr]
}

fn shown_name_chars(entry: &Entry) -> usize {
    let suffix = usize::from(entry.is_dir() || entry.is_symlink());
    entry.name.chars().count() + suffix
}

pub fn column_pixels(chars: [usize; 5], unit: i32) -> [i32; 5] {
    let unit = unit.max(7);
    // Cell padding plus the label's horizontal margins. Date has a wider end margin.
    let pad = [28, 28, 28, 36, 28];
    let min = [96, 48, 64, 140, 96];
    let max = [640, 200, 140, 240, 160];
    let mut out = [0; 5];
    for i in 0..5 {
        out[i] = ((chars[i] as i32) * unit + pad[i]).clamp(min[i], max[i]);
    }
    out
}

pub fn entry_path(dir: &Path, entry: &Entry) -> std::path::PathBuf {
    if entry.name.as_ref() == ".." {
        dir.parent().unwrap_or(dir).to_path_buf()
    } else if let Some(raw) = &entry.raw {
        dir.join(&**raw)
    } else {
        dir.join(entry.name.as_ref())
    }
}
