use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::raw::c_int;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ops::{Choice, Progress, Report};


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
    Damaged(String),
    Message(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::NeedPassword => write!(f, "Password required"),
            Fail::BadPassword => write!(f, "Wrong password"),
            Fail::Cancelled => write!(f, "Cancelled"),
            Fail::Damaged(why) => write!(f, "Damaged data: {why}"),
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
            other @ zip::result::ZipError::InvalidArchive(_) => Fail::Message(format!("Archive is damaged or incomplete ({other})")),
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

pub fn unpack_mark(name: &str) -> &'static str {
    if name.len() >= 4 && name.as_bytes()[name.len() - 4..].eq_ignore_ascii_case(b".zip") {
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
    let mut verified = false;
    for index in 0..archive.len() {
        // Raw access reads metadata only; decrypting AES costs a key derivation per entry.
        let file = archive.by_index_raw(index)?;
        let encrypted = file.encrypted();
        let member = zip_name(&file)
            .filter(|_| !file.is_symlink())
            .map(|name| Member { name, size: file.size(), dir: file.is_dir() });
        drop(file);
        if encrypted && password.is_none() {
            listed.needs_password = true;
            continue;
        }
        if encrypted && !verified {
            zip_at(&mut archive, index, password)?;
            verified = true;
        }
        listed.members.extend(member);
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
        let file = archive.by_index_raw(index)?;
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
    ensure_room(dest, total)?;
    let file_total = plan.iter().filter(|item| !item.2).count() as u64;
    let mut file_index = 0u64;
    let mut written = 0u64;
    let mut files = 0u64;
    let mut damaged = Vec::new();
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
        match keep(&target, &rel, &mut policy, conflict)? {
            Keep::Skip => continue,
            Keep::Cancel => return Ok(Report { cancelled: true, files }),
            Keep::Write => {}
        }
        let mut file = zip_at(&mut archive, index, password)?;
        let encrypted = file.encrypted();
        // Only the exec bits come from the archive. The umask applies as for any new file.
        let mode = if file.unix_mode().is_some_and(|mode| mode & 0o111 != 0) { 0o755 } else { 0o644 };
        let time = zip_time(&file);
        file_index += 1;
        let note = format!("{}\n{}", volume_label(path), file_line(file_index, file_total, &name));
        let mut step = |update: &Progress| {
            let mut update = update.clone();
            update.note = note.clone();
            progress(&update);
        };
        match write_reader(&mut file, &target, mode, size, &mut written, total, &rel, cancel, &mut step) {
            Ok(out) => {
                if let Some(time) = time {
                    let _ = out.set_modified(time);
                }
                files += 1;
            }
            Err(Fail::Cancelled) => {
                let _ = fs::remove_file(&target);
                return Ok(Report { cancelled: true, files });
            }
            Err(Fail::Damaged(why)) => {
                let _ = fs::remove_file(&target);
                // ZipCrypto lets 1 in 256 wrong passwords through to the CRC check.
                if encrypted {
                    return Err(Fail::BadPassword);
                }
                damaged.push(format!("{name}  ({why})"));
            }
            Err(err) => {
                let _ = fs::remove_file(&target);
                return Err(err);
            }
        }
    }
    finish(files, damaged)
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

// The UTC extended timestamp when there is one. Else the DOS field, which is local time.
fn zip_time(file: &zip::read::ZipFile<'_, impl Read>) -> Option<std::time::SystemTime> {
    let utc = file.extra_data_fields().find_map(|field| match field {
        zip::ExtraField::ExtendedTimestamp(stamp) => stamp.mod_time(),
        _ => None,
    });
    let secs = match utc {
        Some(secs) => i64::from(secs),
        None => {
            let dos = file.last_modified()?;
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            tm.tm_year = i32::from(dos.year()) - 1900;
            tm.tm_mon = i32::from(dos.month()) - 1;
            tm.tm_mday = i32::from(dos.day());
            tm.tm_hour = i32::from(dos.hour());
            tm.tm_min = i32::from(dos.minute());
            tm.tm_sec = i32::from(dos.second());
            tm.tm_isdst = -1;
            i64::from(unsafe { libc::mktime(&mut tm) })
        }
    };
    let secs = u64::try_from(secs).ok()?;
    std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
}

fn list_rar(path: &Path, password: Option<&str>) -> Result<Listed, Fail> {
    let owned = rar_path(path);
    let opened = open_rar(&owned, password).open_for_listing().map_err(|err| in_volume(err.into(), &volume_label(&owned)))?;
    let mut listed = Listed { members: Vec::new(), needs_password: false };
    for item in opened {
        let header = item.map_err(|err| in_volume(err.into(), &volume_label(&owned)))?;
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

fn volume_label(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "archive".into())
}

fn in_volume(fail: Fail, volume: &str) -> Fail {
    match fail {
        Fail::Message(text) => Fail::Message(format!("{text} in {volume}")),
        other => other,
    }
}

fn file_line(index: u64, total_files: u64, name: &str) -> String {
    if total_files > 1 {
        format!("File {index} of {total_files}   {name}")
    } else {
        name.to_string()
    }
}

fn finish(files: u64, damaged: Vec<String>) -> Result<Report, Fail> {
    if damaged.is_empty() {
        return Ok(Report { cancelled: false, files });
    }
    let mut text = format!("Archive is damaged: {} failed, {files} unpacked OK", damaged.len());
    for line in damaged.iter().take(20) {
        text.push_str("\n  ");
        text.push_str(line);
    }
    if damaged.len() > 20 {
        text.push_str(&format!("\n  ... {} more", damaged.len() - 20));
    }
    Err(Fail::Message(text))
}

// Shared through the unrar callback's user data, so every field uses interior mutability.
struct RarJob<'a> {
    cancel: &'a AtomicBool,
    progress: RefCell<&'a mut dyn FnMut(&Progress)>,
    total: u64,
    base: Cell<u64>,
    limit: Cell<u64>,
    seen: Cell<u64>,
    sent: Cell<u64>,
    too_big: Cell<bool>,
    volume: RefCell<String>,
    missing: RefCell<Option<String>>,
    line: RefCell<String>,
}

impl RarJob<'_> {
    fn start(&self, base: u64, limit: u64, line: String) {
        self.base.set(base);
        self.limit.set(limit);
        self.seen.set(0);
        self.sent.set(0);
        *self.line.borrow_mut() = line;
    }

    fn report(&self, done: u64, current: PathBuf) {
        let note = format!("{}\n{}", self.volume.borrow(), self.line.borrow());
        (self.progress.borrow_mut())(&Progress { done, total: self.total, current, note });
    }

    fn change_volume(&self, name: String) -> c_int {
        *self.volume.borrow_mut() = name;
        self.report(self.base.get().saturating_add(self.seen.get()), PathBuf::new());
        if self.cancel.load(Ordering::Relaxed) { -1 } else { 0 }
    }

    fn data(&self, count: u64) -> c_int {
        if self.cancel.load(Ordering::Relaxed) {
            return -1;
        }
        let seen = self.seen.get().saturating_add(count);
        self.seen.set(seen);
        if seen > self.limit.get() {
            self.too_big.set(true);
            return -1;
        }
        let done = self.base.get().saturating_add(seen);
        if self.sent.get() == 0 || seen - self.sent.get() >= 256 * 1024 {
            self.sent.set(seen);
            self.report(done, PathBuf::new());
        }
        if self.cancel.load(Ordering::Relaxed) { -1 } else { 0 }
    }
}

extern "C" fn on_rar_event(
    msg: unrar_ng_sys::UINT,
    user: unrar_ng_sys::LPARAM,
    p1: unrar_ng_sys::LPARAM,
    p2: unrar_ng_sys::LPARAM,
) -> c_int {
    if user == 0 {
        return -1;
    }
    let job = unsafe { &*(user as *const RarJob<'_>) };
    match msg {
        unrar_ng_sys::UCM_PROCESSDATA => job.data(p2.max(0) as u64),
        unrar_ng_sys::UCM_CHANGEVOLUMEW => {
            let name = volume_label(Path::new(&unsafe { wide_ptr(p1 as *const libc::wchar_t) }));
            if p2 == unrar_ng_sys::RAR_VOL_ASK {
                *job.missing.borrow_mut() = Some(name);
                -1
            } else {
                job.change_volume(name)
            }
        }
        // The password, if any, is set before reading headers.
        unrar_ng_sys::UCM_NEEDPASSWORD | unrar_ng_sys::UCM_NEEDPASSWORDW => -1,
        _ => 0,
    }
}

struct RarHandle(*const unrar_ng_sys::Handle);

impl Drop for RarHandle {
    fn drop(&mut self) {
        unsafe { unrar_ng_sys::RARCloseArchive(self.0) };
    }
}

fn rar_fail(code: c_int, when: unrar_ng::error::When) -> Fail {
    unrar_ng::error::UnrarError::from(unrar_ng::error::Code::from(code), when).into()
}

fn c_path(path: &Path) -> Result<CString, Fail> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| Fail::Message(format!("bad path {}", path.display())))
}

fn open_rar_extract(path: &Path, password: Option<&str>) -> Result<RarHandle, Fail> {
    let name = c_path(path)?;
    let mut data = unrar_ng_sys::OpenArchiveDataEx::new(name.as_ptr(), unrar_ng_sys::RAR_OM_EXTRACT);
    let raw = unsafe { unrar_ng_sys::RAROpenArchiveEx(&raw mut data) };
    let result = data.open_result as c_int;
    if raw.is_null() {
        return Err(rar_fail(result, unrar_ng::error::When::Open));
    }
    let handle = RarHandle(raw);
    if result != unrar_ng_sys::ERAR_SUCCESS {
        return Err(rar_fail(result, unrar_ng::error::When::Open));
    }
    if let Some(password) = password {
        let password = CString::new(password).map_err(|_| Fail::BadPassword)?;
        unsafe { unrar_ng_sys::RARSetPassword(handle.0, password.as_ptr()) };
    }
    Ok(handle)
}

fn wide_text(chars: &[libc::wchar_t]) -> String {
    chars.iter().take_while(|&&c| c != 0).map(|&c| char::from_u32(c as u32).unwrap_or('?')).collect()
}

// unrar passes volume names as NUL-terminated wide strings of at most 2048 chars.
unsafe fn wide_ptr(ptr: *const libc::wchar_t) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let len = (0..2048).take_while(|&i| unsafe { *ptr.add(i) } != 0).count();
    wide_text(unsafe { std::slice::from_raw_parts(ptr, len) })
}

