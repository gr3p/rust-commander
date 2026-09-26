use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::raw::{c_int, c_uchar};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ops::{Choice, Progress, Report};

pub const MAX_BYTES: u64 = 8 << 30;

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub size: u64,
    pub dir: bool,
}

#[derive(Clone, Debug)]
pub struct Listed {
    pub members: Vec<Member>,
    pub needs_password: bool,
}

#[derive(Debug)]
pub enum Fail {
    NeedPassword,
    BadPassword,
    Cancelled,
    Message(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::NeedPassword => write!(f, "Password required"),
            Fail::BadPassword => write!(f, "Wrong password"),
            Fail::Cancelled => write!(f, "Cancelled"),
            Fail::Message(text) => write!(f, "{text}"),
        }
    }
}

impl From<io::Error> for Fail {
    fn from(err: io::Error) -> Self {
        Fail::Message(err.to_string())
    }
}

impl From<zip::result::ZipError> for Fail {
    fn from(err: zip::result::ZipError) -> Self {
        match err {
            zip::result::ZipError::InvalidPassword => Fail::BadPassword,
            zip::result::ZipError::UnsupportedArchive(zip::result::ZipError::PASSWORD_REQUIRED) => Fail::NeedPassword,
            zip::result::ZipError::Io(err) => Fail::Message(err.to_string()),
            other => Fail::Message(other.to_string()),
        }
    }
}

impl From<unrar_ng::error::UnrarError> for Fail {
    fn from(err: unrar_ng::error::UnrarError) -> Self {
        match err.code {
            unrar_ng::error::Code::MissingPassword => Fail::NeedPassword,
            unrar_ng::error::Code::BadPassword => Fail::BadPassword,
            _ => Fail::Message(err.to_string()),
        }
    }
}

pub fn is_rar_root(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(stem) = lower.strip_suffix(".rar") else { return false };
    // part02.rar and later are volumes. part01.rar and name.rar are the file you unpack.
    let Some((_, number)) = stem.rsplit_once(".part") else { return true };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    number.trim_start_matches('0') == "1"
}

pub fn unpack_mark(name: &str) -> &'static str {
    if is_rar_root(name) {
        "R"
    } else if name.len() >= 4 && name.as_bytes()[name.len() - 4..].eq_ignore_ascii_case(b".zip") {
        "Z"
    } else {
        ""
    }
}

pub fn is_archive(path: &Path) -> bool {
    match extension(path) {
        Some("zip" | "rar") => true,
        _ => false,
    }
}

pub fn list(path: &Path, password: Option<&str>) -> Result<Listed, Fail> {
    match extension(path) {
        Some("zip") => list_zip(path, password),
        Some("rar") => list_rar(path, password),
        _ => Err(Fail::Message("Not an archive".into())),
    }
}

pub fn extract(
    path: &Path,
    dest: &Path,
    names: Option<&HashSet<String>>,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
    conflict: &mut dyn FnMut(&Path, &Path) -> Choice,
) -> Result<Report, Fail> {
    match extension(path) {
        Some("zip") => extract_zip(path, dest, names, password, cancel, progress, conflict),
        Some("rar") => extract_rar(path, dest, names, password, cancel, progress, conflict),
        _ => Err(Fail::Message("Not an archive".into())),
    }
}

fn extension(path: &Path) -> Option<&str> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("zip") {
        Some("zip")
    } else if ext.eq_ignore_ascii_case("rar") {
        Some("rar")
    } else {
        None
    }
}

fn list_zip(path: &Path, password: Option<&str>) -> Result<Listed, Fail> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    let mut listed = Listed { members: Vec::new(), needs_password: false };
    for index in 0..archive.len() {
        let file = match zip_at(&mut archive, index, password) {
            Ok(file) => file,
            Err(Fail::NeedPassword) => {
                listed.needs_password = true;
                continue;
            }
            Err(err) => return Err(err),
        };
        if file.encrypted() && password.is_none() {
            listed.needs_password = true;
        }
        if file.is_symlink() {
            continue;
        }
        let Some(name) = zip_name(&file) else { continue };
        listed.members.push(Member { name, size: file.size(), dir: file.is_dir() });
    }
    Ok(listed)
}

