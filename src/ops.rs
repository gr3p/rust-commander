use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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

/// `pause` stops between chunks with both files closed. Clearing it continues at the same byte.
pub fn transfer(
    items: &[(PathBuf, PathBuf)],
    kind: Kind,
    cancel: &AtomicBool,
    pause: &AtomicBool,
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
        match one(src, dest, kind, &mut policy, cancel, pause, &mut done, total, progress, conflict, true) {
            Ok(Stop::Continue) => {}
            Ok(Stop::Cancel) => {
                return Ok(Report {
                    cancelled: true,
                    files: done,
                });
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted && cancel.load(Ordering::Relaxed) => {
                return Ok(Report {
                    cancelled: true,
                    files: done,
                });
            }
            Err(err) => return Err(err),
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
    pause: &AtomicBool,
    done: &mut u64,
    total: u64,
    progress: &mut dyn FnMut(&Progress),
    conflict: &mut dyn FnMut(&Path, &Path) -> Choice,
    make_parent: bool,
) -> io::Result<Stop> {
    hold(pause, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(Stop::Cancel);
    }
    let meta = fs::symlink_metadata(src)?;
    let is_link = meta.file_type().is_symlink();
    let is_dir = meta.is_dir();

    let existing = fs::symlink_metadata(dest).ok();
    if let Some(old) = &existing {
        use std::os::unix::fs::MetadataExt;
        if old.dev() == meta.dev() && old.ino() == meta.ino() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} and {} are the same file", src.display(), dest.display()),
            ));
        }
    }
    let merge = is_dir && !is_link && dest.is_dir();
    if existing.is_some() && !merge {
        if existing.as_ref().is_some_and(|old| old.is_dir()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot replace folder {} with a file", dest.display()),
            ));
        }
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
        // A file is replaced by the final rename, so a failed copy keeps the old one.
        if is_link || is_dir {
            fs::remove_file(dest)?;
        }
    }

    if is_link {
        ready_parent(dest, make_parent)?;
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
            ready_parent(dest, make_parent)?;
            match fs::rename(src, dest) {
                Ok(()) => return bump_n(done, total, dest, n.max(1), progress),
                Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {}
                Err(err) => return Err(err),
            }
        }
        if !merge {
            use std::os::unix::fs::DirBuilderExt;
            ready_parent(dest, make_parent)?;
            // Private until it gets the source mode, so a copy that stops halfway never exposes it.
            fs::DirBuilder::new().mode(0o700).create(dest)?;
        }
        let copied = (|| -> io::Result<Stop> {
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
                    pause,
                    done,
                    total,
                    progress,
                    conflict,
                    false,
                )? {
                    return Ok(Stop::Cancel);
                }
            }
            Ok(Stop::Continue)
        })();
        if !merge {
            // Set last, so a read-only folder can still take its children and they do not bump its time.
            let _ = fs::set_permissions(dest, meta.permissions());
            if let (Ok(Stop::Continue), Ok(time)) = (&copied, meta.modified()) {
                let _ = File::open(dest).and_then(|dir| dir.set_modified(time));
            }
        }
        if !matches!(copied?, Stop::Continue) {
            return Ok(Stop::Cancel);
        }
        if kind == Kind::Move {
            let _ = fs::remove_dir(src);
        }
        return bump(done, total, src, progress);
    }

    if kind == Kind::Move {
        ready_parent(dest, make_parent)?;
        match fs::rename(src, dest) {
            Ok(()) => return bump(done, total, src, progress),
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {}
            Err(err) => return Err(err),
        }
    }
    // Pipes, sockets and devices cannot be copied as data. Opening a pipe would block.
    if !meta.is_file() {
        return bump(done, total, src, progress);
    }
    ready_parent(dest, make_parent)?;
    copy_file(src, dest, cancel, pause, |n| {
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

// A nested file must not create its parent. That brings back a folder deleted while the copy runs.
fn ready_parent(dest: &Path, make: bool) -> io::Result<()> {
    let Some(parent) = dest.parent().filter(|parent| !parent.as_os_str().is_empty()) else {
        return Ok(());
    };
    if make {
        return fs::create_dir_all(parent);
    }
    if fs::symlink_metadata(parent).is_ok() {
        return Ok(());
    }
    Err(io::Error::new(io::ErrorKind::NotFound, format!("{} disappeared", parent.display())))
}

fn copy_file(
    src: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    mut on_chunk: impl FnMut(u64),
) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let _claim = claim(dest)?;
    let partial = partial_path(dest);
    let mut mine = None;
    let result = (|| {
        use std::os::unix::fs::PermissionsExt;
        let mut input = File::open(src)?;
        let mut meta = input.metadata()?;
        // Private until done. A reader that opens it early must not keep a handle to the data.
        let _ = fs::remove_file(&partial);
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&partial)?;
        let ino = output.metadata()?.ino();
        mine = Some(ino);
        if try_reflink(&input, &output) {
            on_chunk(meta.len());
        } else {
            let mut at = 0;
            while let Some(stop) = copy_loop(&mut input, &mut output, at, cancel, pause, &mut on_chunk)? {
                // Paused: both files are closed so the disk can be unmounted. On resume the partial must
                // still be our inode, and a source that changed meanwhile is copied again from the start.
                drop((input, output));
                hold(pause, cancel)?;
                input = File::open(src)?;
                output = fs::OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW).open(&partial)?;
                let written = output.metadata()?;
                if written.ino() != ino {
                    return Err(replaced(&partial));
                }
                let now = input.metadata()?;
                let same = (now.len(), now.mtime(), now.mtime_nsec()) == (meta.len(), meta.mtime(), meta.mtime_nsec());
                at = if same && written.len() == stop {
                    stop
                } else {
                    output.set_len(0)?;
                    meta = now;
                    0
                };
            }
        }
        // Mode and time follow the file. Setuid and setgid do not, since the copy gets a new owner.
        // Some file systems (vfat) refuse both, which must not fail the copy.
        let _ = output.set_permissions(fs::Permissions::from_mode(meta.permissions().mode() & 0o777));
        if let Ok(time) = meta.modified() {
            let _ = output.set_modified(time);
        }
        output.sync_all()?;
        drop(output);
        // Another program may have taken the partial name. Only our own data is published.
        if fs::symlink_metadata(&partial)?.ino() != ino {
            return Err(replaced(&partial));
        }
        fs::rename(&partial, dest)?;
        Ok(())
    })();
    if result.is_err() && mine.is_some_and(|ino| fs::symlink_metadata(&partial).is_ok_and(|m| m.ino() == ino)) {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn replaced(partial: &Path) -> io::Error {
    io::Error::other(format!("{} was replaced by another program", partial.display()))
}

static WRITING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

struct Claim(PathBuf);

impl Drop for Claim {
    fn drop(&mut self) {
        WRITING.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

// The queue runs beside a foreground copy. Two writers on one name would publish each other's half file.
fn claim(dest: &Path) -> io::Result<Claim> {
    let key = match (dest.parent().map(fs::canonicalize), dest.file_name()) {
        (Some(Ok(dir)), Some(name)) => dir.join(name),
        _ => dest.to_path_buf(),
    };
    if !WRITING.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone()) {
        return Err(io::Error::other(format!("{} is already being written", dest.display())));
    }
    Ok(Claim(key))
}

fn hold(pause: &AtomicBool, cancel: &AtomicBool) -> io::Result<()> {
    while pause.load(Ordering::Relaxed) {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

// Some(offset) means paused there. Both offsets move together, so one number resumes both files.
fn copy_loop(
    input: &mut File,
    output: &mut File,
    start: u64,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> io::Result<Option<u64>> {
    use std::os::fd::AsRawFd;
    let mut off_in = start as libc::loff_t;
    let mut off_out = off_in;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if pause.load(Ordering::Relaxed) {
            return Ok(Some(off_in as u64));
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
                return copy_read_write(input, output, off_in as u64, cancel, pause, on_chunk);
            }
            return Err(err);
        }
        if n == 0 {
            return Ok(None);
        }
        on_chunk(n as u64);
    }
}

fn copy_read_write(
    input: &mut File,
    output: &mut File,
    mut at: u64,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> io::Result<Option<u64>> {
    use std::io::{Seek, SeekFrom};
    input.seek(SeekFrom::Start(at))?;
    output.seek(SeekFrom::Start(at))?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if pause.load(Ordering::Relaxed) {
            return Ok(Some(at));
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        output.write_all(&buf[..n])?;
        at += n as u64;
        on_chunk(n as u64);
    }
}

fn try_reflink(src: &File, dst: &File) -> bool {
    use std::os::fd::AsRawFd;
    unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) == 0 }
}

pub(crate) fn partial_path(dest: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".rust-commander-partial-");
    name.push(dest.file_name().unwrap_or(std::ffi::OsStr::new("file")));
    dest.with_file_name(name)
}

/// Writes through a temp file and a rename, so a crash or a full disk keeps the old file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let old = fs::metadata(&path).ok();
    // A rename would split a hard link or take the file from its owner.
    if old.as_ref().is_some_and(|meta| meta.nlink() > 1 || meta.uid() != unsafe { libc::geteuid() }) {
        return fs::write(&path, bytes);
    }
    let temp = partial_path(&path);
    let _ = fs::remove_file(&temp);
    let opened = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o666)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp);
    let mut file = match opened {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied && old.is_some() => return fs::write(&path, bytes),
        Err(err) => return Err(err),
    };
    let result = (|| {
        if let Some(old) = &old {
            file.set_permissions(old.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn check_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad file name"));
    }
    Ok(())
}

fn rename_noreplace(src: &Path, dest: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_src = std::ffi::CString::new(src.as_os_str().as_bytes())?;
    let c_dest = std::ffi::CString::new(dest.as_os_str().as_bytes())?;
    let rc = unsafe { libc::renameat2(libc::AT_FDCWD, c_src.as_ptr(), libc::AT_FDCWD, c_dest.as_ptr(), libc::RENAME_NOREPLACE) };
    if rc == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if !matches!(err.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
        return Err(err);
    }
    // Some file systems have no RENAME_NOREPLACE.
    if fs::symlink_metadata(dest).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", dest.display())));
    }
    fs::rename(src, dest)
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
    if dest.starts_with(&src) && dest != src {
        return true;
    }
    // A pane entered through a symlinked folder hides the nesting from the text.
    // Only the parent of src is resolved: a symlink src is copied as a link.
    let (Some(parent), Some(name)) = (src.parent(), src.file_name()) else {
        return false;
    };
    match (fs::canonicalize(parent), real_path(&dest)) {
        (Ok(parent), Some(dest)) => {
            let src = parent.join(name);
            dest.starts_with(&src) && dest != src
        }
        _ => false,
    }
}

// The real path of the deepest part that exists, with the rest appended as written.
fn real_path(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(real) = fs::canonicalize(cur) {
            return Some(rest.into_iter().rev().fold(real, |acc: PathBuf, name| acc.join(name)));
        }
        rest.push(cur.file_name()?);
        cur = cur.parent()?;
    }
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

pub fn make_file(parent: &Path, name: &str) -> io::Result<PathBuf> {
    check_name(name)?;
    let path = parent.join(name);
    fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    Ok(path)
}

pub fn rename_in_place(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    let dest = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(new_name);
    check_name(new_name)?;
    if dest == path {
        return Ok(dest);
    }
    if let Ok(old) = fs::symlink_metadata(&dest) {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(path)?;
        if old.dev() != meta.dev() || old.ino() != meta.ino() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{new_name} already exists")));
        }
        // Same file, only the case differs on a case-insensitive disk.
        fs::rename(path, &dest)?;
        return Ok(dest);
    }
    rename_noreplace(path, &dest)?;
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

/// The trash is the home disk. A file anywhere else is deleted where it stands.
pub fn on_home_disk(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|dir| dir.exists()) else {
        return false;
    };
    let Ok(home_meta) = fs::metadata(home) else { return false };
    let probe = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => path,
        _ => path.parent().unwrap_or(path),
    };
    fs::metadata(probe).is_ok_and(|meta| meta.dev() == home_meta.dev())
}

