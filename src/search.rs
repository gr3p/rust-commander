use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct Options {
    pub root: PathBuf,
    pub query: String,
    pub recursive: bool,
    pub hidden: bool,
    pub files_only: bool,
    pub case_sensitive: bool,
    pub text: String,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub min_mtime: Option<i64>,
    pub max_mtime: Option<i64>,
    pub duplicates: bool,
    pub in_archives: bool,
}

#[derive(Debug)]
pub enum Event {
    Matches(Vec<PathBuf>),
    Finished { errors: usize, cancelled: bool },
}

// Each result is a row of widgets, about 10 KB; a one-letter search from / would take gigabytes.
pub const MAX_RESULTS: usize = 10_000;

pub fn run(options: Options, cancelled: impl Fn() -> bool, mut emit: impl FnMut(Event)) {
    let stop = AtomicBool::new(false);
    let cancelled = |stop: &AtomicBool| cancelled() || stop.load(Ordering::Relaxed);
    if options.duplicates {
        run_duplicates(&options, &cancelled, &stop, &mut emit);
        return;
    }
    let needle = fold(&options.query, options.case_sensitive);
    let pattern = (needle.contains('*') || needle.contains('?')).then(|| needle.chars().collect::<Vec<_>>());
    // File bytes are folded per byte, so the needle must be too.
    let text_needle = if options.case_sensitive { options.text.clone() } else { options.text.to_ascii_lowercase() };
    let mut stack = vec![options.root.clone()];
    let mut errors = 0;
    let mut found = 0;
    let mut matches = Vec::with_capacity(64);
    let mut seen_archives = std::collections::HashSet::new();
    while let Some(dir) = stack.pop() {
        if cancelled(&stop) {
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        for entry in entries {
            if cancelled(&stop) || found == MAX_RESULTS {
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !options.hidden && name.starts_with('.') {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            if kind.is_dir() && options.recursive {
                stack.push(path.clone());
            }
            let name_ok = name_matches(&name, &needle, pattern.as_deref(), options.case_sensitive);
            let meta_ok = meta_ok(&path, &options);
            if !meta_ok {
                continue;
            }
            if kind.is_dir() || kind.is_symlink() {
                if options.files_only || !options.text.is_empty() || !name_ok {
                    continue;
                }
                push_match(&mut matches, &mut emit, path, &mut found);
                continue;
            }
            // Opening a FIFO or device would block.
            let content_ok = options.text.is_empty() || kind.is_file() && file_contains(&path, text_needle.as_bytes(), !options.case_sensitive, &cancelled, &stop);
            let mut hit = name_ok && content_ok;
            if !hit && options.in_archives && crate::archive::is_archive(&path) && seen_archives.insert(path.clone()) {
                match crate::archive::archive_hit(&path, &options.query, &options.text, options.case_sensitive, &|| cancelled(&stop)) {
                    Ok(true) => hit = true,
                    Ok(false) => {}
                    Err(crate::archive::Fail::Cancelled) => stop.store(true, Ordering::Relaxed),
                    Err(_) => errors += 1,
                }
            }
            if !hit {
                continue;
            }
            push_match(&mut matches, &mut emit, path, &mut found);
        }
        flush(&mut matches, &mut emit);
        if found == MAX_RESULTS {
            break;
        }
    }
    emit(Event::Finished { errors, cancelled: cancelled(&stop) });
}

fn run_duplicates(options: &Options, cancelled: &impl Fn(&AtomicBool) -> bool, stop: &AtomicBool, emit: &mut impl FnMut(Event)) {
    let needle = fold(&options.query, options.case_sensitive);
    let pattern = (needle.contains('*') || needle.contains('?')).then(|| needle.chars().collect::<Vec<_>>());
    let mut stack = vec![options.root.clone()];
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut errors = 0;
    while let Some(dir) = stack.pop() {
        if cancelled(stop) {
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        for entry in entries {
            if cancelled(stop) {
                break;
            }
            let Ok(entry) = entry else {
                errors += 1;
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !options.hidden && name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                errors += 1;
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && options.recursive {
                stack.push(path.clone());
            }
            if !kind.is_file() {
                continue;
            }
            if !name_matches(&name, &needle, pattern.as_deref(), options.case_sensitive) || !meta_ok(&path, options) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                errors += 1;
                continue;
            };
            by_size.entry(meta.len()).or_default().push(path);
        }
    }
    let mut found = 0;
    let mut matches = Vec::new();
    for (_size, group) in by_size {
        if group.len() < 2 || found == MAX_RESULTS || cancelled(stop) {
            continue;
        }
        let mut by_hash: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for path in group {
            if cancelled(stop) {
                break;
            }
            match hash_file(&path, &|| cancelled(stop)) {
                Ok(hash) => by_hash.entry(hash).or_default().push(path),
                Err(_) => errors += 1,
            }
        }
        for (_hash, paths) in by_hash {
            if paths.len() < 2 {
                continue;
            }
            for path in paths {
                if found == MAX_RESULTS {
                    break;
                }
                push_match(&mut matches, emit, path, &mut found);
            }
        }
    }
    flush(&mut matches, emit);
    emit(Event::Finished { errors, cancelled: cancelled(stop) });
}

fn push_match(matches: &mut Vec<PathBuf>, emit: &mut impl FnMut(Event), path: PathBuf, found: &mut usize) {
    matches.push(path);
    *found += 1;
    if matches.len() == 64 {
        emit(Event::Matches(std::mem::take(matches)));
    }
}

fn flush(matches: &mut Vec<PathBuf>, emit: &mut impl FnMut(Event)) {
    if !matches.is_empty() {
        emit(Event::Matches(std::mem::take(matches)));
    }
}

fn fold(text: &str, case_sensitive: bool) -> String {
    if case_sensitive { text.to_string() } else { text.to_lowercase() }
}

fn name_matches(name: &str, needle: &str, pattern: Option<&[char]>, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    let shown = if case_sensitive { name.to_string() } else { name.to_lowercase() };
    match pattern {
        Some(pattern) => crate::listing::wildcard_match(pattern, &shown),
        None => shown.contains(needle),
    }
}

fn meta_ok(path: &Path, options: &Options) -> bool {
    if options.min_size.is_none() && options.max_size.is_none() && options.min_mtime.is_none() && options.max_mtime.is_none() {
        return true;
    }
    let Ok(meta) = fs::symlink_metadata(path) else { return false };
    if meta.is_dir() && (options.min_size.is_some() || options.max_size.is_some()) {
        return false;
    }
    if meta.is_dir() && options.min_mtime.is_none() && options.max_mtime.is_none() {
        return true;
    }
    let size = meta.len();
    if options.min_size.is_some_and(|min| size < min) || options.max_size.is_some_and(|max| size > max) {
        return false;
    }
    let modified = meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
    if options.min_mtime.is_some_and(|min| modified < min) || options.max_mtime.is_some_and(|max| modified > max) {
        return false;
    }
    true
}

fn file_contains(path: &Path, needle: &[u8], fold: bool, cancelled: &impl Fn(&AtomicBool) -> bool, stop: &AtomicBool) -> bool {
    if needle.is_empty() {
        return true;
    }
    let Ok(mut file) = File::open(path) else { return false };
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry = Vec::new();
    let mut first = true;
    loop {
        if cancelled(stop) {
            return false;
        }
        let Ok(n) = file.read(&mut buf) else { return false };
        if n == 0 {
            return false;
        }
        if first && buf[..n].contains(&0) {
            return false;
        }
        first = false;
        if window_has(&carry, &buf[..n], needle, fold) {
            return true;
        }
        // Keep the tail across reads, also when a read is shorter than the needle.
        carry.extend_from_slice(&buf[..n]);
        let cut = carry.len().saturating_sub(needle.len().saturating_sub(1));
        carry.drain(..cut);
    }
}

fn window_has(carry: &[u8], next: &[u8], needle: &[u8], fold: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut hay = Vec::with_capacity(carry.len() + next.len());
    hay.extend_from_slice(carry);
    hay.extend_from_slice(next);
    if hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|window| {
        if fold {
            window.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b)
        } else {
            window == needle
        }
    })
}

fn hash_file(path: &Path, cancelled: &dyn Fn() -> bool) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if cancelled() {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> (PathBuf, Options) {
        let dir = std::env::temp_dir().join(format!("rc-search-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let options = opts(&dir);
        (dir, options)
    }

    fn opts(dir: &Path) -> Options {
        Options {
            root: dir.to_path_buf(),
            query: String::new(),
            recursive: true,
            hidden: false,
            files_only: true,
            case_sensitive: false,
            text: String::new(),
            min_size: None,
            max_size: None,
            min_mtime: None,
            max_mtime: None,
            duplicates: false,
            in_archives: false,
        }
    }

    fn hits(options: Options) -> Vec<PathBuf> {
        let mut found = Vec::new();
        run(options, || false, |event| {
            if let Event::Matches(paths) = event {
                found.extend(paths);
            }
        });
        found
    }

    #[test]
    fn finds_text_and_size() {
        let (dir, mut options) = base("text");
        fs::write(dir.join("a.txt"), b"alpha secret").unwrap();
        fs::write(dir.join("b.txt"), b"beta").unwrap();
        options.text = "secret".into();
        let found = hits(options);
        assert_eq!(found, vec![dir.join("a.txt")]);
        let mut options = opts(&dir);
        options.min_size = Some(5);
        options.max_size = Some(5);
        fs::write(dir.join("c.txt"), b"abcde").unwrap();
        let found = hits(options);
        assert!(found.iter().any(|path| path.ends_with("c.txt")));
        assert!(!found.iter().any(|path| path.ends_with("b.txt")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_search_skips_fifos_and_matches_non_ascii_and_short_chunks() {
        let (dir, mut options) = base("fifo");
        let fifo = std::ffi::CString::new(dir.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        fs::write(dir.join("a.txt"), "Blåbär och Secret").unwrap();
        options.files_only = false;
        options.text = "blåbär".into();
        options.case_sensitive = true;
        assert!(hits(options).is_empty());
        let mut options = opts(&dir);
        options.files_only = false;
        options.case_sensitive = true;
        options.text = "Blåbär".into();
        assert_eq!(hits(options), vec![dir.join("a.txt")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn needle_spanning_a_chunk_boundary_is_found() {
        let (dir, mut options) = base("boundary");
        let mut data = vec![b'x'; 64 * 1024 - 3];
        data.extend_from_slice(b"needle");
        fs::write(dir.join("big.txt"), data).unwrap();
        options.text = "NEEDLE".into();
        assert_eq!(hits(options), vec![dir.join("big.txt")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicates_need_equal_content_not_just_size() {
        let (dir, mut options) = base("dupes");
        fs::write(dir.join("a.txt"), b"same").unwrap();
        fs::write(dir.join("b.txt"), b"same").unwrap();
        fs::write(dir.join("c.txt"), b"diff").unwrap();
        options.duplicates = true;
        let mut found = hits(options);
        found.sort();
        assert_eq!(found, vec![dir.join("a.txt"), dir.join("b.txt")]);
        let _ = fs::remove_dir_all(&dir);
    }
}
