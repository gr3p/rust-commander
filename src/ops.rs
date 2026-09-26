use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const CHUNK: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Overwrite,
    Skip,
    Cancel,
    OverwriteAll,
    SkipAll,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub current: PathBuf,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub cancelled: bool,
    pub files: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    Ask,
    Overwrite,
    Skip,
}

pub fn transfer(
    items: &[(PathBuf, PathBuf)],
    kind: Kind,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
    conflict: &mut dyn FnMut(&Path, &Path) -> Choice,
) -> io::Result<Report> {
    let total: u64 = items.iter().map(|(src, _)| count_entries(src)).sum();
    let mut done = 0u64;
    let mut policy = Policy::Ask;
    for (src, dest) in items {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report {
                cancelled: true,
                files: done,
            });
        }
        if inside(src, dest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "target is inside the source",
            ));
        }
        match one(src, dest, kind, &mut policy, cancel, &mut done, total, progress, conflict)? {
            Stop::Cancel => {
                return Ok(Report {
                    cancelled: true,
                    files: done,
                });
            }
            Stop::Continue => {}
        }
    }
    Ok(Report {
        cancelled: false,
        files: done,
    })
}

enum Stop {
    Continue,
    Cancel,
}

fn one(
    src: &Path,
    dest: &Path,
    kind: Kind,
    policy: &mut Policy,
    cancel: &AtomicBool,
    done: &mut u64,
    total: u64,
    progress: &mut dyn FnMut(&Progress),
    conflict: &mut dyn FnMut(&Path, &Path) -> Choice,
) -> io::Result<Stop> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(Stop::Cancel);
    }
    let meta = fs::symlink_metadata(src)?;
    let is_link = meta.file_type().is_symlink();
    let is_dir = meta.is_dir();

    if dest.exists() && !(is_dir && !is_link && dest.is_dir()) {
        let choice = match *policy {
            Policy::Ask => conflict(src, dest),
            Policy::Overwrite => Choice::Overwrite,
            Policy::Skip => Choice::Skip,
        };
        match choice {
            Choice::Cancel => return Ok(Stop::Cancel),
            Choice::Skip => return bump(done, total, src, progress),
            Choice::SkipAll => {
                *policy = Policy::Skip;
                return bump(done, total, src, progress);
            }
            Choice::OverwriteAll => *policy = Policy::Overwrite,
            Choice::Overwrite => {}
        }
        if dest.is_dir() {
            fs::remove_dir_all(dest)?;
        } else {
            fs::remove_file(dest)?;
        }
    }

    if is_link {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let target = fs::read_link(src)?;
        std::os::unix::fs::symlink(&target, dest)?;
        if kind == Kind::Move {
            fs::remove_file(src)?;
        }
        return bump(done, total, src, progress);
    }

    if is_dir {
        if kind == Kind::Move && !dest.exists() {
            let n = count_entries(src);
            match fs::rename(src, dest) {
                Ok(()) => return bump_n(done, total, dest, n.max(1), progress),
                Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {}
                Err(err) => return Err(err),
            }
        }
        fs::create_dir_all(dest)?;
        let mut children: Vec<PathBuf> = fs::read_dir(src)?
            .map(|e| e.map(|e| e.path()))
            .collect::<io::Result<_>>()?;
        children.sort();
        for child in children {
            let name = child.file_name().unwrap_or_default();
            if let Stop::Cancel = one(
                &child,
                &dest.join(name),
                kind,
                policy,
                cancel,
                done,
                total,
                progress,
                conflict,
            )? {
                return Ok(Stop::Cancel);
            }
        }
        if kind == Kind::Move {
            let _ = fs::remove_dir(src);
        }
        return bump(done, total, src, progress);
    }

    if kind == Kind::Move && !dest.exists() {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(src, dest) {
            Ok(()) => return bump(done, total, src, progress),
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {}
            Err(err) => return Err(err),
        }
    }
    copy_file(src, dest, cancel, |n| {
        progress(&Progress {
            done: *done,
            total,
            current: src.to_path_buf(),
            note: String::new(),
        });
        let _ = n;
    })?;
    if kind == Kind::Move {
        fs::remove_file(src)?;
    }
    bump(done, total, src, progress)
}

fn bump(
    done: &mut u64,
    total: u64,
    src: &Path,
    progress: &mut dyn FnMut(&Progress),
) -> io::Result<Stop> {
    bump_n(done, total, src, 1, progress)
}

fn bump_n(
    done: &mut u64,
    total: u64,
    src: &Path,
    n: u64,
    progress: &mut dyn FnMut(&Progress),
) -> io::Result<Stop> {
    *done += n;
    progress(&Progress {
        done: *done,
        note: String::new(),
        total,
        current: src.to_path_buf(),
    });
    Ok(Stop::Continue)
}

fn copy_file(src: &Path, dest: &Path, cancel: &AtomicBool, mut on_chunk: impl FnMut(u64)) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = partial_path(dest);
    let result = (|| {
        let mut input = File::open(src)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&partial)?;
        if try_reflink(&input, &output) {
            on_chunk(input.metadata().map(|m| m.len()).unwrap_or(0));
        } else {
            copy_loop(&mut input, &mut output, cancel, &mut on_chunk)?;
        }
        output.sync_all()?;
        drop(output);
        fs::rename(&partial, dest)?;
        if let Ok(meta) = fs::symlink_metadata(src) {
            let _ = fs::set_permissions(dest, meta.permissions());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn copy_loop(
    input: &mut File,
    output: &mut File,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut off_in: libc::loff_t = 0;
    let mut off_out: libc::loff_t = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = unsafe {
            libc::copy_file_range(
                input.as_raw_fd(),
                &mut off_in,
                output.as_raw_fd(),
                &mut off_out,
                CHUNK,
                0,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            let code = err.raw_os_error();
            if matches!(code, Some(libc::EXDEV | libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP)) {
                return copy_read_write(input, output, off_in as u64, cancel, on_chunk);
            }
            return Err(err);
        }
        if n == 0 {
            return Ok(());
        }
        on_chunk(n as u64);
    }
}

fn copy_read_write(
    input: &mut File,
    output: &mut File,
    offset: u64,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> io::Result<()> {
    use std::io::{Seek, SeekFrom};
    input.seek(SeekFrom::Start(offset))?;
    output.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        output.write_all(&buf[..n])?;
        on_chunk(n as u64);
    }
}

fn try_reflink(src: &File, dst: &File) -> bool {
    use std::os::fd::AsRawFd;
    unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) == 0 }
}

fn partial_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().and_then(|s| s.to_str()).unwrap_or("file");
    dest.with_file_name(format!(".comarchy-partial-{name}"))
}

fn count_entries(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 1;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return 1;
    }
    let mut n = 1u64;
    if let Ok(rd) = fs::read_dir(path) {
        for child in rd.flatten() {
            n += count_entries(&child.path());
        }
    }
    n
}

pub fn inside(src: &Path, dest: &Path) -> bool {
    let src = normalize(src);
    let dest = normalize(dest);
    dest.starts_with(&src) && dest != src
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn make_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let path = parent.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}

pub fn rename_in_place(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    let dest = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(new_name);
    fs::rename(path, &dest)?;
    Ok(dest)
}

pub fn delete_paths(
    paths: &[PathBuf],
    permanent: bool,
    trash: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    for path in paths {
        if permanent {
            let meta = fs::symlink_metadata(path)?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
        } else {
            trash(path)?;
        }
    }
    Ok(())
}