/// One random pass, then the file is removed. A symlink is refused so the target stays.
pub fn shred(path: &Path, cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("refusing symlink {}", path.display())));
    }
    if meta.is_dir() {
        let mut files = Vec::new();
        let mut dirs = vec![path.to_path_buf()];
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let child = entry.path();
                let meta = fs::symlink_metadata(&child)?;
                if meta.file_type().is_symlink() {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("refusing symlink {}", child.display())));
                }
                if meta.is_dir() {
                    stack.push(child.clone());
                    dirs.push(child);
                } else {
                    files.push(child);
                }
            }
        }
        for file in &files {
            shred_file(file, cancel)?;
        }
        dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in dirs {
            fs::remove_dir(&dir)?;
        }
        return Ok(());
    }
    shred_file(path, cancel)
}

fn shred_file(path: &Path, cancel: &AtomicBool) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not a regular file {}", path.display())));
    }
    // Overwriting a hard-linked file would destroy the other names too.
    if meta.nlink() > 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} has other hard links", path.display())));
    }
    let len = meta.len();
    let mut random = File::open("/dev/urandom")?;
    let mut buf = vec![0u8; CHUNK];
    let mut left = len;
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = buf.len().min(left as usize);
        random.read_exact(&mut buf[..n])?;
        file.write_all(&buf[..n])?;
        left -= n as u64;
    }
    file.sync_all()?;
    drop(file);
    fs::remove_file(path)
}

