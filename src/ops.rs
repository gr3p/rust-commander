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
        match one(src, dest, kind, &mut policy, cancel, &mut done, total, progress, conflict) {
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
        if !merge {
            // Set last, so a read-only folder can still take its children and they do not bump its time.
            let _ = fs::set_permissions(dest, meta.permissions());
            if let Ok(time) = meta.modified() {
                let _ = File::open(dest).and_then(|dir| dir.set_modified(time));
            }
        }
        if kind == Kind::Move {
            let _ = fs::remove_dir(src);
        }
        return bump(done, total, src, progress);
    }

    if kind == Kind::Move {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
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
        use std::os::unix::fs::PermissionsExt;
        let mut input = File::open(src)?;
        let meta = input.metadata()?;
        // Private until done. A reader that opens it early must not keep a handle to the data.
        let _ = fs::remove_file(&partial);
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&partial)?;
        if try_reflink(&input, &output) {
            on_chunk(meta.len());
        } else {
            copy_loop(&mut input, &mut output, cancel, &mut on_chunk)?;
        }
        // Mode and time follow the file. Setuid and setgid do not, since the copy gets a new owner.
        // Some file systems (vfat) refuse both, which must not fail the copy.
        let _ = output.set_permissions(fs::Permissions::from_mode(meta.permissions().mode() & 0o777));
        if let Ok(time) = meta.modified() {
            let _ = output.set_modified(time);
        }
        output.sync_all()?;
        drop(output);
        fs::rename(&partial, dest)?;
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
