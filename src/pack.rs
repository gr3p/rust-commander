use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::archive::Fail;
use crate::ops::{Progress, Report};

pub const SEVEN_ZIP_HINT: &str = "Point 7-Zip at the 7z program, for example /usr/bin/7z or /usr/bin/7zz. The package is 7zip or p7zip-full. This program creates .7z. It does not create .rar.";
pub const RAR_HINT: &str = "Point RAR at RARLAB's rar program, for example /usr/bin/rar. unrar only unpacks. 7z does not create .rar. Unpacking RAR does not need this path.";

pub const LEVELS: [(&str, u8); 6] = [("Store", 0), ("Fastest", 1), ("Fast", 3), ("Normal", 5), ("Maximum", 7), ("Ultra", 9)];
pub const DEFAULT_LEVEL: usize = 3;

#[derive(Clone)]
pub struct Options {
    /// 0 stores, 9 is the strongest.
    pub level: u8,
    pub password: Option<String>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options").field("level", &self.level).field("password", &self.password.is_some()).finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    SevenZ,
    Rar,
}

pub fn format_of(path: &Path) -> Option<Format> {
    match path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()).as_deref() {
        Some("zip") => Some(Format::Zip),
        Some("7z") => Some(Format::SevenZ),
        Some("rar") => Some(Format::Rar),
        _ => None,
    }
}

pub fn extension(format: Format) -> &'static str {
    match format {
        Format::Zip => "zip",
        Format::SevenZ => "7z",
        Format::Rar => "rar",
    }
}

pub fn hint(format: Format) -> &'static str {
    match format {
        Format::Zip => "",
        Format::SevenZ => SEVEN_ZIP_HINT,
        Format::Rar => RAR_HINT,
    }
}

pub fn names(format: Format) -> &'static [&'static str] {
    match format {
        Format::Zip => &[],
        Format::SevenZ => &["7z", "7zz"],
        Format::Rar => &["rar"],
    }
}

pub fn find_program(configured: &str, names: &[&str]) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        return path.is_file().then_some(path);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn pack(
    format: Format,
    bin: Option<&Path>,
    sources: &[PathBuf],
    cwd: &Path,
    dest: &Path,
    opts: &Options,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Report, Fail> {
    if sources.is_empty() {
        return Err(Fail::Message("Nothing selected".into()));
    }
    let password = opts.password.as_deref().filter(|text| !text.is_empty());
    if password.is_some_and(|text| text.contains(['\n', '\r', '\0'])) {
        return Err(Fail::Message("Password cannot contain line breaks".into()));
    }
    let level = opts.level.min(9);
    let Some(parent) = dest.parent() else {
        return Err(Fail::Message("missing destination folder".into()));
    };
    if !parent.is_dir() {
        return Err(Fail::Message(format!("folder does not exist: {}", parent.display())));
    }
    let part = part_path(dest);
    let packed = match format {
        Format::Zip => create_zip(sources, cwd, &part, level, password, cancel, progress),
        Format::SevenZ | Format::Rar => {
            let Some(bin) = bin else {
                return Err(Fail::Message(hint(format).into()));
            };
            run_tool(format, bin, sources, cwd, &part, level, password, cancel, progress)
        }
    };
    match packed {
        Ok(files) => {
            fs::rename(&part, dest)?;
            Ok(Report { cancelled: false, files })
        }
        Err(Fail::Cancelled) => {
            let _ = fs::remove_file(&part);
            Ok(Report { cancelled: true, files: 0 })
        }
        Err(err) => {
            let _ = fs::remove_file(&part);
            Err(err)
        }
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

fn create_zip(
    sources: &[PathBuf],
    cwd: &Path,
    dest: &Path,
    level: u8,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<u64, Fail> {
    let mut entries = Vec::new();
    for source in sources {
        walk(source, cwd, &mut entries)?;
    }
    let total: u64 = entries.iter().filter_map(|(_, _, len)| *len).sum();
    let mut file = File::create(dest)?;
    let mut zip = zip::ZipWriter::new(&mut file);
    let opts = if level == 0 {
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
    } else {
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(i64::from(level)))
    };
    let file_opts = match password {
        Some(password) => opts.with_aes_encryption(zip::AesMode::Aes256, password),
        None => opts,
    };
    let mut done = 0u64;
    let mut files = 0u64;
    for (path, rel, len) in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(Fail::Cancelled);
        }
        let name = rel_slash(&rel);
        if name.contains('\n') {
            return Err(Fail::Message(format!("name has a newline: {name}")));
        }
        if len.is_none() {
            let dir = if name.ends_with('/') { name } else { format!("{name}/") };
            zip.add_directory(dir, opts)?;
            continue;
        }
        let large = len.is_some_and(|len| len >= u64::from(u32::MAX));
        zip.start_file(name, file_opts.clone().large_file(large))?;
        let mut input = File::open(&path)?;
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Fail::Cancelled);
            }
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            zip.write_all(&buf[..n])?;
            done += n as u64;
            progress(&Progress { done, total, current: rel.clone(), note: String::new() });
        }
        files += 1;
    }
    zip.finish()?;
    Ok(files)
}