fn extract_zip(
    path: &Path,
    dest: &Path,
    names: Option<&HashSet<String>>,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
    conflict: &mut dyn FnMut(&Path, &Path) -> Choice,
) -> Result<Report, Fail> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    let mut plan = Vec::new();
    for index in 0..archive.len() {
        let file = zip_at(&mut archive, index, password)?;
        if file.encrypted() && password.is_none() {
            return Err(Fail::NeedPassword);
        }
        if file.is_symlink() {
            continue;
        }
        let Some(name) = zip_name(&file) else { continue };
        if names.is_some_and(|set| !set.contains(&name)) {
            continue;
        }
        plan.push((index, name, file.is_dir(), file.size()));
    }
    let total = plan.iter().filter(|item| !item.2).map(|item| item.3).sum();
    let file_total = plan.iter().filter(|item| !item.2).count() as u64;
    let mut file_index = 0u64;
    let mut written = 0u64;
    let mut files = 0u64;
    let mut policy = Policy::Ask;
    for (index, name, dir, size) in plan {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report { cancelled: true, files });
        }
        let rel = PathBuf::from(&name);
        let target = located(dest, &rel)?;
        if dir {
            ensure_dir(&target)?;
            continue;
        }
        if size > MAX_BYTES || written.saturating_add(size) > MAX_BYTES {
            return Err(Fail::Message("archive expands past 8 GiB".into()));
        }
        match keep(&target, &rel, &mut policy, conflict)? {
            Keep::Skip => continue,
            Keep::Cancel => return Ok(Report { cancelled: true, files }),
            Keep::Write => {}
        }
        let mut file = zip_at(&mut archive, index, password)?;
        file_index += 1;
        let note = unpack_note(path, file_index, file_total, &name);
        let mut step = |update: &Progress| {
            let mut update = update.clone();
            update.note = note.clone();
            progress(&update);
        };
        match write_reader(&mut file, &target, size, &mut written, total, &rel, cancel, &mut step) {
            Ok(()) => files += 1,
            Err(Fail::Cancelled) => {
                let _ = fs::remove_file(&target);
                return Ok(Report { cancelled: true, files });
            }
            Err(err) => {
                let _ = fs::remove_file(&target);
                return Err(err);
            }
        }
    }
    Ok(Report { cancelled: false, files })
}

fn zip_at<'a, R: Read + io::Seek>(
    archive: &'a mut zip::ZipArchive<R>,
    index: usize,
    password: Option<&str>,
) -> Result<zip::read::ZipFile<'a, R>, Fail> {
    match password {
        Some(password) => Ok(archive.by_index_decrypt(index, password.as_bytes())?),
        None => Ok(archive.by_index(index)?),
    }
}

fn zip_name(file: &zip::read::ZipFile<'_, impl Read>) -> Option<String> {
    let path = file.enclosed_name()?;
    slash(path.to_str()?)
}

fn list_rar(path: &Path, password: Option<&str>) -> Result<Listed, Fail> {
    let owned = rar_path(path);
    let opened = open_rar(&owned, password).open_for_listing()?;
    let mut listed = Listed { members: Vec::new(), needs_password: false };
    for item in opened {
        let header = item?;
        if header.is_encrypted() {
            listed.needs_password = true;
        }
        if header.is_directory() {
            push_rar(&mut listed, &header, true);
            continue;
        }
        if rar_symlink(header.file_attr) {
            continue;
        }
        push_rar(&mut listed, &header, false);
    }
    Ok(listed)
}

fn push_rar(listed: &mut Listed, header: &unrar_ng::FileHeader, dir: bool) {
    let Some(name) = header.filename.to_str() else { return };
    // RAR from Windows stores backslash separators.
    let Some(name) = slash(&name.replace('\\', "/")) else { return };
    listed.members.push(Member { name, size: header.unpacked_size, dir });
}

fn unpack_note(archive: &Path, index: u64, total_files: u64, name: &str) -> String {
    let label = archive.file_name().and_then(|name| name.to_str()).unwrap_or("archive");
    let left = total_files.saturating_sub(index).saturating_add(1);
    format!("{label}   {index}/{total_files}   {left} left   {name}")
}

struct RarHook {
    cancel: *const AtomicBool,
    total: u64,
    base: u64,
    seen: u64,
    sent: u64,
    note: String,
    progress: [usize; 2],
}

thread_local! {
    static RAR_HOOK: RefCell<Option<RarHook>> = const { RefCell::new(None) };
}

struct ClearRarHook;

