use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::archive::Fail;
use crate::ops::{Progress, Report};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Md5,
    Sha1,
    Sha256,
}

impl Kind {
    pub fn extension(self) -> &'static str {
        match self {
            Kind::Md5 => "md5",
            Kind::Sha1 => "sha1",
            Kind::Sha256 => "sha256",
        }
    }

    fn width(self) -> usize {
        match self {
            Kind::Md5 => 32,
            Kind::Sha1 => 40,
            Kind::Sha256 => 64,
        }
    }
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    match path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()).as_deref() {
        Some("md5") => Some(Kind::Md5),
        Some("sha1") => Some(Kind::Sha1),
        Some("sha256") => Some(Kind::Sha256),
        _ => None,
    }
}

pub fn create(
    kind: Kind,
    root: &Path,
    sources: &[PathBuf],
    dest: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Report, Fail> {
    let mut files = Vec::new();
    for source in sources {
        crate::sfv::collect(source, root, dest, &mut files)?;
    }
    if files.is_empty() {
        return Err(Fail::Message("Nothing selected".into()));
    }
    let total = files.len() as u64;
    let mut out = String::new();
    for (index, (path, rel)) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report { cancelled: true, files: index as u64 });
        }
        let hash = match hash_file(kind, path, cancel) {
            Ok(hash) => hash,
            Err(Fail::Cancelled) => return Ok(Report { cancelled: true, files: index as u64 }),
            Err(err) => return Err(err),
        };
        out.push_str(&format!("{hash}  {rel}\n"));
        progress(&Progress { done: index as u64 + 1, total, current: path.clone(), note: String::new() });
    }
    crate::ops::write_atomic(dest, out.as_bytes())?;
    Ok(Report { cancelled: false, files: total })
}

pub fn verify(path: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(&Progress)) -> Result<Report, Fail> {
    let kind = kind_of(path).ok_or_else(|| Fail::Message("Not a checksum file".into()))?;
    let root = path.parent().unwrap_or(Path::new("."));
    let text = fs::read_to_string(path)?;
    // A line that cannot be read is a failure, so a file never passes unchecked.
    let mut rows = Vec::new();
    let mut issues = Vec::new();
    for row in text.lines().filter_map(|line| parse_line(kind, line)) {
        match row {
            Ok(row) => rows.push(row),
            Err(line) => issues.push(format!("bad line  {line}")),
        }
    }
    if rows.is_empty() && issues.is_empty() {
        return Err(Fail::Message("Nothing to check".into()));
    }
    let total = rows.len() as u64;
    let (mut ok, mut failed, mut missing) = (0u64, issues.len() as u64, 0u64);
    for (index, (name, expect)) in rows.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Report { cancelled: true, files: ok });
        }
        let done = index as u64;
        let note = crate::sfv::check_note(done, total, name, &issues);
        progress(&Progress { done, total, current: PathBuf::from(name), note });
        match check_one(kind, root, name, expect, cancel) {
            Check::Cancelled => return Ok(Report { cancelled: true, files: ok }),
            Check::Ok => ok += 1,
            Check::Missing => {
                missing += 1;
                issues.push(format!("missing  {name}"));
            }
            Check::Mismatch => {
                failed += 1;
                issues.push(format!("mismatch  {name}"));
            }
            Check::Bad => {
                failed += 1;
                issues.push(format!("bad path  {name}"));
            }
            Check::Error(err) => {
                failed += 1;
                issues.push(format!("{err}  {name}"));
            }
        }
        let note = crate::sfv::check_note(done + 1, total, name, &issues);
        progress(&Progress { done: done + 1, total, current: PathBuf::from(name), note });
    }
    if issues.is_empty() {
        return Ok(Report { cancelled: false, files: ok });
    }
    // The same layout as an SFV result, which the progress window reads.
    let shown = issues.len().min(crate::sfv::ISSUE_LIMIT);
    let mut text = format!("OK {ok}, failed {failed}, missing {missing}\n{}", issues[..shown].join("\n"));
    if issues.len() > shown {
        text.push_str(&format!("\n… {} more", issues.len() - shown));
    }
    Err(Fail::Message(text))
}

enum Check {
    Ok,
    Missing,
    Mismatch,
    Bad,
    Cancelled,
    Error(String),
}

