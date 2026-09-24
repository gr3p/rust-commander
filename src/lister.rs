use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub const WINDOW: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Text,
    Binary,
    Hex,
}

pub struct Reader {
    file: File,
    pub len: u64,
    pub offset: u64,
    pub bytes_read: u64,
}

impl Reader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            len,
            offset: 0,
            bytes_read: 0,
        })
    }

    pub fn read_window(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; WINDOW];
        let start = self.offset.min(self.len);
        self.file.seek_to(start)?;
        let n = self.file.read(&mut buf)?;
        self.bytes_read += n as u64;
        buf.truncate(n);
        Ok(buf)
    }

    pub fn scroll(&mut self, delta: i64) {
        let next = self.offset as i64 + delta;
        let next = next.max(0) as u64;
        self.offset = next.min(self.len);
    }
}

trait SeekTo {
    fn seek_to(&mut self, pos: u64) -> io::Result<()>;
}

impl SeekTo for File {
    fn seek_to(&mut self, pos: u64) -> io::Result<()> {
        use std::io::{Seek, SeekFrom};
        self.seek(SeekFrom::Start(pos)).map(|_| ())
    }
}

pub fn format_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn format_binary(bytes: &[u8], width: usize) -> String {
    let width = width.max(1);
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(width).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for b in chunk {
            out.push(if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            });
        }
    }
    out
}

pub fn format_hex(bytes: &[u8], base: u64) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{:08x}  ", base + (i as u64) * 16));
        for (j, b) in chunk.iter().enumerate() {
            if j == 8 {
                out.push(' ');
            }
            out.push_str(&format!("{b:02x} "));
        }
        out.push(' ');
        for b in chunk {
            out.push(if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            });
        }
    }
    out
}

pub fn render(mode: ViewMode, bytes: &[u8], offset: u64) -> String {
    match mode {
        ViewMode::Text => format_text(bytes),
        ViewMode::Binary => format_binary(bytes, 64),
        ViewMode::Hex => format_hex(bytes, offset),
    }
}

#[derive(Clone, Debug, Default)]
pub struct PreviewSlot {
    pub generation: u64,
    pub text: String,
}

pub fn apply_preview(slot: &mut PreviewSlot, generation: u64, text: String) -> bool {
    if slot.generation != generation {
        return false;
    }
    slot.text = text;
    true
}
