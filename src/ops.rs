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
    // A move within one disk is a single rename, so its tree is never counted.
    let moved: Vec<bool> = items
        .iter()
        .map(|(src, dest)| kind == Kind::Move && !inside(src, dest) && rename_noreplace(src, dest).is_ok())
        .collect();
    let total: u64 = items
        .iter()
        .zip(&moved)
        .map(|((src, _), &moved)| if moved { 1 } else { count_entries(src, cancel) })
        .sum();
    let mut done = 0u64;
    let mut policy = Policy::Ask;
    for ((src, dest), &moved) in items.iter().zip(&moved) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report {
                cancelled: true,
                files: done,
            });
        }
        if moved {
            bump(&mut done, total, dest, progress)?;
            continue;
        }
        if inside(src, dest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "target is inside the source",
            ));
        }
        match one(src, dest, kind, &mut policy, cancel, pause, &mut done, total, progress, conflict) {
            Ok(Stop::Continue | Stop::Again) => {}
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
    // The target name was taken after the check, so the item starts over and asks about it.
    Again,
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
) -> io::Result<Stop> {
    for _ in 0..3 {
        match place(src, dest, kind, policy, cancel, pause, done, total, progress, conflict)? {
            Stop::Again => {}
            stop => return Ok(stop),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", dest.display())))
}

fn place(
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
) -> io::Result<Stop> {
    use std::os::unix::fs::MetadataExt;
    hold(pause, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(Stop::Cancel);
    }
    let meta = fs::symlink_metadata(src)?;
    let is_link = meta.file_type().is_symlink();
    let is_dir = meta.is_dir();

    let existing = fs::symlink_metadata(dest).ok();
    if let Some(old) = &existing {
        if old.dev() == meta.dev() && old.ino() == meta.ino() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} and {} are the same file", src.display(), dest.display()),
            ));
        }
    }
    let merge = is_dir && !is_link && dest.is_dir();
    let mut replace = false;
    if existing.is_some() && !merge {
        if existing.as_ref().is_some_and(|old| old.is_dir()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot replace folder {} with a file", dest.display()),
            ));
        }
        match decide(policy, conflict, src, dest) {
            Choice::Overwrite => replace = true,
            Choice::Cancel => return Ok(Stop::Cancel),
            _ => return bump(done, total, src, progress),
        }
        // A file is replaced by the final rename, so a failed copy keeps the old one.
        if !meta.is_file() {
            fs::remove_file(dest)?;
        }
    }

    if is_link {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let target = fs::read_link(src)?;
        match std::os::unix::fs::symlink(&target, dest) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(Stop::Again),
            other => other?,
        }
        if kind == Kind::Move {
            fs::remove_file(src)?;
        }
        return bump(done, total, src, progress);
    }

    if is_dir {
        if kind == Kind::Move && !dest.exists() {
            match rename_noreplace(src, dest) {
                // Counted only to keep the progress in step with the total.
                Ok(()) => return bump_n(done, total, dest, count_entries(dest, cancel).max(1), progress),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(Stop::Again),
                Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {}
                Err(err) => return Err(err),
            }
        }
        if !merge {
            use std::os::unix::fs::DirBuilderExt;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            // Private until it gets the source mode, so a copy that stops halfway never exposes it.
            match fs::DirBuilder::new().mode(0o700).create(dest) {
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(Stop::Again),
                other => other?,
            }
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
            // Skipped entries keep the folder. Anything else that stops its removal is an error.
            match fs::remove_dir(src) {
                Err(err) if err.raw_os_error() != Some(libc::ENOTEMPTY) => {
                    return Err(io::Error::new(err.kind(), format!("cannot remove {}: {err}", src.display())));
                }
                _ => {}
            }
        }
        return bump(done, total, src, progress);
    }

    if kind == Kind::Move {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let moved = if replace { fs::rename(src, dest) } else { rename_noreplace(src, dest) };
        match moved {
            Ok(()) => return bump(done, total, src, progress),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(Stop::Again),
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {}
            Err(err) => return Err(err),
        }
    }
    // Pipes are made anew. Sockets and devices cannot be copied as data, and opening a pipe would block.
    if !meta.is_file() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        if !meta.file_type().is_fifo() {
            // A socket (gpg-agent, a running server) must not stop a whole tree. It stays at the source.
            crate::log::info(&format!("skip socket or device {}", src.display()));
            return bump(done, total, src, progress);
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let c = std::ffi::CString::new(dest.as_os_str().as_bytes())?;
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::AlreadyExists {
                return Ok(Stop::Again);
            }
            return Err(err);
        }
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(meta.mode() & 0o777));
        if kind == Kind::Move {
            fs::remove_file(src)?;
        }
        return bump(done, total, src, progress);
    }
    // Synced only when something is lost if the copy does not reach the disk: the source or an old target.
    let kept = copy_file(
        src,
        dest,
        replace,
        kind == Kind::Move || replace,
        cancel,
        pause,
        |_| {
            progress(&Progress {
                done: *done,
                total,
                current: src.to_path_buf(),
                note: String::new(),
            })
        },
        &mut || decide(policy, conflict, src, dest),
    )?;
    match kept {
        Some(Choice::Cancel) => return Ok(Stop::Cancel),
        Some(_) => return bump(done, total, src, progress),
        None => {}
    }
    if kind == Kind::Move {
        fs::remove_file(src)?;
    }
    bump(done, total, src, progress)
}

