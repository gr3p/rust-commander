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
    // Search hits can share a name. Two sources on one target would overwrite the first after it moved.
    let mut targets = BTreeSet::new();
    if let Some((_, dest)) = items.iter().find(|(_, dest)| !targets.insert(dest)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("two items would both become {}", dest.display())));
    }
    let total = match count_all(items, cancel, progress) {
        Some(total) => total,
        None => return Ok(Report { cancelled: true, files: 0 }),
    };
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

/// Lets a long loop report at most every 50 ms, so a million small files do not flood the channel.
#[derive(Default)]
pub(crate) struct Ticker(Option<std::time::Instant>);

impl Ticker {
    // The first report waits a tick too, so a small job never says it is counting.
    pub(crate) fn later() -> Self {
        Self(Some(std::time::Instant::now()))
    }

    pub(crate) fn due(&mut self) -> bool {
        if self.0.is_some_and(|last| last.elapsed() < Duration::from_millis(50)) {
            return false;
        }
        self.0 = Some(std::time::Instant::now());
        true
    }
}

fn counting(total: u64, current: &Path) -> Progress {
    Progress { done: 0, total: 0, current: current.to_path_buf(), note: format!("Counting... {total}") }
}

// Same count as `count_entries`, but a huge tree shows that it is counting and can be cancelled. None is cancelled.
fn count_all(items: &[(PathBuf, PathBuf)], cancel: &AtomicBool, progress: &mut dyn FnMut(&Progress)) -> Option<u64> {
    fn walk(path: &Path, total: &mut u64, tick: &mut Ticker, cancel: &AtomicBool, progress: &mut dyn FnMut(&Progress)) -> bool {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        *total += 1;
        if tick.due() {
            progress(&counting(*total, path));
        }
        let is_dir = fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
        if is_dir {
            if let Ok(rd) = fs::read_dir(path) {
                for child in rd.flatten() {
                    if !walk(&child.path(), total, tick, cancel, progress) {
                        return false;
                    }
                }
            }
        }
        true
    }
    let mut total = 0;
    let mut tick = Ticker::later();
    for (src, _) in items {
        if !walk(src, &mut total, &mut tick, cancel, progress) {
            return None;
        }
    }
    Some(total)
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

    let mut target = Target::new(dest)?;
    let existing = &target.existing;
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
            target.existing = None;
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
            match target.commit(src) {
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
        match target.commit(src) {
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
    let copied = copy_file(src, &target, cancel, pause, |n| {
        progress(&Progress {
            done: *done,
            total,
            current: src.to_path_buf(),
            note: String::new(),
        });
        let _ = n;
    })?;
    if kind == Kind::Move {
        require_unchanged(src, &copied)?;
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
    target: &Target,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    mut on_chunk: impl FnMut(u64),
) -> io::Result<fs::Metadata> {
    use std::os::unix::fs::MetadataExt;
    let mut input = File::open(src)?;
    let mut meta = input.metadata()?;
    let (mut output, mut partial) = temporary_file(&target.path, 0o600)?.into_parts();
    let identity = output.metadata()?;
    let result = (|| {
        use std::os::unix::fs::PermissionsExt;
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
                if (written.dev(), written.ino()) != (identity.dev(), identity.ino()) {
                    return Err(replaced(&partial));
                }
                let now = input.metadata()?;
                let same = unchanged(&now, &meta);
                at = if same && written.len() == stop {
                    stop
                } else {
                    output.set_len(0)?;
                    meta = now;
                    0
                };
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            if !unchanged(&input.metadata()?, &meta) {
                return Err(changed(src));
            }
            require_unchanged(src, &meta)?;
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
        let current = fs::symlink_metadata(&partial)?;
        if (current.dev(), current.ino()) != (identity.dev(), identity.ino()) {
            return Err(replaced(&partial));
        }
        target.commit(&partial)?;
        Ok(meta)
    })();
    if fs::symlink_metadata(&partial).is_ok_and(|m| (m.dev(), m.ino()) != (identity.dev(), identity.ino())) {
        partial.disable_cleanup(true);
    }
    result
}

fn replaced(partial: &Path) -> io::Error {
    io::Error::other(format!("{} was replaced by another program", partial.display()))
}

fn changed(path: &Path) -> io::Error {
    io::Error::other(format!("{} changed during the operation; it was not removed or replaced", path.display()))
}

fn unchanged(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    (a.dev(), a.ino(), a.len(), a.mtime(), a.mtime_nsec(), a.ctime(), a.ctime_nsec())
        == (b.dev(), b.ino(), b.len(), b.mtime(), b.mtime_nsec(), b.ctime(), b.ctime_nsec())
}

fn require_unchanged(path: &Path, expected: &fs::Metadata) -> io::Result<()> {
    if unchanged(&fs::symlink_metadata(path)?, expected) {
        Ok(())
    } else {
        Err(changed(path))
    }
}

pub(crate) struct Target {
    path: PathBuf,
    existing: Option<fs::Metadata>,
    _claim: Claim,
}

impl Target {
    pub(crate) fn new(path: &Path) -> io::Result<Self> {
        let claim = claim(path)?;
        let existing = match fs::symlink_metadata(path) {
            Ok(meta) => Some(meta),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        };
        Ok(Self { path: path.to_path_buf(), existing, _claim: claim })
    }

    pub(crate) fn commit(&self, from: &Path) -> io::Result<()> {
        if let Some(old) = &self.existing {
            require_unchanged(&self.path, old)?;
            fs::rename(from, &self.path)
        } else {
            rename_noreplace(from, &self.path)
        }
    }
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
    let key = match (dest.parent().and_then(real_path), dest.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
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

pub(crate) fn temporary_file(dest: &Path, mode: u32) -> io::Result<tempfile::NamedTempFile> {
    use std::os::unix::fs::PermissionsExt;
    let mut suffix = std::ffi::OsString::new();
    if let Some(ext) = dest.extension() {
        suffix.push(".");
        suffix.push(ext);
    }
    tempfile::Builder::new()
        .prefix(".rust-commander-partial-")
        .suffix(&suffix)
        .permissions(fs::Permissions::from_mode(mode))
        .tempfile_in(dest.parent().unwrap_or_else(|| Path::new(".")))
}

/// Writes through a temp file and a rename. Unsafe in-place fallbacks are refused.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let target = Target::new(&path)?;
    let old = &target.existing;
    if old.as_ref().is_some_and(|meta| meta.nlink() > 1 || meta.uid() != unsafe { libc::geteuid() }) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("Cannot safely save {}: it has other hard links or a different owner. Save to a new file instead.", path.display()),
        ));
    }
    let mut temp = temporary_file(&path, old.as_ref().map_or(0o666, |meta| meta.mode() & 0o7777))?;
    temp.write_all(bytes)?;
    if let Some(old) = old {
        temp.as_file().set_permissions(old.permissions())?;
    }
    temp.as_file().sync_all()?;
    target.commit(temp.path())
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
    // NFS and many FUSE mounts (NTFS-3g, sshfs) reject the flag.
    noreplace_fallback(src, dest)
}

/// Places `src` at `dest` without replacing anything there, using only calls that refuse an existing name.
/// A file gets a hard link, then loses its old name. A folder takes the place of a fresh empty folder,
/// and rename refuses that if something was put in it meanwhile.
fn noreplace_fallback(src: &Path, dest: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir(dest)?;
        if let Err(err) = fs::rename(src, dest) {
            let _ = fs::remove_dir(dest);
            return Err(err);
        }
        return Ok(());
    }
    if let Err(err) = fs::hard_link(src, dest) {
        return match err.raw_os_error() {
            Some(libc::EPERM | libc::EOPNOTSUPP | libc::ENOSYS | libc::EMLINK) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("cannot safely rename to {}: this filesystem supports neither no-replace renames nor hard links", dest.display()),
            )),
            _ => Err(err),
        };
    }
    if let Err(err) = fs::remove_file(src) {
        if fs::symlink_metadata(dest).is_ok_and(|now| (now.dev(), now.ino()) == (meta.dev(), meta.ino())) {
            let _ = fs::remove_file(dest);
        }
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    fn copy(src: &Path, dest: &Path, progress: &mut dyn FnMut(&Progress)) -> io::Result<Report> {
        let off = AtomicBool::new(false);
        transfer(&[(src.to_path_buf(), dest.to_path_buf())], Kind::Copy, &off, &off, progress, &mut |_, _| Choice::Overwrite)
    }

    #[test]
    fn copying_a_legacy_partial_name_keeps_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join(".rust-commander-partial-report.txt");
        let dest = dir.path().join("report.txt");
        fs::write(&src, b"original").unwrap();
        assert!(!copy(&src, &dest, &mut |_| {}).unwrap().cancelled);
        assert_eq!(fs::read(&src).unwrap(), b"original");
        assert_eq!(fs::read(&dest).unwrap(), b"original");
        write_atomic(&dest, b"edited").unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_copy_blocks_moves_and_saves_to_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source");
        let other = dir.path().join("other");
        let dest = dir.path().join("target");
        fs::write(&src, b"copy").unwrap();
        fs::write(&other, b"move").unwrap();
        let off = AtomicBool::new(false);
        let mut checked = false;
        copy(&src, &dest, &mut |_| {
            if checked { return; }
            checked = true;
            let moved = transfer(&[(other.clone(), dest.clone())], Kind::Move, &off, &off, &mut |_| {}, &mut |_, _| Choice::Overwrite);
            assert!(moved.unwrap_err().to_string().contains("already being written"));
            assert!(write_atomic(&dest, b"edited").unwrap_err().to_string().contains("already being written"));
        }).unwrap();
        assert!(checked);
        assert_eq!(fs::read(&other).unwrap(), b"move");
        assert_eq!(fs::read(&dest).unwrap(), b"copy");
        write_atomic(&dest, b"after").unwrap();
    }

    #[test]
    fn a_target_created_or_replaced_during_copy_is_not_overwritten() {
        for exists in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("source");
            let dest = dir.path().join("target");
            fs::write(&src, b"copy").unwrap();
            if exists { fs::write(&dest, b"old").unwrap(); }
            let mut changed = false;
            let result = copy(&src, &dest, &mut |_| {
                if changed { return; }
                changed = true;
                let replacement = dir.path().join("replacement");
                fs::write(&replacement, b"new external contents").unwrap();
                fs::rename(replacement, &dest).unwrap();
            });
            assert!(changed && result.is_err());
            assert_eq!(fs::read(&dest).unwrap(), b"new external contents");
            assert_eq!(fs::read(&src).unwrap(), b"copy");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        }
    }

    #[test]
    fn a_changed_source_does_not_replace_the_destination() {
        for replace in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("source");
            let dest = dir.path().join("target");
            fs::write(&src, b"old source").unwrap();
            fs::write(&dest, b"old destination").unwrap();
            let mut changed = false;
            let result = copy(&src, &dest, &mut |_| {
                if changed { return; }
                changed = true;
                if replace {
                    let replacement = dir.path().join("replacement");
                    fs::write(&replacement, b"new saved contents").unwrap();
                    fs::rename(replacement, &src).unwrap();
                } else {
                    fs::write(&src, b"new saved contents").unwrap();
                }
            });
            assert!(changed && result.is_err());
            assert_eq!(fs::read(&src).unwrap(), b"new saved contents");
            assert_eq!(fs::read(&dest).unwrap(), b"old destination");
        }
    }

    #[test]
    fn noreplace_fallback_moves_files_links_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, b"data").unwrap();
        noreplace_fallback(&file, &dir.path().join("moved")).unwrap();
        assert!(!file.exists());
        assert_eq!(fs::read(dir.path().join("moved")).unwrap(), b"data");

        let link = dir.path().join("link");
        std::os::unix::fs::symlink("nowhere", &link).unwrap();
        noreplace_fallback(&link, &dir.path().join("link2")).unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read_link(dir.path().join("link2")).unwrap(), Path::new("nowhere"));

        let folder = dir.path().join("folder");
        fs::create_dir_all(folder.join("sub")).unwrap();
        fs::write(folder.join("sub/inner"), b"inner").unwrap();
        noreplace_fallback(&folder, &dir.path().join("folder2")).unwrap();
        assert!(!folder.exists());
        assert_eq!(fs::read(dir.path().join("folder2/sub/inner")).unwrap(), b"inner");
    }

    #[test]
    fn noreplace_fallback_never_replaces_an_existing_name() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let taken = dir.path().join("taken");
        fs::write(&src, b"new").unwrap();
        fs::write(&taken, b"old").unwrap();
        assert_eq!(noreplace_fallback(&src, &taken).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&src).unwrap(), b"new");
        assert_eq!(fs::read(&taken).unwrap(), b"old");

        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a"), b"a").unwrap();
        let busy = dir.path().join("busy");
        fs::create_dir(&busy).unwrap();
        assert_eq!(noreplace_fallback(&folder, &busy).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(noreplace_fallback(&folder, &taken).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(folder.join("a")).unwrap(), b"a");
        assert!(busy.is_dir());
        assert_eq!(fs::read(&taken).unwrap(), b"old");

        // A folder cannot go inside itself; the empty placeholder is cleaned up.
        let inside = folder.join("inside");
        assert!(noreplace_fallback(&folder, &inside).is_err());
        assert!(!inside.exists());
        assert!(folder.join("a").exists());
    }

    #[test]
    fn cross_device_move_keeps_a_replaced_source() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let out = match tempfile::tempdir_in("/dev/shm") {
            Ok(out) if out.path().metadata().unwrap().dev() != dir.path().metadata().unwrap().dev() => out,
            _ => {
                eprintln!("cross-device test needs a writable /dev/shm on a different filesystem");
                return;
            }
        };
        let src = dir.path().join("source");
        let dest = out.path().join("target");
        fs::write(&src, b"old source").unwrap();
        let off = AtomicBool::new(false);
        let mut changed = false;
        let result = transfer(&[(src.clone(), dest.clone())], Kind::Move, &off, &off, &mut |_| {
            if changed { return; }
            changed = true;
            let replacement = dir.path().join("replacement");
            fs::write(&replacement, b"new saved contents").unwrap();
            fs::rename(replacement, &src).unwrap();
        }, &mut |_, _| Choice::Overwrite);
        assert!(changed && result.is_err());
        assert_eq!(fs::read(&src).unwrap(), b"new saved contents");
        assert!(!dest.exists());
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
    }

    #[test]
    fn atomic_save_refuses_hard_links_without_changing_either_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        let link = dir.path().join("link");
        fs::write(&file, b"original").unwrap();
        fs::hard_link(&file, &link).unwrap();
        let err = write_atomic(&file, b"new contents").unwrap_err();
        assert!(err.to_string().contains("hard links"));
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(fs::read(&link).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn atomic_write_failure_keeps_original() {
        const CHILD: &str = "RC_ATOMIC_FAILURE_TEST_DIR";
        if let Some(dir) = std::env::var_os(CHILD) {
            let path = PathBuf::from(dir).join("original");
            let limit = libc::rlimit { rlim_cur: 4, rlim_max: 4 };
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
            }
            assert!(write_atomic(&path, b"replacement contents").is_err());
            assert_eq!(fs::read(&path).unwrap(), b"original contents");
            assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("original"), b"original contents").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "ops::safety_tests::atomic_write_failure_keeps_original", "--nocapture"])
            .env(CHILD, dir.path())
            .status().unwrap();
        assert!(status.success());
    }

    #[test]
    fn a_replaced_temporary_file_is_not_published_or_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source");
        let dest = dir.path().join("target");
        fs::write(&src, b"source").unwrap();
        let mut replaced = None;
        let result = copy(&src, &dest, &mut |_| {
            if replaced.is_some() { return; }
            let partial = fs::read_dir(dir.path()).unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.file_name().unwrap().to_string_lossy().starts_with(".rust-commander-partial-"))
                .unwrap();
            let external = dir.path().join("external");
            fs::write(&external, b"external contents").unwrap();
            fs::rename(external, &partial).unwrap();
            replaced = Some(partial);
        });
        assert!(result.is_err());
        assert!(!dest.exists());
        assert_eq!(fs::read(replaced.unwrap()).unwrap(), b"external contents");
    }

    #[test]
    fn cancellation_preserves_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source");
        let dest = dir.path().join("target");
        fs::write(&src, b"source").unwrap();
        fs::write(&dest, b"original").unwrap();
        let cancel = AtomicBool::new(false);
        let pause = AtomicBool::new(false);
        let report = transfer(&[(src.clone(), dest.clone())], Kind::Copy, &cancel, &pause,
            &mut |_| cancel.store(true, Ordering::Relaxed), &mut |_, _| Choice::Overwrite).unwrap();
        assert!(report.cancelled);
        assert_eq!(fs::read(&dest).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
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
    if permanent {
        remove_paths(paths, &AtomicBool::new(false), &mut |_| {})?;
        return Ok(());
    }
    for path in paths {
        trash(path)?;
    }
    Ok(())
}

