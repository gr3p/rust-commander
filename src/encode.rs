use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::archive::Fail;
use crate::ops::{Progress, Report};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Base64,
    Uue,
    Mime,
}

impl Codec {
    pub fn extension(self) -> &'static str {
        match self {
            Codec::Base64 => "b64",
            Codec::Uue => "uue",
            Codec::Mime => "mime",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Codec::Base64 => "Base64",
            Codec::Uue => "UUE",
            Codec::Mime => "MIME",
        }
    }
}

pub fn codec_of(path: &Path) -> Option<Codec> {
    match path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()).as_deref() {
        Some("b64") => Some(Codec::Base64),
        Some("uue") => Some(Codec::Uue),
        Some("mime") => Some(Codec::Mime),
        _ => None,
    }
}

pub fn encode(
    codec: Codec,
    source: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Report, Fail> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(Fail::Message("Encode needs a regular file".into()));
    }
    let name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    // The name ends up in a header line.
    let name: String = name.chars().map(|c| if c.is_control() || c == '"' { '_' } else { c }).collect();
    let total = meta.len();
    let written = write_stream(dest, |out| {
        let mut file = File::open(source)?;
        match codec {
            Codec::Base64 => write_base64(&mut file, out, false, total, cancel, progress, source),
            Codec::Mime => {
                let header = format!(
                    "Content-Type: application/octet-stream; name=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"{name}\"\r\n\r\n"
                );
                out.write_all(header.as_bytes())?;
                write_base64(&mut file, out, true, total, cancel, progress, source)
            }
            Codec::Uue => {
                writeln!(out, "begin 644 {name}")?;
                write_uue(&mut file, out, total, cancel, progress, source)?;
                writeln!(out, "`")?;
                writeln!(out, "end")?;
                Ok(())
            }
        }
    });
    match written {
        Err(Fail::Cancelled) => Ok(Report { cancelled: true, files: 0 }),
        other => other.map(|()| Report { cancelled: false, files: 1 }),
    }
}

pub fn decode(
    source: &Path,
    dest_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<(Report, PathBuf), Fail> {
    let codec = codec_of(source).ok_or_else(|| Fail::Message("Not a Base64, UUE or MIME file".into()))?;
    let total = fs::metadata(source)?.len();
    let mut lines = Lines(BufReader::new(File::open(source)?));
    // The header comes first, so the name is known before the target is created.
    let name = match codec {
        Codec::Base64 => source.file_stem().map(|name| name.to_string_lossy().into_owned()),
        Codec::Uue => Some(uue_name(&mut lines)?),
        Codec::Mime => Some(mime_name(&mut lines)?),
    };
    let name = name.as_deref().and_then(|name| Path::new(name).file_name()).map(|name| name.to_string_lossy().into_owned());
    let dest = dest_dir.join(name.unwrap_or_else(|| "decoded".into()));
    let written = write_stream(&dest, |out| {
        let (mut b64, mut buf, mut done, mut ended) = (B64::default(), Vec::new(), 0u64, false);
        for (index, line) in lines.enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(Fail::Cancelled);
            }
            let line = line?;
            done += line.len() as u64 + 1;
            buf.clear();
            if codec == Codec::Uue {
                if line.trim_end() == "end" {
                    ended = true;
                    break;
                }
                uue_line(&line, &mut buf);
            } else {
                b64.feed(&line, &mut buf)?;
            }
            out.write_all(&buf)?;
            if index % 1024 == 0 {
                progress(&Progress { done, total, current: source.to_path_buf(), note: String::new() });
            }
        }
        if codec == Codec::Uue && !ended {
            return Err(Fail::Message("UUE data is cut off".into()));
        }
        Ok(())
    });
    match written {
        Err(Fail::Cancelled) => Ok((Report { cancelled: true, files: 0 }, dest)),
        other => other.map(|()| (Report { cancelled: false, files: 1 }, dest)),
    }
}