// Overwrite, Skip or Cancel. The "all" answers stick for the rest of the job.
fn decide(policy: &mut Policy, conflict: &mut dyn FnMut(&Path, &Path) -> Choice, src: &Path, dest: &Path) -> Choice {
    let choice = match *policy {
        Policy::Ask => conflict(src, dest),
        Policy::Overwrite => Choice::Overwrite,
        Policy::Skip => Choice::Skip,
    };
    match choice {
        Choice::OverwriteAll => {
            *policy = Policy::Overwrite;
            Choice::Overwrite
        }
        Choice::SkipAll => {
            *policy = Policy::Skip;
            Choice::Skip
        }
        other => other,
    }
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

/// `None` once published. A target that appears meanwhile goes to `exists`, and a Skip or Cancel
/// from it comes back with the copy discarded.
fn copy_file(
    src: &Path,
    dest: &Path,
    replace: bool,
    sync: bool,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    mut on_chunk: impl FnMut(u64),
    exists: &mut dyn FnMut() -> Choice,
) -> io::Result<Option<Choice>> {
    use std::os::unix::fs::MetadataExt;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
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
        // A short copy must never replace a target or let a move delete its source.
        let written = output.metadata()?.len();
        if written < meta.len() {
            return Err(io::Error::other(format!(
                "{} was copied short: {written} of {} bytes",
                src.display(),
                meta.len()
            )));
        }
        // Mode and time follow the file. Setuid and setgid do not, since the copy gets a new owner.
        // Some file systems (vfat) refuse both, which must not fail the copy.
        let _ = output.set_permissions(fs::Permissions::from_mode(meta.permissions().mode() & 0o777));
        if let Ok(time) = meta.modified() {
            let _ = output.set_modified(time);
        }
        if sync {
            output.sync_all()?;
        }
        drop(output);
        let mut replace = replace;
        loop {
            // Another program may have taken the partial name. Only our own data is published.
            if fs::symlink_metadata(&partial)?.ino() != ino {
                return Err(replaced(&partial));
            }
            let published = if replace { fs::rename(&partial, dest) } else { rename_noreplace(&partial, dest) };
            match published {
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => match exists() {
                    Choice::Overwrite => {
                        if !sync {
                            File::open(&partial)?.sync_all()?;
                        }
                        replace = true;
                    }
                    choice => return Ok(Some(choice)),
                },
                other => return other.map(|()| None),
            }
        }
    })();
    if !matches!(result, Ok(None)) && mine.is_some_and(|ino| fs::symlink_metadata(&partial).is_ok_and(|m| m.ino() == ino)) {
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
    let mut first = true;
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
        // Some kernels, FUSE and procfs answer 0 at once for a file that has data.
        if n == 0 && first {
            return copy_read_write(input, output, off_in as u64, cancel, pause, on_chunk);
        }
        if n == 0 {
            return Ok(None);
        }
        first = false;
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
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    const PREFIX: &[u8] = b".rust-commander-partial-";
    let name = dest.file_name().unwrap_or(std::ffi::OsStr::new("file")).as_bytes();
    let mut out = PREFIX.to_vec();
    // NAME_MAX is 255 bytes. A longer name keeps its start plus a hash of the whole, so two stay apart.
    if PREFIX.len() + name.len() > 255 {
        let hash = name.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        out.extend_from_slice(&name[..255 - PREFIX.len() - 17]);
        out.extend_from_slice(format!("~{hash:016x}").as_bytes());
    } else {
        out.extend_from_slice(name);
    }
    dest.with_file_name(std::ffi::OsString::from_vec(out))
}