/// Deletes for good, like `rm -r --one-file-system`. Everything is counted first, so progress has a
/// total and a disk mounted inside a folder is refused before anything is gone. `cancel` stops it
/// between two entries; what is already deleted stays deleted.
pub fn remove_paths(paths: &[PathBuf], cancel: &AtomicBool, progress: &mut dyn FnMut(&Progress)) -> io::Result<Report> {
    let paths = outermost(paths);
    let mut sweep = Sweep { cancel, progress, counting: true, total: 0, done: 0, tick: Ticker::default() };
    let result = (|| {
        let mut roots = Vec::with_capacity(paths.len());
        for path in &paths {
            let dev = root_dev(path)?;
            sweep.count(path, dev)?;
            roots.push((path, dev));
        }
        sweep.counting = false;
        if let Some((first, _)) = roots.first() {
            sweep.report(first, true);
        }
        for (path, dev) in roots {
            sweep.remove(path, dev)?;
        }
        Ok(())
    })();
    let files = sweep.done;
    match result {
        Ok(()) => Ok(Report { cancelled: false, files }),
        Err(_) if cancel.load(Ordering::Relaxed) => Ok(Report { cancelled: true, files }),
        Err(err) => Err(err),
    }
}

// Search results can list a folder and something inside it. The folder covers both, once.
// Paths sort by component, so whatever is inside a folder comes right after it.
fn outermost(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut sorted = paths.to_vec();
    sorted.sort();
    sorted.dedup();
    let mut kept: Vec<PathBuf> = Vec::with_capacity(sorted.len());
    for path in sorted {
        if kept.last().is_some_and(|root| path.starts_with(root)) {
            continue;
        }
        kept.push(path);
    }
    kept
}