#[cfg(test)]
mod shred_tests {
    use super::*;

    #[test]
    fn shred_removes_files_but_refuses_hard_links_and_symlinks() {
        let dir = std::env::temp_dir().join(format!("rc-shred-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tree/sub")).unwrap();
        fs::write(dir.join("tree/sub/a.txt"), b"secret").unwrap();
        let cancel = AtomicBool::new(false);
        shred(&dir.join("tree"), &cancel).unwrap();
        assert!(!dir.join("tree").exists());

        fs::write(dir.join("keep.txt"), b"keep").unwrap();
        fs::hard_link(dir.join("keep.txt"), dir.join("twin.txt")).unwrap();
        assert!(shred(&dir.join("keep.txt"), &cancel).is_err());
        assert_eq!(fs::read(dir.join("twin.txt")).unwrap(), b"keep");

        std::os::unix::fs::symlink(dir.join("twin.txt"), dir.join("link")).unwrap();
        assert!(shred(&dir.join("link"), &cancel).is_err());
        assert_eq!(fs::read(dir.join("twin.txt")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_moves_into_a_target_that_does_not_exist_yet() {
        let dir = std::env::temp_dir().join(format!("rc-move-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/sub")).unwrap();
        fs::write(dir.join("src/sub/a.txt"), b"a").unwrap();
        let dest = dir.join("new/deeper/src");
        let off = AtomicBool::new(false);
        let report = transfer(&[(dir.join("src"), dest.clone())], Kind::Move, &off, &off, &mut |_| {}, &mut |_, _| Choice::Cancel).unwrap();
        assert!(!report.cancelled);
        assert_eq!(fs::read(dest.join("sub/a.txt")).unwrap(), b"a");
        assert!(!dir.join("src").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_folder_is_not_created_again() {
        let dir = std::env::temp_dir().join(format!("rc-gone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.txt"), b"a").unwrap();
        let off = AtomicBool::new(false);
        let mut done = 0u64;
        let mut policy = Policy::Overwrite;
        let err = one(
            &dir.join("src/a.txt"),
            &dir.join("gone/a.txt"),
            Kind::Copy,
            &mut policy,
            &off,
            &off,
            &mut done,
            1,
            &mut |_| {},
            &mut |_, _| Choice::Overwrite,
            false,
        );
        assert!(err.is_err());
        assert!(!dir.join("gone").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn home_disk_is_the_trash_disk() {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        assert!(on_home_disk(&home));
        let foreign = PathBuf::from("/proc/self");
        if fs::metadata(&foreign).ok().map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.dev() != fs::metadata(&home).unwrap().dev()
        }) == Some(true) {
            assert!(!on_home_disk(&foreign));
        }
    }
}