/// Writes through a temp file and a rename, so a crash or a full disk keeps the old file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let old = fs::metadata(&path).ok();
    // A rename would split a hard link or take the file from its owner.
    if old.as_ref().is_some_and(|meta| meta.nlink() > 1 || meta.uid() != unsafe { libc::geteuid() }) {
        return fs::write(&path, bytes);
    }
    let temp = partial_path(&path);
    let _ = fs::remove_file(&temp);
    // Private until it gets the old mode, which only goes on after the data.
    let opened = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if old.is_some() { 0o600 } else { 0o666 })
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp);
    let mut file = match opened {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied && old.is_some() => return fs::write(&path, bytes),
        Err(err) => return Err(err),
    };
    // The temp gets the editor's group. A file shared through its group keeps it, or is written in place.
    if let Some(old) = &old {
        let gid = file.metadata().map(|meta| meta.gid()).ok();
        if gid != Some(old.gid()) && unsafe { libc::fchown(file.as_raw_fd(), libc::uid_t::MAX, old.gid()) } != 0 {
            drop(file);
            let _ = fs::remove_file(&temp);
            return fs::write(&path, bytes);
        }
    }
    let result = (|| {
        file.write_all(bytes)?;
        // After the data, since a write clears setuid and setgid.
        if let Some(old) = &old {
            file.set_permissions(old.permissions())?;
        }
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

pub fn rename_noreplace(src: &Path, dest: &Path) -> io::Result<()> {
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

// Stops early on cancel. The job sees the flag right after.
fn count_entries(path: &Path, cancel: &AtomicBool) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 1;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return 1;
    }
    let mut n = 1u64;
    if let Ok(rd) = fs::read_dir(path) {
        for child in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            n += count_entries(&child.path(), cancel);
        }
    }
    n
}

pub fn inside(src: &Path, dest: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let src = normalize(src);
    let dest = normalize(dest);
    if dest.starts_with(&src) && dest != src {
        return true;
    }
    let Some(real) = real_path(&dest) else {
        return false;
    };
    // A pane entered through a symlinked folder hides the nesting from the text.
    // Only the parent of src is resolved: a symlink src is copied as a link.
    if let (Some(parent), Some(name)) = (src.parent(), src.file_name()) {
        if let Ok(parent) = fs::canonicalize(parent) {
            let src = parent.join(name);
            if real.starts_with(&src) && real != src {
                return true;
            }
        }
    }
    // A bind mount shows the same folder under a second path, so only the inode tells.
    let Ok(meta) = fs::symlink_metadata(&src) else {
        return false;
    };
    meta.is_dir()
        && real
            .ancestors()
            .skip(1)
            .any(|dir| fs::metadata(dir).is_ok_and(|m| (m.dev(), m.ino()) == (meta.dev(), meta.ino())))
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
            remove_tree(path, &AtomicBool::new(false), &mut |_| {})?;
        } else {
            trash(path)?;
        }
    }
    Ok(())
}