fn check_one(kind: Kind, root: &Path, name: &str, expect: &str, cancel: &AtomicBool) -> Check {
    let Some(rel) = crate::sfv::safe_rel(name) else { return Check::Bad };
    let path = root.join(rel);
    if !crate::sfv::inside(root, &path) {
        return Check::Bad;
    }
    match hash_file(kind, &path, cancel) {
        Ok(hash) if hash.eq_ignore_ascii_case(expect) => Check::Ok,
        Ok(_) => Check::Mismatch,
        Err(Fail::Cancelled) => Check::Cancelled,
        Err(Fail::Message(err)) if err.contains("os error 2") || err.contains("No such file") => Check::Missing,
        Err(err) => Check::Error(err.to_string()),
    }
}

fn hash_file(kind: Kind, path: &Path, cancel: &AtomicBool) -> Result<String, Fail> {
    let mut file = File::open(path)?;
    match kind {
        Kind::Md5 => hash_read::<Md5>(&mut file, cancel),
        Kind::Sha1 => hash_read::<Sha1>(&mut file, cancel),
        Kind::Sha256 => hash_read::<Sha256>(&mut file, cancel),
    }
}

fn hash_read<D: Digest>(file: &mut File, cancel: &AtomicBool) -> Result<String, Fail> {
    let mut hasher = D::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        // Stop between reads so cancel does not wait out a large file.
        if cancel.load(Ordering::Relaxed) {
            return Err(Fail::Cancelled);
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

// GNU coreutils: hex, two spaces, name. A space and a star mark binary mode.
fn parse_line(kind: Kind, line: &str) -> Option<Result<(String, String), String>> {
    let line = line.trim_start().trim_end_matches('\r');
    if line.trim().is_empty() || line.starts_with('#') || line.starts_with(';') {
        return None;
    }
    let row = (|| {
        let hash = line.get(..kind.width())?;
        let rest = &line[kind.width()..];
        let name = rest.strip_prefix("  ").or_else(|| rest.strip_prefix(" *"))?;
        (!name.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| (name.to_string(), hash.to_ascii_lowercase()))
    })();
    Some(row.ok_or_else(|| line.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rc-sum-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn md5_round_trip() {
        let dir = dir("md5");
        fs::write(dir.join("a.txt"), b"a").unwrap();
        let dest = dir.join("a.md5");
        create(Kind::Md5, &dir, &[dir.join("a.txt")], &dest, &AtomicBool::new(false), &mut |_| {}).unwrap();
        let text = fs::read_to_string(&dest).unwrap();
        assert!(text.starts_with("0cc175b9c0f1b6a831c399e269772661  a.txt"));
        verify(&dest, &AtomicBool::new(false), &mut |_| {}).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha_known_values_and_checksum_files_are_left_out() {
        let dir = dir("sha");
        fs::write(dir.join("a.txt"), b"abc").unwrap();
        fs::write(dir.join("old.sha1"), b"ignored").unwrap();
        let cancel = AtomicBool::new(false);
        let out = dir.join("all.sha256");
        create(Kind::Sha256, &dir, &[dir.join("a.txt"), dir.join("old.sha1")], &out, &cancel, &mut |_| {}).unwrap();
        let text = fs::read_to_string(&out).unwrap();
        assert_eq!(text, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a.txt\n");
        let out = dir.join("all.sha1");
        create(Kind::Sha1, &dir, &[dir.join("a.txt")], &out, &cancel, &mut |_| {}).unwrap();
        assert!(fs::read_to_string(&out).unwrap().starts_with("a9993e364706816aba3e25717850c26c9cd0d89d  a.txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_reports_mismatch_and_missing_in_the_sfv_layout() {
        let dir = dir("bad");
        fs::write(dir.join("a.txt"), b"a").unwrap();
        let list = dir.join("a.md5");
        let wrong = "00000000000000000000000000000000";
        fs::write(&list, format!("{wrong}  a.txt\n{wrong}  gone.txt\n{wrong}  ../escape\n")).unwrap();
        let err = verify(&list, &AtomicBool::new(false), &mut |_| {}).unwrap_err().to_string();
        assert!(err.starts_with("OK 0, failed 2, missing 1\n"), "{err}");
        assert!(err.contains("mismatch  a.txt") && err.contains("missing  gone.txt") && err.contains("bad path  ../escape"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_fails_on_a_line_it_cannot_read() {
        let dir = dir("unread");
        fs::write(dir.join("a  b.txt"), b"a").unwrap();
        let list = dir.join("a.md5");
        fs::write(&list, "0cc175b9c0f1b6a831c399e269772661 *a  b.txt\n\\0cc175b9c0f1b6a831c399e269772661  a\\nb\n").unwrap();
        let err = verify(&list, &AtomicBool::new(false), &mut |_| {}).unwrap_err().to_string();
        assert!(err.starts_with("OK 1, failed 1, missing 0\n") && err.contains("bad line"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