fn write_base64(
    file: &mut File,
    out: &mut File,
    crlf: bool,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
    source: &Path,
) -> Result<(), Fail> {
    let mut buf = [0u8; 57];
    let (mut done, mut step) = (0u64, 0u64);
    let nl = if crlf { "\r\n" } else { "\n" };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Fail::Cancelled);
        }
        let n = read_full(file, &mut buf)?;
        if n == 0 {
            break;
        }
        let line = base64_encode(&buf[..n]);
        out.write_all(line.as_bytes())?;
        out.write_all(nl.as_bytes())?;
        done += n as u64;
        step += 1;
        if step % 1024 == 0 {
            progress(&Progress { done, total, current: source.to_path_buf(), note: String::new() });
        }
    }
    Ok(())
}

fn write_uue(
    file: &mut File,
    out: &mut File,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
    source: &Path,
) -> Result<(), Fail> {
    let mut buf = [0u8; 45];
    let (mut done, mut step) = (0u64, 0u64);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Fail::Cancelled);
        }
        let n = read_full(file, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut line = String::new();
        line.push((n as u8 + 32) as char);
        line.push_str(&uu_encode(&buf[..n]));
        writeln!(out, "{line}")?;
        done += n as u64;
        step += 1;
        if step % 1024 == 0 {
            progress(&Progress { done, total, current: source.to_path_buf(), note: String::new() });
        }
    }
    Ok(())
}

fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn base64_encode(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | data[i + 2] as u32;
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(A[((n >> 6) & 63) as usize] as char);
        out.push(A[(n & 63) as usize] as char);
        i += 3;
    }
    if i < data.len() {
        let left = data.len() - i;
        let b0 = data[i] as u32;
        let b1 = if left > 1 { data[i + 1] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8);
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        if left == 1 {
            out.push('=');
        } else {
            out.push(A[((n >> 6) & 63) as usize] as char);
        }
        out.push('=');
    }
    out
}

#[derive(Default)]
struct B64 {
    acc: u32,
    bits: u32,
}

impl B64 {
    fn feed(&mut self, line: &str, out: &mut Vec<u8>) -> Result<(), Fail> {
        for byte in line.bytes() {
            let val = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' | b' ' | b'\t' | b'\r' => continue,
                _ => return Err(Fail::Message("Bad Base64".into())),
            };
            self.acc = (self.acc << 6) | val as u32;
            self.bits += 6;
            if self.bits >= 8 {
                self.bits -= 8;
                out.push((self.acc >> self.bits) as u8);
                self.acc &= (1 << self.bits) - 1;
            }
        }
        Ok(())
    }
}

fn uu_encode(data: &[u8]) -> String {
    let mut padded = data.to_vec();
    while padded.len() % 3 != 0 {
        padded.push(0);
    }
    let mut out = String::new();
    for chunk in padded.chunks(3) {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
        for shift in [18, 12, 6, 0] {
            let mut c = ((n >> shift) & 63) as u8 + 32;
            if c == 32 {
                c = b'`';
            }
            out.push(c as char);
        }
    }
    out
}

// Like `BufRead::lines`, but a long line comes in pieces, so `base64 -w0` output never sits whole in memory.
struct Lines(BufReader<File>);

impl Iterator for Lines {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut buf = Vec::new();
        match (&mut self.0).take(64 * 1024).read_until(b'\n', &mut buf) {
            Ok(0) => None,
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                }
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                Some(String::from_utf8(buf).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)))
            }
            Err(err) => Some(Err(err)),
        }
    }
}

fn uue_name(lines: &mut Lines) -> Result<String, Fail> {
    for line in lines {
        let line = line?;
        if let Some(rest) = line.strip_prefix("begin ") {
            let name = rest.split_once(' ').map(|(_, name)| name.trim().to_string()).filter(|name| !name.is_empty());
            return name.ok_or_else(|| Fail::Message("Missing file name after begin".into()));
        }
    }
    Err(Fail::Message("Missing begin line".into()))
}