impl Drop for ClearRarHook {
    fn drop(&mut self) {
        RAR_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

fn pack_cb(progress: &mut dyn FnMut(&Progress)) -> [usize; 2] {
    let raw = progress as *mut dyn FnMut(&Progress);
    unsafe { std::mem::transmute(raw) }
}

fn call_cb(parts: [usize; 2], update: &Progress) {
    let progress: *mut dyn FnMut(&Progress) = unsafe { std::mem::transmute(parts) };
    unsafe { (*progress)(update) };
}

fn hook_install(cancel: &AtomicBool, total: u64, progress: &mut dyn FnMut(&Progress)) {
    let packed = pack_cb(progress);
    RAR_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(RarHook {
            cancel,
            total,
            base: 0,
            seen: 0,
            sent: 0,
            note: String::new(),
            progress: packed,
        });
    });
}

fn hook_file(base: u64, note: String) {
    RAR_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(hook) = slot.as_mut() else { return };
        hook.base = base;
        hook.seen = 0;
        hook.sent = 0;
        hook.note = note;
    });
}

extern "C" fn on_rar_data(_addr: *mut c_uchar, count: c_int) -> c_int {
    if count <= 0 {
        return 1;
    }
    let report = RAR_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(hook) = slot.as_mut() else { return (1, None) };
        if unsafe { (*hook.cancel).load(Ordering::Relaxed) } {
            return (0, None);
        }
        hook.seen += count as u64;
        if hook.sent != 0 && hook.seen - hook.sent < 256 * 1024 {
            return (1, None);
        }
        hook.sent = hook.seen;
        (
            1,
            Some((hook.base + hook.seen, hook.total, hook.note.clone(), hook.progress)),
        )
    });
    let (code, event) = report;
    if let Some((done, total, note, progress)) = event {
        call_cb(progress, &Progress { done, total, current: PathBuf::new(), note });
        let stop = RAR_HOOK.with(|slot| {
            slot.borrow().as_ref().is_some_and(|hook| unsafe { (*hook.cancel).load(Ordering::Relaxed) })
        });
        return if stop { 0 } else { code };
    }
    code
}

fn emit_progress(done: u64, total: u64, current: PathBuf, note: String) {
    let progress = RAR_HOOK.with(|slot| slot.borrow().as_ref().map(|hook| hook.progress));
    if let Some(progress) = progress {
        call_cb(progress, &Progress { done, total, current, note });
    }
}

fn arm_rar(archive: &unrar_ng::OpenArchive<unrar_ng::Process, unrar_ng::CursorBeforeHeader>) {
    // OpenArchive keeps the unrar handle in its first field. RARSetCallback does not clear this.
    let handle = unsafe { *(archive as *const _ as *const *const unrar_ng_sys::Handle) };
    unsafe { unrar_ng_sys::RARSetProcessDataProc(handle, Some(on_rar_data)) };
}