/// Deletes for good, entry by entry, so `cancel` stops it and `each` sees every path removed.
/// A link is removed, never followed, and nothing on another file system is touched.
pub fn remove_tree(path: &Path, cancel: &AtomicBool, each: &mut dyn FnMut(&Path)) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    if !fs::symlink_metadata(path)?.is_dir() {
        fs::remove_file(path)?;
        each(path);
        return Ok(());
    }
    let top = open_dir(libc::AT_FDCWD, &std::ffi::CString::new(path.as_os_str().as_bytes())?)?;
    let dev = dev_of(&top)?;
    clear_dir(top, path, dev, cancel, each)?;
    fs::remove_dir(path)?;
    each(path);
    Ok(())
}

// O_NOFOLLOW: a folder swapped for a link meanwhile fails here instead of being entered.
fn open_dir(dir: std::os::fd::RawFd, name: &std::ffi::CStr) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let fd = unsafe { libc::openat(dir, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

fn dev_of(fd: &std::os::fd::OwnedFd) -> io::Result<libc::dev_t> {
    use std::os::fd::AsRawFd;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st.st_dev)
}

struct Stream(*mut libc::DIR);

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

fn clear_dir(
    dir: std::os::fd::OwnedFd,
    path: &Path,
    dev: libc::dev_t,
    cancel: &AtomicBool,
    each: &mut dyn FnMut(&Path),
) -> io::Result<()> {
    use std::os::fd::{AsRawFd, IntoRawFd};
    use std::os::unix::ffi::OsStrExt;
    let stream = unsafe { libc::fdopendir(dir.as_raw_fd()) };
    if stream.is_null() {
        return Err(io::Error::last_os_error());
    }
    // The stream owns the descriptor from here and closes it.
    let fd = dir.into_raw_fd();
    let stream = Stream(stream);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        unsafe { *libc::__errno_location() = 0 };
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(0) { Ok(()) } else { Err(err) };
        }
        let (name, kind) = unsafe { (std::ffi::CStr::from_ptr((&raw const (*entry).d_name).cast()), (*entry).d_type) };
        if name == c"." || name == c".." {
            continue;
        }
        let child = path.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        remove_at(fd, name, kind == libc::DT_DIR || kind == libc::DT_UNKNOWN, &child, dev, cancel, each)
            .map_err(|err| if err.kind() == io::ErrorKind::Interrupted { err } else { at_path(err, &child) })?;
    }
}