// A folder must be on its parent's disk, so a mount point is refused rather than emptied.
fn root_dev(path: &Path) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).map_err(|err| at(path, err))?;
    if !meta.is_dir() {
        return Ok(meta.dev());
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Ok(fs::metadata(parent).map_err(|err| at(parent, err))?.dev()),
        _ => Ok(meta.dev()),
    }
}

fn at(path: &Path, err: io::Error) -> io::Error {
    io::Error::new(err.kind(), format!("{}: {err}", path.display()))
}

struct Sweep<'a> {
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(&Progress),
    counting: bool,
    total: u64,
    done: u64,
    tick: Ticker,
}

impl Sweep<'_> {
    fn stopped(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        Ok(())
    }

    // A total of 0 while counting leaves the bar pulsing.
    fn report(&mut self, current: &Path, force: bool) {
        if !self.tick.due() && !force {
            return;
        }
        let progress = if self.counting {
            counting(self.total, current)
        } else {
            Progress { done: self.done, total: self.total.max(self.done), current: current.to_path_buf(), note: String::new() }
        };
        (self.progress)(&progress);
    }

    fn count(&mut self, path: &Path, dev: u64) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        self.stopped()?;
        let meta = fs::symlink_metadata(path).map_err(|err| at(path, err))?;
        self.total += 1;
        self.report(path, false);
        if meta.is_dir() {
            if meta.dev() != dev {
                return Err(io::Error::other(format!("{} is another disk", path.display())));
            }
            for entry in fs::read_dir(path).map_err(|err| at(path, err))? {
                let child = entry.map_err(|err| at(path, err))?.path();
                self.count(&child, dev)?;
            }
        }
        Ok(())
    }

    fn remove(&mut self, path: &Path, dev: u64) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        self.stopped()?;
        let meta = fs::symlink_metadata(path).map_err(|err| at(path, err))?;
        if meta.is_dir() {
            if meta.dev() != dev {
                return Err(io::Error::other(format!("{} is another disk", path.display())));
            }
            for entry in fs::read_dir(path).map_err(|err| at(path, err))? {
                let child = entry.map_err(|err| at(path, err))?.path();
                self.remove(&child, dev)?;
            }
            fs::remove_dir(path).map_err(|err| at(path, err))?;
        } else {
            fs::remove_file(path).map_err(|err| at(path, err))?;
        }
        self.done += 1;
        self.report(path, false);
        Ok(())
    }
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
/// A socket or fifo has no data blocks, so only the name is removed.
pub fn shred(path: &Path, cancel: &AtomicBool) -> io::Result<()> {
    let report = shred_paths(std::slice::from_ref(&path.to_path_buf()), cancel, &mut |_| {})?;
    if report.cancelled {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    Ok(())
}

/// Shreds a whole selection. Every item is checked before the first overwrite, so a refusal anywhere
/// leaves all of it intact. Progress is in bytes overwritten.
pub fn shred_paths(paths: &[PathBuf], cancel: &AtomicBool, progress: &mut dyn FnMut(&Progress)) -> io::Result<Report> {
    let mut plan = ShredPlan::default();
    for path in &outermost(paths) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report { cancelled: true, files: 0 });
        }
        plan.add(path)?;
    }
    let total: u64 = plan.files.iter().map(|(_, len)| len).sum();
    let count = plan.files.len();
    let mut done = 0u64;
    let mut tick = Ticker::default();
    for (index, (file, _)) in plan.files.iter().enumerate() {
        let result = shred_file(file, cancel, &mut |n| {
            done += n;
            if tick.due() {
                let note = format!("{}/{count}  {}", index + 1, file.display());
                progress(&Progress { done, total: total.max(done), current: file.clone(), note });
            }
        });
        match result {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => return Ok(Report { cancelled: true, files: index as u64 }),
            Err(err) => return Err(at(file, err)),
            Ok(()) => {}
        }
    }
    for name in &plan.names {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report { cancelled: true, files: count as u64 });
        }
        fs::remove_file(name).map_err(|err| at(name, err))?;
    }
    plan.dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in &plan.dirs {
        fs::remove_dir(dir).map_err(|err| at(dir, err))?;
    }
    Ok(Report { cancelled: false, files: (count + plan.names.len()) as u64 })
}