fn walk(path: &Path, cwd: &Path, out: &mut Vec<(PathBuf, PathBuf, Option<u64>)>) -> Result<(), Fail> {
    let rel = path.strip_prefix(cwd).map_err(|_| Fail::Message("file is outside the folder".into()))?.to_path_buf();
    if rel.as_os_str().is_empty() {
        return Err(Fail::Message("cannot pack the folder into itself".into()));
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(Fail::Message(format!("symlink {}", path.display())));
    }
    if meta.is_dir() {
        out.push((path.to_path_buf(), rel, None));
        let mut children: Vec<_> = fs::read_dir(path)?.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<_>>()?;
        children.sort();
        for child in children {
            walk(&child, cwd, out)?;
        }
        Ok(())
    } else {
        out.push((path.to_path_buf(), rel, Some(meta.len())));
        Ok(())
    }
}

fn rel_slash(path: &Path) -> String {
    let mut out = String::new();
    for part in path.components() {
        if let std::path::Component::Normal(part) = part {
            if !out.is_empty() {
                out.push('/');
            }
            out.push_str(&part.to_string_lossy());
        }
    }
    out
}

fn run_tool(
    format: Format,
    bin: &Path,
    sources: &[PathBuf],
    cwd: &Path,
    part: &Path,
    level: u8,
    password: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<u64, Fail> {
    let list = PathBuf::from(format!("{}.list", part.display()));
    let mut body = String::new();
    for source in sources {
        let rel = source.strip_prefix(cwd).map_err(|_| Fail::Message("file is outside the folder".into()))?;
        let text = rel_slash(rel);
        if text.is_empty() || text.contains('\n') {
            return Err(Fail::Message(format!("cannot list {}", source.display())));
        }
        body.push_str(&text);
        body.push('\n');
    }
    fs::write(&list, body)?;
    let run = (|| {
        let mut cmd = Command::new(bin);
        let stdin = if password.is_some() { Stdio::piped() } else { Stdio::null() };
        cmd.current_dir(cwd).stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::piped());
        match format {
            Format::SevenZ => {
                cmd.args(["a", "-y", "-bso0", "-bsp1", "-bse2"]).arg(format!("-mx={level}"));
                if password.is_some() {
                    cmd.args(["-p", "-mhe=on"]);
                }
            }
            Format::Rar => {
                cmd.args(["a", "-r", "-y", "-idcd"]).arg(format!("-m{}", rar_method(level)));
                if password.is_some() {
                    cmd.arg("-hp");
                }
            }
            Format::Zip => {}
        }
        cmd.arg(part).arg(format!("@{}", list.display()));
        use std::os::unix::process::CommandExt;
        // New session without a controlling tty, so password prompts read stdin instead of /dev/tty.
        unsafe {
            cmd.pre_exec(|| if libc::setsid() < 0 { Err(io::Error::last_os_error()) } else { Ok(()) });
        }
        let mut child = cmd.spawn().map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                Fail::Message(hint(format).into())
            } else {
                Fail::Message(err.to_string())
            }
        })?;
        if let (Some(password), Some(mut input)) = (password, child.stdin.take()) {
            // Sent twice for tools that ask to repeat it. Kept off the command line, which other users can read.
            let _ = input.write_all(format!("{password}\n{password}\n").as_bytes());
        }
        drive(format, &mut child, part, cancel, progress)
    })();
    let _ = fs::remove_file(&list);
    run
}

fn rar_method(level: u8) -> u8 {
    match level {
        0 => 0,
        1 | 2 => 1,
        3 | 4 => 2,
        5 | 6 => 3,
        7 | 8 => 4,
        _ => 5,
    }
}