fn remove_at(
    dir: std::os::fd::RawFd,
    name: &std::ffi::CStr,
    maybe_dir: bool,
    path: &Path,
    dev: libc::dev_t,
    cancel: &AtomicBool,
    each: &mut dyn FnMut(&Path),
) -> io::Result<()> {
    if maybe_dir {
        match open_dir(dir, name) {
            Ok(sub) => {
                if dev_of(&sub)? != dev {
                    return Err(io::Error::other(format!("contains a mount point: {}", path.display())));
                }
                clear_dir(sub, path, dev, cancel, each)?;
                if unsafe { libc::unlinkat(dir, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                each(path);
                return Ok(());
            }
            Err(err) if matches!(err.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        }
    }
    if unsafe { libc::unlinkat(dir, name.as_ptr(), 0) } != 0 {
        let err = io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(libc::EISDIR) if !maybe_dir => remove_at(dir, name, true, path, dev, cancel, each),
            Some(libc::ENOENT) => Ok(()),
            _ => Err(err),
        };
    }
    each(path);
    Ok(())
}

fn at_path(err: io::Error, path: &Path) -> io::Error {
    // Errors from deeper down already name their path.
    if err.raw_os_error().is_none() {
        return err;
    }
    io::Error::new(err.kind(), format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rc-ops-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(items: &[(PathBuf, PathBuf)], kind: Kind, progress: &mut dyn FnMut(&Progress), choice: Choice) -> io::Result<Report> {
        transfer(items, kind, &AtomicBool::new(false), &AtomicBool::new(false), progress, &mut |_, _| choice)
    }

    #[test]
    fn long_names_get_a_partial_that_fits() {
        let dir = Path::new("/tmp");
        let a = partial_path(&dir.join("a".repeat(250)));
        let b = partial_path(&dir.join(format!("{}b", "a".repeat(249))));
        assert_eq!(a.file_name().unwrap().len(), 255);
        assert_ne!(a, b);
        assert_eq!(partial_path(&dir.join("x")), dir.join(".rust-commander-partial-x"));
        assert_eq!(partial_path(&dir.join("a".repeat(231))).file_name().unwrap().len(), 255);
    }

    #[test]
    fn target_that_appears_while_copying_is_asked_about() {
        let dir = scratch("appears");
        let (src, dest) = (dir.join("src"), dir.join("dest"));
        fs::write(&src, b"new").unwrap();
        let mut sneak = |_: &Progress| {
            let _ = fs::OpenOptions::new().write(true).create_new(true).open(&dest).and_then(|mut f| f.write_all(b"old"));
        };
        run(&[(src.clone(), dest.clone())], Kind::Copy, &mut sneak, Choice::Skip).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"old");
        assert!(src.exists());
        let partials = fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("partial")).count();
        assert_eq!(partials, 0);
        run(&[(src.clone(), dest.clone())], Kind::Copy, &mut |_| {}, Choice::Overwrite).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pipe_is_made_anew_and_socket_is_skipped() {
        let dir = scratch("special");
        let fifo = dir.join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o640) }, 0);
        fs::set_permissions(&fifo, fs::Permissions::from_mode(0o640)).unwrap();
        let out = dir.join("out");
        run(&[(fifo.clone(), out.join("fifo"))], Kind::Copy, &mut |_| {}, Choice::Overwrite).unwrap();
        let made = fs::symlink_metadata(out.join("fifo")).unwrap();
        assert!(made.file_type().is_fifo());
        assert_eq!(made.mode() & 0o777, 0o640);
        let _socket = std::os::unix::net::UnixListener::bind(dir.join("sock")).unwrap();
        assert!(run(&[(dir.join("sock"), out.join("sock"))], Kind::Copy, &mut |_| {}, Choice::Overwrite).is_ok());
        assert!(fs::symlink_metadata(out.join("sock")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_goes_entry_by_entry_without_following_links() {
        let dir = scratch("tree");
        let keep = dir.join("keep");
        fs::create_dir_all(&keep).unwrap();
        fs::write(keep.join("precious"), b"p").unwrap();
        let tree = dir.join("tree");
        fs::create_dir_all(tree.join("a/b")).unwrap();
        fs::write(tree.join("a/b/f"), b"f").unwrap();
        fs::write(tree.join("g"), b"g").unwrap();
        std::os::unix::fs::symlink(&keep, tree.join("a/link")).unwrap();
        let cancel = AtomicBool::new(true);
        let err = remove_tree(&tree, &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(tree.join("g").exists());
        let mut seen = Vec::new();
        remove_tree(&tree, &AtomicBool::new(false), &mut |path| seen.push(path.to_path_buf())).unwrap();
        assert!(!tree.exists());
        assert_eq!(seen.len(), 6);
        assert_eq!(seen.last(), Some(&tree));
        assert!(keep.join("precious").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_save_keeps_mode() {
        let dir = scratch("save");
        let path = dir.join("run.sh");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o4750)).unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o4750);
        let _ = fs::remove_dir_all(&dir);
    }
}