#[derive(Default)]
struct ShredPlan {
    files: Vec<(PathBuf, u64)>,
    names: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

impl ShredPlan {
    fn add(&mut self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(path).map_err(|err| at(path, err))?;
        let dev = meta.dev();
        self.check(path, &meta, dev)?;
        if !meta.is_dir() {
            return Ok(());
        }
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).map_err(|err| at(&dir, err))? {
                let child = entry.map_err(|err| at(&dir, err))?.path();
                let meta = fs::symlink_metadata(&child).map_err(|err| at(&child, err))?;
                self.check(&child, &meta, dev)?;
                if meta.is_dir() {
                    stack.push(child);
                }
            }
        }
        Ok(())
    }

    // Refuses what cannot be overwritten safely, then files the entry under what will be done with it.
    fn check(&mut self, path: &Path, meta: &fs::Metadata, dev: u64) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let kind = meta.file_type();
        if kind.is_symlink() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("refusing symlink {}", path.display())));
        }
        let bare = name_only(kind);
        let refuse = if meta.dev() != dev {
            Some("is another disk")
        } else if !meta.is_dir() && !meta.is_file() && !bare {
            Some("is not a regular file")
        } else if !meta.is_dir() && !bare && meta.nlink() > 1 {
            Some("has other hard links")
        } else if !bare && !writable(path) {
            // A folder must be writable too, or its entries could not be removed after they are overwritten.
            Some("is not writable")
        } else {
            None
        };
        if let Some(why) = refuse {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} {why}", path.display())));
        }
        if meta.is_dir() {
            self.dirs.push(path.to_path_buf());
        } else if bare {
            self.names.push(path.to_path_buf());
        } else {
            self.files.push((path.to_path_buf(), meta.len()));
        }
        Ok(())
    }
}