fn rar_header_name(header: &unrar_ng_sys::HeaderDataEx) -> Option<String> {
    let wide = header.filename_w;
    let name: Option<String> = wide.iter().take_while(|&&c| c != 0).map(|&c| char::from_u32(c as u32)).collect();
    // RAR from Windows stores backslash separators.
    slash(&name?.replace('\\', "/"))
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
    let chosen = |member: &&Member| !member.dir && names.is_none_or(|set| set.contains(&member.name));
    let total = listed.members.iter().filter(chosen).map(|m| m.size).sum();
    ensure_room(dest, total)?;
    let file_total = listed.members.iter().filter(chosen).count() as u64;
    let mut file_index = 0u64;
    let job = RarJob {
        cancel,
        progress: RefCell::new(progress),
        total,
        base: Cell::new(0),
        limit: Cell::new(0),
        seen: Cell::new(0),
        sent: Cell::new(0),
        too_big: Cell::new(false),
        volume: RefCell::new(volume_label(&owned)),
        missing: RefCell::new(None),
        line: RefCell::new(String::new()),
    };
    // Declared after `job` so the archive closes before the callback context goes away.
    let handle = open_rar_extract(&owned, password)?;
    unsafe { unrar_ng_sys::RARSetCallback(handle.0, Some(on_rar_event), &raw const job as unrar_ng_sys::LPARAM) };
    let fail = |code: c_int, when: unrar_ng::error::When| -> Fail {
        if let Some(volume) = job.missing.borrow_mut().take() {
            return Fail::Message(format!("Missing volume {volume}\nPut all parts of the archive in the same folder."));
        }
        in_volume(rar_fail(code, when), &job.volume.borrow())
    };
    let skip = |handle: &RarHandle| -> Result<(), Fail> {
        let code = unsafe { unrar_ng_sys::RARProcessFile(handle.0, unrar_ng_sys::RAR_SKIP, ptr::null(), ptr::null()) };
        if code == unrar_ng_sys::ERAR_SUCCESS { Ok(()) } else { Err(fail(code, unrar_ng::error::When::Process)) }
    };
    let mut header = Box::new(unrar_ng_sys::HeaderDataEx::default());
    let mut written = 0u64;
    let mut files = 0u64;
    let mut damaged = Vec::new();
    let mut policy = Policy::Ask;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report { cancelled: true, files });
        }
        match unsafe { unrar_ng_sys::RARReadHeaderEx(handle.0, &raw mut *header) } {
            unrar_ng_sys::ERAR_SUCCESS => {}
            unrar_ng_sys::ERAR_END_ARCHIVE => break,
            _ if cancel.load(Ordering::Relaxed) => return Ok(Report { cancelled: true, files }),
            unrar_ng_sys::ERAR_BAD_DATA if job.missing.borrow().is_none() => {
                damaged.push(format!("file header  (damaged in {})", job.volume.borrow()));
                break;
            }
            code => return Err(fail(code, unrar_ng::error::When::Read)),
        }
        let arc_name = header.archive_name_w;
        *job.volume.borrow_mut() = volume_label(Path::new(&wide_text(&arc_name)));
        let flags = header.flags;
        let dir = flags & unrar_ng_sys::RHDF_DIRECTORY != 0;
        let size = ((header.unp_size_high as u64) << 32) | header.unp_size as u64;
        let name = rar_header_name(&header);
        let wanted = name.as_ref().is_some_and(|name| names.is_none_or(|set| set.contains(name)));
        if !wanted || rar_symlink(header.file_attr) {
            skip(&handle)?;
            continue;
        }
        let name = name.unwrap();
        let rel = PathBuf::from(&name);
        let target = located(dest, &rel)?;
        if dir {
            ensure_dir(&target)?;
            skip(&handle)?;
            continue;
        }
        match keep(&target, &rel, &mut policy, conflict)? {
            Keep::Skip => {
                skip(&handle)?;
                continue;
            }
            Keep::Cancel => return Ok(Report { cancelled: true, files }),
            Keep::Write => {}
        }
        if let Some(parent) = target.parent() {
            ensure_dir(parent)?;
        }
        file_index += 1;
        job.start(written, size, file_line(file_index, file_total, &name));
        job.report(written, rel.clone());
        let dest_name = c_path(&target)?;
        let code = unsafe { unrar_ng_sys::RARProcessFile(handle.0, unrar_ng_sys::RAR_EXTRACT, ptr::null(), dest_name.as_ptr()) };
        if code != unrar_ng_sys::ERAR_SUCCESS {
            let _ = fs::remove_file(&target);
            if cancel.load(Ordering::Relaxed) {
                return Ok(Report { cancelled: true, files });
            }
            if job.too_big.get() {
                return Err(oversized(&rel));
            }
            // RAR4 reports a wrong password as a CRC error.
            if code == unrar_ng_sys::ERAR_BAD_DATA && password.is_some() && flags & unrar_ng_sys::RHDF_ENCRYPTED != 0 {
                return Err(Fail::BadPassword);
            }
            if code == unrar_ng_sys::ERAR_BAD_DATA && job.missing.borrow().is_none() {
                damaged.push(format!("{name}  (CRC error in {})", job.volume.borrow()));
                continue;
            }
            return Err(fail(code, unrar_ng::error::When::Process));
        }
        if fs::symlink_metadata(&target).is_ok_and(|meta| meta.file_type().is_symlink()) {
            let _ = fs::remove_file(&target);
            return Err(Fail::Message(format!("refusing symlink {}", rel.display())));
        }
        let meta = fs::metadata(&target)?;
        let len = meta.len();
        if len > size {
            let _ = fs::remove_file(&target);
            return Err(oversized(&rel));
        }
        // unrar restores the archived mode. Keep only its exec bits, as for zip.
        let mode = if meta.permissions().mode() & 0o111 != 0 { 0o755 } else { 0o644 };
        fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
        written += len;
        files += 1;
        job.report(written, rel);
    }
    finish(files, damaged)
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
    let Ok(meta) = fs::symlink_metadata(dest) else {
        return Ok(Keep::Write);
    };
    if meta.is_dir() {
        return Err(Fail::Message(format!("cannot replace folder {} with a file", dest.display())));
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
    fs::remove_file(dest)?;
    Ok(())
}