fn extract_rar(
    path: &Path,
    dest: &Path,
    names: Option<&HashSet<String>>,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
    conflict: &mut dyn FnMut(&Path, &Path) -> Choice,
) -> Result<Report, Fail> {
    let owned = rar_path(path);
    let listed = list_rar(&owned, password)?;
    if listed.needs_password && password.is_none() {
        return Err(Fail::NeedPassword);
    }
    let total = listed.members.iter().filter(|m| !m.dir).map(|m| m.size).sum();
    let file_total = listed
        .members
        .iter()
        .filter(|member| !member.dir && names.is_none_or(|set| set.contains(&member.name)))
        .count() as u64;
    let mut file_index = 0u64;
    let mut opened = open_rar(&owned, password).open_for_processing()?;
    hook_install(cancel, total, progress);
    let _clear = ClearRarHook;
    arm_rar(&opened);
    let mut written = 0u64;
    let mut files = 0u64;
    let mut policy = Policy::Ask;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report { cancelled: true, files });
        }
        let Some(file) = opened.read_header()? else { break };
        let header = file.entry();
        let dir = header.is_directory();
        let size = header.unpacked_size;
        let symlink = rar_symlink(header.file_attr);
        let raw = header.filename.to_str().map(|name| name.replace('\\', "/"));
        let name = raw.as_deref().and_then(slash);
        let skip = name.is_none()
            || symlink
            || name.as_ref().is_some_and(|name| names.is_some_and(|set| !set.contains(name)));
        if skip {
            opened = file.skip()?;
            continue;
        }
        let name = name.unwrap();
        let rel = PathBuf::from(&name);
        let target = located(dest, &rel)?;
        if dir {
            ensure_dir(&target)?;
            opened = file.skip()?;
            continue;
        }
        if size > MAX_BYTES || written.saturating_add(size) > MAX_BYTES {
            return Err(Fail::Message("archive expands past 8 GiB".into()));
        }
        match keep(&target, &rel, &mut policy, conflict)? {
            Keep::Skip => {
                opened = file.skip()?;
                continue;
            }
            Keep::Cancel => return Ok(Report { cancelled: true, files }),
            Keep::Write => {}
        }
        if let Some(parent) = target.parent() {
            ensure_dir(parent)?;
        }
        file_index += 1;
        let note = unpack_note(path, file_index, file_total, &name);
        hook_file(written, note.clone());
        emit_progress(written, total, rel.clone(), note.clone());
        opened = match file.extract_to(&target) {
            Ok(next) => next,
            Err(_) if cancel.load(Ordering::Relaxed) => {
                let _ = fs::remove_file(&target);
                return Ok(Report { cancelled: true, files });
            }
            Err(err) => {
                let _ = fs::remove_file(&target);
                return Err(err.into());
            }
        };
        if fs::symlink_metadata(&target).is_ok_and(|meta| meta.file_type().is_symlink()) {
            let _ = fs::remove_file(&target);
            return Err(Fail::Message(format!("refusing symlink {}", rel.display())));
        }
        let len = fs::metadata(&target)?.len();
        if written.saturating_add(len) > MAX_BYTES {
            let _ = fs::remove_file(&target);
            return Err(Fail::Message("archive expands past 8 GiB".into()));
        }
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644))?;
        written += len;
        files += 1;
        emit_progress(written, total, rel, note);
    }
    Ok(Report { cancelled: false, files })
}

fn open_rar<'a>(path: &'a Path, password: Option<&'a str>) -> unrar_ng::Archive<'a> {
    match password {
        Some(password) => unrar_ng::Archive::with_password(path, password),
        None => unrar_ng::Archive::new(path),
    }
}

fn rar_path(path: &Path) -> PathBuf {
    let first = unrar_ng::Archive::new(path).first_part();
    if first.as_path() != path && first.is_file() { first } else { path.to_path_buf() }
}

fn rar_symlink(attr: u32) -> bool {
    (attr >> 16) & 0o170000 == 0o120000
}

fn slash(name: &str) -> Option<String> {
    let rel = safe_rel(name)?;
    let mut out = String::new();
    for part in rel.components() {
        let Component::Normal(part) = part else { return None };
        let part = part.to_str()?;
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    (!out.is_empty()).then_some(out)
}

fn safe_rel(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let path = Path::new(name);
    if path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn located(root: &Path, rel: &Path) -> Result<PathBuf, Fail> {
    if fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(Fail::Message("destination is a symlink".into()));
    }
    let mut cur = root.to_path_buf();
    let mut parts: Vec<_> = rel.components().collect();
    let Some(last) = parts.pop() else {
        return Err(Fail::Message("empty archive name".into()));
    };
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(Fail::Message("unsafe archive name".into()));
        };
        cur.push(name);
        match fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(Fail::Message("symlink in destination".into())),
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(Fail::Message(format!("{} is not a folder", cur.display()))),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&cur)?;
                fs::set_permissions(&cur, fs::Permissions::from_mode(0o755))?;
            }
            Err(err) => return Err(err.into()),
        }
    }
    let Component::Normal(name) = last else {
        return Err(Fail::Message("unsafe archive name".into()));
    };
    cur.push(name);
    Ok(cur)
}

fn ensure_dir(path: &Path) -> Result<(), Fail> {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(Fail::Message("symlink in destination".into()));
    }
    if path.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Policy {
    Ask,
    Overwrite,
    Skip,
}

enum Keep {
    Write,
    Skip,
    Cancel,
}

fn keep(
    dest: &Path,
    src: &Path,
    policy: &mut Policy,
    conflict: &mut dyn FnMut(&Path, &Path) -> Choice,
) -> Result<Keep, Fail> {
    if fs::symlink_metadata(dest).is_err() {
        return Ok(Keep::Write);
    }
    let choice = match *policy {
        Policy::Ask => conflict(src, dest),
        Policy::Overwrite => Choice::Overwrite,
        Policy::Skip => Choice::Skip,
    };
    match choice {
        Choice::Cancel => Ok(Keep::Cancel),
        Choice::Skip => Ok(Keep::Skip),
        Choice::SkipAll => {
            *policy = Policy::Skip;
            Ok(Keep::Skip)
        }
        Choice::OverwriteAll => {
            *policy = Policy::Overwrite;
            clear_dest(dest)?;
            Ok(Keep::Write)
        }
        Choice::Overwrite => {
            clear_dest(dest)?;
            Ok(Keep::Write)
        }
    }
}