fn name_only(kind: fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    // Socket and fifo: nothing to overwrite.
    kind.is_socket() || kind.is_fifo()
}

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
}

fn shred_file(path: &Path, cancel: &AtomicBool, on_write: &mut dyn FnMut(u64)) -> io::Result<()> {
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
        on_write(n as u64);
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

        fs::create_dir_all(dir.join("mixed")).unwrap();
        fs::write(dir.join("mixed/a.txt"), b"a").unwrap();
        fs::hard_link(dir.join("keep.txt"), dir.join("mixed/b.txt")).unwrap();
        assert!(shred(&dir.join("mixed"), &cancel).is_err());
        assert_eq!(fs::read(dir.join("mixed/a.txt")).unwrap(), b"a");

        std::os::unix::fs::symlink(dir.join("twin.txt"), dir.join("link")).unwrap();
        assert!(shred(&dir.join("link"), &cancel).is_err());
        assert_eq!(fs::read(dir.join("twin.txt")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_paths_counts_then_deletes_with_progress() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("a/b")).unwrap();
        fs::write(tree.join("one"), b"1").unwrap();
        fs::write(tree.join("a/two"), b"2").unwrap();
        fs::write(tree.join("a/b/three"), b"3").unwrap();
        std::os::unix::fs::symlink(dir.path(), tree.join("a/up")).unwrap();
        let single = dir.path().join("single");
        fs::write(&single, b"s").unwrap();
        let mut seen = Vec::new();
        let off = AtomicBool::new(false);
        let report = remove_paths(&[tree.clone(), single.clone()], &off, &mut |p| seen.push((p.done, p.total, p.note.clone()))).unwrap();
        // tree, a, b, one, two, three, up, single. The symlink goes, its target stays.
        assert_eq!(report.files, 8);
        assert!(!report.cancelled);
        assert!(!tree.exists() && !single.exists() && dir.path().exists());
        assert!(seen.first().is_some_and(|(_, total, note)| *total == 0 && note.starts_with("Counting")));
        assert!(seen.iter().any(|(done, total, note)| *done == 0 && *total == 8 && note.is_empty()));
    }

    #[test]
    fn remove_paths_cancel_stops_before_the_next_entry() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("sub/file"), b"x").unwrap();
        let cancel = AtomicBool::new(false);
        let report = remove_paths(std::slice::from_ref(&tree), &cancel, &mut |p| {
            if p.total > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        assert!(report.cancelled);
        assert_eq!(report.files, 0);
        assert!(tree.join("sub/file").exists());
    }

    #[test]
    fn remove_paths_names_the_missing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("not-there");
        let err = remove_paths(std::slice::from_ref(&gone), &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("not-there"));
    }

    #[test]
    fn shred_paths_checks_the_whole_selection_before_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let linked = dir.path().join("linked");
        fs::write(&first, b"keep me").unwrap();
        fs::write(&linked, b"shared").unwrap();
        fs::hard_link(&linked, dir.path().join("other-name")).unwrap();
        let off = AtomicBool::new(false);
        let err = shred_paths(&[first.clone(), linked.clone()], &off, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("has other hard links"));
        assert_eq!(fs::read(&first).unwrap(), b"keep me");

        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("a"), vec![1u8; 300_000]).unwrap();
        let mut seen = Vec::new();
        let report = shred_paths(&[first.clone(), tree.clone()], &off, &mut |p| seen.push((p.done, p.total))).unwrap();
        assert_eq!(report.files, 2);
        assert!(!first.exists() && !tree.exists());
        assert!(seen.first().is_some_and(|(done, total)| *done > 0 && *total == 300_007));
    }

    #[test]
    fn a_folder_and_something_inside_it_are_handled_once() {
        let dir = tempfile::tempdir().unwrap();
        let off = AtomicBool::new(false);
        for shredding in [false, true] {
            let tree = dir.path().join("tree");
            fs::create_dir_all(tree.join("sub")).unwrap();
            fs::write(tree.join("sub/a"), b"a").unwrap();
            let paths = [tree.clone(), tree.join("sub/a"), tree.join("sub"), tree.clone()];
            let report = if shredding {
                shred_paths(&paths, &off, &mut |_| {}).unwrap()
            } else {
                remove_paths(&paths, &off, &mut |_| {}).unwrap()
            };
            assert!(!report.cancelled && !tree.exists());
        }
    }

    #[test]
    fn transfer_cancelled_while_counting_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a"), b"a").unwrap();
        let dest = dir.path().join("dest");
        let on = AtomicBool::new(true);
        let off = AtomicBool::new(false);
        let report = transfer(&[(src.clone(), dest.clone())], Kind::Copy, &on, &off, &mut |_| {}, &mut |_, _| Choice::Overwrite).unwrap();
        assert!(report.cancelled);
        assert_eq!(report.files, 0);
        assert!(!dest.exists());
    }

    #[test]
    fn shred_unlinks_sockets_and_fifos() {
        let dir = std::env::temp_dir().join(format!("rc-shred-sock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tree")).unwrap();
        fs::write(dir.join("tree/a.txt"), b"secret").unwrap();
        let sock = dir.join("tree/iface_socket");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let pipe = dir.join("tree/pipe");
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let cancel = AtomicBool::new(false);
        shred(&dir.join("tree"), &cancel).unwrap();
        assert!(!dir.join("tree").exists());

        let alone = dir.join("alone.sock");
        let _alone = std::os::unix::net::UnixListener::bind(&alone).unwrap();
        shred(&alone, &cancel).unwrap();
        assert!(!alone.exists());
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
    fn two_sources_on_one_target_are_refused() {
        let dir = std::env::temp_dir().join(format!("rc-twin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("a/x.txt"), b"a").unwrap();
        fs::write(dir.join("b/x.txt"), b"b").unwrap();
        let items = [(dir.join("a/x.txt"), dir.join("x.txt")), (dir.join("b/x.txt"), dir.join("x.txt"))];
        let off = AtomicBool::new(false);
        assert!(transfer(&items, Kind::Move, &off, &off, &mut |_| {}, &mut |_, _| Choice::Overwrite).is_err());
        assert!(dir.join("a/x.txt").exists() && dir.join("b/x.txt").exists());
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