fn oversized(rel: &Path) -> Fail {
    Fail::Message(format!("{} unpacks larger than the archive declares", rel.display()))
}

// Free bytes for unprivileged users on the filesystem holding `path` (or its nearest existing parent).
fn free_bytes(path: &Path) -> Option<u64> {
    path.ancestors().find_map(|dir| {
        let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
            return None;
        }
        let st = unsafe { st.assume_init() };
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    })
}

fn ensure_room(dest: &Path, need: u64) -> Result<(), Fail> {
    match free_bytes(dest) {
        Some(free) if free < need => Err(Fail::Message(format!(
            "Not enough space in {}: needs {}, {} free",
            dest.display(),
            crate::listing::format_size(need),
            crate::listing::format_size(free)
        ))),
        _ => Ok(()),
    }
}

fn write_reader(
    input: &mut impl Read,
    dest: &Path,
    mode: u32,
    declared: u64,
    written: &mut u64,
    total: u64,
    current: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<File, Fail> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dest)?;
    // Sized to the member so small files don't each zero a 1 MiB buffer.
    let mut buf = vec![0u8; declared.saturating_add(1).min(1024 * 1024) as usize];
    let mut got = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Fail::Cancelled);
        }
        // Decoders hand out a few KiB per read; fill the buffer so writes and progress updates stay large.
        let mut n = 0;
        while n < buf.len() {
            let count = input.read(&mut buf[n..]).map_err(|err| match err.kind() {
                io::ErrorKind::InvalidData => Fail::Damaged(err.to_string()),
                _ => err.into(),
            })?;
            if count == 0 {
                break;
            }
            n += count;
        }
        if n == 0 {
            break;
        }
        got += n as u64;
        if got > declared {
            return Err(oversized(current));
        }
        output.write_all(&buf[..n])?;
        *written += n as u64;
        progress(&Progress { done: *written, total, current: current.to_path_buf(), note: String::new() });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn tmp(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rust-commander-arc-{}-{}-{name}",
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
            &[("ok.txt", "hello"), ("sub/note.txt", "nested"), ("../evil.txt", "nope"), ("/tmp/rust-commander-abs.txt", "nope")],
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
        assert!(!Path::new("/tmp/rust-commander-abs.txt").exists());
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
        let mut notes = Vec::new();
        extract(&archive, &dest, None, None, &cancel, &mut |p| notes.push(p.note.clone()), &mut no_conflict).unwrap();
        assert_eq!(fs::read(dest.join("VERSION")).unwrap(), b"unrar-0.4.0");
        // Stored as 2015-08-07.
        let time = fs::metadata(dest.join("VERSION")).unwrap().modified().unwrap();
        assert!(time < std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000), "{time:?}");
        assert!(notes.iter().any(|note| note == "version.rar\nVERSION"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_rar_names_file_and_volume() {
        let dir = tmp("rar-bad");
        let mut bytes = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/version.rar")).unwrap();
        // Inside the packed data of VERSION, past its header.
        bytes[0x48] ^= 0xff;
        let archive = dir.join("broken.rar");
        fs::write(&archive, bytes).unwrap();
        let dest = dir.join("out");
        fs::create_dir(&dest).unwrap();
        let cancel = AtomicBool::new(false);
        let err = extract(&archive, &dest, None, None, &cancel, &mut |_| {}, &mut no_conflict).unwrap_err().to_string();
        assert!(err.starts_with("Archive is damaged: 1 failed, 0 unpacked OK"), "{err}");
        assert!(err.contains("VERSION  (CRC error in broken.rar)"), "{err}");
        assert!(!dest.join("VERSION").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_zip_member_is_reported_and_others_kept() {
        let dir = tmp("zip-bad");
        let archive = dir.join("bad.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in [("a.txt", "first payload"), ("b.txt", "second payload")] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let mut bytes = fs::read(&archive).unwrap();
        let at = bytes.windows(13).position(|w| w == b"first payload").unwrap();
        bytes[at] ^= 0x20;
        fs::write(&archive, bytes).unwrap();
        let dest = dir.join("out");
        fs::create_dir(&dest).unwrap();
        let cancel = AtomicBool::new(false);
        let err = extract(&archive, &dest, None, None, &cancel, &mut |_| {}, &mut no_conflict).unwrap_err().to_string();
        assert!(err.starts_with("Archive is damaged: 1 failed, 1 unpacked OK"), "{err}");
        assert!(err.contains("a.txt"), "{err}");
        assert!(!dest.join("a.txt").exists());
        assert_eq!(fs::read_to_string(dest.join("b.txt")).unwrap(), "second payload");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_keeps_unpack_mark() {
        assert_eq!(unpack_mark("Movie.rar"), "");
        assert_eq!(unpack_mark("notes.ZIP"), "Z");
        assert_eq!(unpack_mark("notes.txt"), "");
    }

    #[test]
    fn rar_header_names_are_safe() {
        let named = |name: &str| {
            let mut wide = [0; 1024];
            for (slot, c) in wide.iter_mut().zip(name.chars()) {
                *slot = c as libc::wchar_t;
            }
            let mut header = Box::new(unrar_ng_sys::HeaderDataEx::default());
            header.filename_w = wide;
            rar_header_name(&header)
        };
        assert_eq!(named("dir\\möte.md").as_deref(), Some("dir/möte.md"));
        assert_eq!(named("..\\evil.txt"), None);
        assert_eq!(named("/etc/passwd"), None);
        assert_eq!(named(""), None);
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

    #[test]
    fn room_check_uses_free_space_not_fixed_cap() {
        let dir = tmp("room");
        let free = free_bytes(&dir.join("missing/child")).unwrap();
        assert!(ensure_room(&dir, free / 2).is_ok());
        assert!(matches!(ensure_room(&dir, u64::MAX), Err(Fail::Message(msg)) if msg.starts_with("Not enough space")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_reader_rejects_data_past_declared_size() {
        let dir = tmp("declared");
        let target = dir.join("out.bin");
        let cancel = AtomicBool::new(false);
        let mut written = 0;
        let err = write_reader(&mut &b"too long"[..], &target, 0o644, 3, &mut written, 3, Path::new("out.bin"), &cancel, &mut |_| {});
        assert!(matches!(err, Err(Fail::Message(msg)) if msg.contains("larger than the archive declares")));
        let _ = fs::remove_dir_all(&dir);
    }
}