fn uue_line(line: &str, out: &mut Vec<u8>) {
    let line = line.trim_end_matches(['\r', '\n']);
    // The length is the first character; '`' stands for 0.
    let Some(first) = line.bytes().next() else { return };
    let len = (first.wrapping_sub(32) & 63) as usize;
    // Encoders may cut trailing spaces, so missing characters count as 0.
    let sextets: Vec<u8> = line.bytes().skip(1).map(|c| c.wrapping_sub(32) & 63).chain(std::iter::repeat(0)).take(len.div_ceil(3) * 4).collect();
    let start = out.len();
    for quad in sextets.chunks(4) {
        let n = ((quad[0] as u32) << 18) | ((quad[1] as u32) << 12) | ((quad[2] as u32) << 6) | quad[3] as u32;
        out.extend([(n >> 16) as u8, (n >> 8) as u8, n as u8]);
    }
    out.truncate(start + len);
}

fn mime_name(lines: &mut Lines) -> Result<String, Fail> {
    let mut name = None;
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            return Ok(name.unwrap_or_else(|| "decoded".into()));
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains("name=") {
            if let Some((_, value)) = line.split_once("name=") {
                let value = value.split(';').next().unwrap_or("").trim().trim_matches('"').to_string();
                if !value.is_empty() {
                    name = Some(value);
                }
            }
        }
    }
    Err(Fail::Message("Missing MIME header".into()))
}

fn write_stream(dest: &Path, write: impl FnOnce(&mut File) -> Result<(), Fail>) -> Result<(), Fail> {
    if fs::symlink_metadata(dest).is_ok() {
        return Err(Fail::Message(format!("{} already exists", dest.file_name().unwrap_or_default().to_string_lossy())));
    }
    let temp = crate::ops::partial_path(dest);
    let _ = fs::remove_file(&temp);
    let mut file = File::create(&temp)?;
    let result = (|| {
        write(&mut file)?;
        file.sync_all()?;
        fs::rename(&temp, dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rc-enc-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_codec_round_trips_binary_data_of_any_length() {
        let dir = dir("round");
        let cancel = AtomicBool::new(false);
        for len in [0usize, 1, 2, 3, 44, 45, 46, 57, 58, 100_000] {
            let data: Vec<u8> = (0..len).map(|i| (i * 31 % 256) as u8).collect();
            let src = dir.join("my file.bin");
            fs::write(&src, &data).unwrap();
            for codec in [Codec::Base64, Codec::Uue, Codec::Mime] {
                let dest = dir.join(format!("my file.bin.{}", codec.extension()));
                encode(codec, &src, &dest, &cancel, &mut |_| {}).unwrap();
                let out = dir.join("back");
                fs::create_dir_all(&out).unwrap();
                let (_, path) = decode(&dest, &out, &cancel, &mut |_| {}).unwrap();
                assert_eq!(fs::read(&path).unwrap(), data, "{codec:?} {len}");
                let _ = fs::remove_dir_all(&out);
                fs::remove_file(&dest).unwrap();
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn known_encodings_and_no_overwrite() {
        let dir = dir("known");
        let cancel = AtomicBool::new(false);
        let src = dir.join("a.txt");
        fs::write(&src, b"Man").unwrap();
        let dest = dir.join("a.txt.b64");
        encode(Codec::Base64, &src, &dest, &cancel, &mut |_| {}).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "TWFu\n");
        let err = encode(Codec::Base64, &src, &dest, &cancel, &mut |_| {}).unwrap_err().to_string();
        assert!(err.contains("already exists"), "{err}");
        let uue = dir.join("a.txt.uue");
        encode(Codec::Uue, &src, &uue, &cancel, &mut |_| {}).unwrap();
        assert_eq!(fs::read_to_string(&uue).unwrap(), "begin 644 a.txt\n#36%N\n`\nend\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cut_off_uue_leaves_no_file() {
        let dir = dir("cut");
        let uue = dir.join("x.uue");
        fs::write(&uue, "begin 644 x.bin\n#36%N\n").unwrap();
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        assert!(decode(&uue, &out, &AtomicBool::new(false), &mut |_| {}).is_err());
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