fn drive(
    format: Format,
    child: &mut Child,
    part: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<u64, Fail> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel::<Chunk>();
    if let Some(pipe) = stdout {
        spawn_reader(pipe, tx.clone(), true);
    }
    if let Some(pipe) = stderr {
        spawn_reader(pipe, tx.clone(), false);
    }
    drop(tx);
    let mut err = Vec::new();
    let mut killed = false;
    loop {
        if cancel.load(Ordering::Relaxed) && !killed {
            kill_group(child);
            killed = true;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Chunk::Out(bytes)) => report_percent(&bytes, part, progress),
            Ok(Chunk::Err(bytes)) => {
                report_percent(&bytes, part, progress);
                err.extend(bytes);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if child.try_wait()?.is_some() && rx.try_recv().is_err() {
                    // Keep looping until the readers disconnect.
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    if killed || cancel.load(Ordering::Relaxed) {
        return Err(Fail::Cancelled);
    }
    if status.success() {
        return Ok(1);
    }
    let text = String::from_utf8_lossy(&err);
    let tail = tail_lines(&text, 20);
    let message = if tail.is_empty() {
        format!("exit {}", status.code().map(|n| n.to_string()).unwrap_or_else(|| "signal".into()))
    } else {
        tail
    };
    let _ = format;
    Err(Fail::Message(message))
}

enum Chunk {
    Out(Vec<u8>),
    Err(Vec<u8>),
}

fn spawn_reader(mut pipe: impl Read + Send + 'static, tx: mpsc::Sender<Chunk>, out: bool) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = if out { Chunk::Out(buf[..n].to_vec()) } else { Chunk::Err(buf[..n].to_vec()) };
                    if tx.send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn report_percent(bytes: &[u8], current: &Path, progress: &mut dyn FnMut(&Progress)) {
    if let Some(done) = last_percent(bytes) {
        progress(&Progress { done, total: 100, current: current.to_path_buf(), note: String::new() });
    }
}

fn last_percent(bytes: &[u8]) -> Option<u64> {
    let start = bytes.len().saturating_sub(256);
    let window = &bytes[start..];
    let mut last = None;
    let mut i = 0;
    while i < window.len() {
        if window[i] == b'%' {
            let mut j = i;
            while j > 0 && window[j - 1].is_ascii_digit() {
                j -= 1;
            }
            if j < i {
                if let Ok(n) = std::str::from_utf8(&window[j..i]).unwrap_or("").parse::<u64>() {
                    if n <= 100 {
                        last = Some(n);
                    }
                }
            }
        }
        i += 1;
    }
    last
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<_> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

fn kill_group(child: &Child) {
    let pid = child.id() as i32;
    unsafe { libc::kill(-pid, libc::SIGKILL) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive;
    use std::sync::atomic::AtomicU64;

    fn tmp(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "comarchy-pack-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn zip_roundtrip() {
        let dir = tmp("zip");
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/b.txt"), b"beta").unwrap();
        let dest = dir.join("out.zip");
        let cancel = AtomicBool::new(false);
        let opts = Options { level: 5, password: None };
        pack(Format::Zip, None, &[dir.join("a.txt"), dir.join("sub")], &dir, &dest, &opts, &cancel, &mut |_| {}).unwrap();
        let out = dir.join("unpacked");
        fs::create_dir(&out).unwrap();
        archive::extract(&dest, &out, None, None, &cancel, &mut |_| {}, &mut |_, _| crate::ops::Choice::Cancel).unwrap();
        assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(out.join("sub/b.txt")).unwrap(), b"beta");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_stored_and_encrypted() {
        let dir = tmp("zipaes");
        fs::write(dir.join("a.txt"), b"alpha alpha alpha").unwrap();
        let dest = dir.join("out.zip");
        let cancel = AtomicBool::new(false);
        let opts = Options { level: 0, password: Some("hemligt".into()) };
        pack(Format::Zip, None, &[dir.join("a.txt")], &dir, &dest, &opts, &cancel, &mut |_| {}).unwrap();
        let listed = archive::list(&dest, None).unwrap();
        assert!(listed.needs_password);
        let out = dir.join("unpacked");
        fs::create_dir(&out).unwrap();
        let wrong = archive::extract(&dest, &out, None, Some("fel"), &cancel, &mut |_| {}, &mut |_, _| crate::ops::Choice::Cancel);
        assert!(wrong.is_err());
        archive::extract(&dest, &out, None, Some("hemligt"), &cancel, &mut |_| {}, &mut |_, _| crate::ops::Choice::Cancel).unwrap();
        assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha alpha alpha");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_password_with_newline() {
        let dir = tmp("zipnl");
        fs::write(dir.join("a.txt"), b"a").unwrap();
        let opts = Options { level: 5, password: Some("a\nb".into()) };
        let err = pack(Format::Zip, None, &[dir.join("a.txt")], &dir, &dir.join("out.zip"), &opts, &AtomicBool::new(false), &mut |_| {});
        assert!(err.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "packs a 4.5 GiB sparse file; run with --release -- --ignored"]
    fn zip_over_4_gib() {
        let dir = tmp("zip64");
        let big = dir.join("big.bin");
        File::create(&big).unwrap().set_len(4608 * 1024 * 1024).unwrap();
        fs::write(dir.join("small.txt"), b"small").unwrap();
        let dest = dir.join("out.zip");
        let opts = Options { level: 1, password: None };
        pack(Format::Zip, None, &[big.clone(), dir.join("small.txt")], &dir, &dest, &opts, &AtomicBool::new(false), &mut |_| {}).unwrap();
        let listed = archive::list(&dest, None).unwrap();
        let member = listed.members.iter().find(|member| member.name == "big.bin").unwrap();
        assert_eq!(member.size, 4608 * 1024 * 1024);
        if let Some(bin) = find_program("", names(Format::SevenZ)) {
            let tested = Command::new(bin).args(["t", dest.to_str().unwrap()]).output().unwrap();
            assert!(tested.status.success(), "{}", String::from_utf8_lossy(&tested.stdout));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rar_when_present() {
        let Some(bin) = find_program("", names(Format::Rar)) else { return };
        let dir = tmp("rar");
        fs::write(dir.join("a.txt"), b"alpha").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/b.txt"), b"beta").unwrap();
        let cancel = AtomicBool::new(false);
        for (name, password) in [("plain.rar", None), ("secret.rar", Some("hemligt"))] {
            let dest = dir.join(name);
            let opts = Options { level: 5, password: password.map(String::from) };
            pack(Format::Rar, Some(&bin), &[dir.join("a.txt"), dir.join("sub")], &dir, &dest, &opts, &cancel, &mut |_| {}).unwrap();
            if password.is_some() {
                assert!(archive::list(&dest, None).is_err() || archive::list(&dest, None).unwrap().needs_password);
                assert!(archive::list(&dest, Some("fel")).is_err());
            }
            let out = dir.join(format!("out-{name}"));
            fs::create_dir(&out).unwrap();
            archive::extract(&dest, &out, None, password, &cancel, &mut |_| {}, &mut |_, _| crate::ops::Choice::Cancel).unwrap();
            assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha");
            assert_eq!(fs::read(out.join("sub/b.txt")).unwrap(), b"beta");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn seven_zip_when_present() {
        let Some(bin) = find_program("", names(Format::SevenZ)) else { return };
        let dir = tmp("7z");
        fs::write(dir.join("note.txt"), b"seven").unwrap();
        let dest = dir.join("out.7z");
        let cancel = AtomicBool::new(false);
        let opts = Options { level: 9, password: None };
        pack(Format::SevenZ, Some(&bin), &[dir.join("note.txt")], &dir, &dest, &opts, &cancel, &mut |_| {}).unwrap();
        assert!(dest.is_file());
        let listed = Command::new(&bin).args(["l", dest.to_str().unwrap()]).output().unwrap();
        let text = String::from_utf8_lossy(&listed.stdout);
        assert!(text.contains("note.txt"), "{text}");
        let secret = dir.join("secret.7z");
        let opts = Options { level: 1, password: Some("hemligt".into()) };
        pack(Format::SevenZ, Some(&bin), &[dir.join("note.txt")], &dir, &secret, &opts, &cancel, &mut |_| {}).unwrap();
        let locked = Command::new(&bin).args(["l", "-pfel", secret.to_str().unwrap()]).output().unwrap();
        assert!(!locked.status.success());
        let open = Command::new(&bin).args(["l", "-phemligt", secret.to_str().unwrap()]).output().unwrap();
        assert!(String::from_utf8_lossy(&open.stdout).contains("note.txt"));
        let _ = fs::remove_dir_all(&dir);
    }
}
