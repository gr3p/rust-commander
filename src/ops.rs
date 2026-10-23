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
    Err(io::Error::new(io::ErrorKind::Unsupported, format!("cannot safely rename to {}: this filesystem does not support no-replace renames", dest.display())))
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
    for path in paths {
        if permanent {
            use std::os::unix::fs::MetadataExt;
            let meta = fs::symlink_metadata(path)?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                let dev = match path.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => fs::metadata(parent)?.dev(),
                    _ => meta.dev(),
                };
                remove_tree(path, dev)?;
            } else {
                fs::remove_file(path)?;
            }
        } else {
            trash(path)?;
        }
    }
    Ok(())
}

// Like `rm -r --one-file-system`: a disk mounted inside the tree is refused, never emptied.
fn remove_tree(path: &Path, dev: u64) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if fs::symlink_metadata(path)?.dev() != dev {
        return Err(io::Error::other(format!("{} is another disk", path.display())));
    }
    for entry in fs::read_dir(path)? {
        let child = entry?.path();
        if fs::symlink_metadata(&child)?.is_dir() {
            remove_tree(&child, dev)?;
        } else {
            fs::remove_file(&child)?;
        }
    }
    fs::remove_dir(path)
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
        use std::os::unix::fs::MetadataExt;
        // Everything is checked before the first overwrite, so a refusal never leaves a half-destroyed tree.
        let dev = meta.dev();
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
                let refuse = if meta.dev() != dev {
                    Some("is another disk")
                } else if !meta.is_dir() && !meta.is_file() {
                    Some("is not a regular file")
                } else if !meta.is_dir() && meta.nlink() > 1 {
                    Some("has other hard links")
                } else if !writable(&child) {
                    Some("is not writable")
                } else {
                    None
                };
                if let Some(why) = refuse {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} {why}", child.display())));
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

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
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