fn clear_dest(dest: &Path) -> Result<(), Fail> {
    let meta = fs::symlink_metadata(dest)?;
    if meta.file_type().is_symlink() || meta.is_file() {
        fs::remove_file(dest)?;
    } else {
        fs::remove_dir_all(dest)?;
    }
    Ok(())
}

fn write_reader(
    input: &mut impl Read,
    dest: &Path,
    declared: u64,
    written: &mut u64,
    total: u64,
    current: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<(), Fail> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dest)?;
    let mut buf = vec![0u8; 1024 * 1024];
    let mut got = 0u64;
    let room = MAX_BYTES.saturating_sub(*written);
    let limit = declared.min(room);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Fail::Cancelled);
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        got += n as u64;
        if got > limit {
            return Err(Fail::Message("archive expands past 8 GiB".into()));
        }
        output.write_all(&buf[..n])?;
        *written += n as u64;
        progress(&Progress { done: *written, total, current: current.to_path_buf(), note: String::new() });
    }
    output.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn tmp(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "comarchy-arc-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn zip_with(dir: &Path, files: &[(&str, &str)], link: Option<(&str, &str)>) -> PathBuf {
        let path = dir.join("sample.zip");
        let file = File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        if let Some((name, target)) = link {
            zip.add_symlink(name, target, opts).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn no_conflict(_: &Path, _: &Path) -> Choice {
        Choice::Cancel
    }

    #[test]
    fn unpacks_zip_tree_and_skips_escape_and_symlink() {
        let dir = tmp("zip");
        let archive = zip_with(
            &dir,
            &[("ok.txt", "hello"), ("sub/note.txt", "nested"), ("../evil.txt", "nope"), ("/tmp/comarchy-abs.txt", "nope")],
            Some(("link", "ok.txt")),
        );
        let dest = dir.join("out");
        fs::create_dir(&dest).unwrap();
        let cancel = AtomicBool::new(false);
        let report = extract(&archive, &dest, None, None, &cancel, &mut |_| {}, &mut no_conflict).unwrap();
        assert!(!report.cancelled);
        assert_eq!(fs::read_to_string(dest.join("ok.txt")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join("sub/note.txt")).unwrap(), "nested");
        assert!(!dir.join("evil.txt").exists());
        assert!(!Path::new("/tmp/comarchy-abs.txt").exists());
        assert!(!dest.join("link").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpacks_rar_fixture() {
        let dir = tmp("rar");
        let archive = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/version.rar");
        let dest = dir.join("out");
        fs::create_dir(&dest).unwrap();
        let cancel = AtomicBool::new(false);
        extract(&archive, &dest, None, None, &cancel, &mut |_| {}, &mut no_conflict).unwrap();
        assert_eq!(fs::read(dest.join("VERSION")).unwrap(), b"unrar-0.4.0");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rar_root_is_the_first_volume() {
        assert!(is_rar_root("Movie.rar"));
        assert!(is_rar_root("Movie.part01.rar"));
        assert!(is_rar_root("Movie.part1.rar"));
        assert!(is_rar_root("Movie.PART01.RAR"));
        assert!(!is_rar_root("Movie.part02.rar"));
        assert!(!is_rar_root("Movie.part10.rar"));
        assert!(!is_rar_root("Movie.r00"));
        assert_eq!(unpack_mark("Movie.rar"), "R");
        assert_eq!(unpack_mark("Movie.part02.rar"), "");
        assert_eq!(unpack_mark("notes.ZIP"), "Z");
        assert_eq!(unpack_mark("notes.txt"), "");
    }

    #[test]
    fn cancel_stops_rar_extract() {
        let dir = tmp("rar-cancel");
        let archive = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/version.rar");
        let dest = dir.join("out");
        fs::create_dir(&dest).unwrap();
        let cancel = AtomicBool::new(false);
        let report = extract(&archive, &dest, None, None, &cancel, &mut |_| cancel.store(true, Ordering::Relaxed), &mut no_conflict).unwrap();
        assert!(report.cancelled);
        assert!(!dest.join("VERSION").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
