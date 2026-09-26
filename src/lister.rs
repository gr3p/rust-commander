use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const WINDOW: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Text,
    Binary,
    Hex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Ansi,
    Ascii,
}

pub struct Reader {
    path: PathBuf,
    pub len: u64,
    pub offset: u64,
    pub bytes_read: u64,
}

impl Reader {
    pub fn open(path: &Path) -> io::Result<Self> {
        // stat only. read_window opens and closes, so the SMB handle is not kept.
        let meta = fs::metadata(path)?;
        if !meta.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
        }
        Ok(Self {
            path: path.to_path_buf(),
            len: meta.len(),
            offset: 0,
            bytes_read: 0,
        })
    }

    pub fn read_window(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; WINDOW];
        let start = self.offset.min(self.len);
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(start))?;
        let n = file.read(&mut buf)?;
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

pub fn format_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn format_binary(bytes: &[u8], width: usize, charset: Charset) -> String {
    let width = width.max(1);
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(width).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for b in chunk {
            out.push(glyph(charset, *b));
        }
    }
    out
}

pub fn format_hex(bytes: &[u8], base: u64, charset: Charset) -> String {
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
            out.push(glyph(charset, *b));
        }
    }
    out
}

pub fn render(mode: ViewMode, charset: Charset, bytes: &[u8], offset: u64) -> String {
    match mode {
        ViewMode::Text => match charset {
            Charset::Utf8 => format_text(bytes),
            Charset::Ansi | Charset::Ascii => decode_page(bytes, charset),
        },
        ViewMode::Binary => format_binary(bytes, 64, charset),
        ViewMode::Hex => format_hex(bytes, offset, charset),
    }
}

fn glyph(charset: Charset, b: u8) -> char {
    if matches!(b, 0x00..=0x1F | 0x7F) {
        return '.';
    }
    match charset {
        Charset::Utf8 => {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        }
        Charset::Ansi | Charset::Ascii => page_char(charset, b),
    }
}

fn decode_page(bytes: &[u8], charset: Charset) -> String {
    bytes
        .iter()
        .map(|b| match *b {
            b'\n' | b'\r' | b'\t' => *b as char,
            0x00..=0x1F | 0x7F => '.',
            b => page_char(charset, b),
        })
        .collect()
}

fn page_char(charset: Charset, b: u8) -> char {
    if b < 0x80 {
        return b as char;
    }
    // 0x80..=0xFF. Undefined Windows-1252 bytes are '.'.
    let table = match charset {
        Charset::Ansi => CP1252,
        _ => CP850,
    };
    table.chars().nth((b - 0x80) as usize).unwrap_or('.')
}

const CP1252: &str = "\u{20ac}\u{002e}\u{201a}\u{0192}\u{201e}\u{2026}\u{2020}\u{2021}\u{02c6}\u{2030}\u{0160}\u{2039}\u{0152}\u{002e}\u{017d}\u{002e}\u{002e}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{02dc}\u{2122}\u{0161}\u{203a}\u{0153}\u{002e}\u{017e}\u{0178}\u{00a0}\u{00a1}\u{00a2}\u{00a3}\u{00a4}\u{00a5}\u{00a6}\u{00a7}\u{00a8}\u{00a9}\u{00aa}\u{00ab}\u{00ac}\u{00ad}\u{00ae}\u{00af}\u{00b0}\u{00b1}\u{00b2}\u{00b3}\u{00b4}\u{00b5}\u{00b6}\u{00b7}\u{00b8}\u{00b9}\u{00ba}\u{00bb}\u{00bc}\u{00bd}\u{00be}\u{00bf}\u{00c0}\u{00c1}\u{00c2}\u{00c3}\u{00c4}\u{00c5}\u{00c6}\u{00c7}\u{00c8}\u{00c9}\u{00ca}\u{00cb}\u{00cc}\u{00cd}\u{00ce}\u{00cf}\u{00d0}\u{00d1}\u{00d2}\u{00d3}\u{00d4}\u{00d5}\u{00d6}\u{00d7}\u{00d8}\u{00d9}\u{00da}\u{00db}\u{00dc}\u{00dd}\u{00de}\u{00df}\u{00e0}\u{00e1}\u{00e2}\u{00e3}\u{00e4}\u{00e5}\u{00e6}\u{00e7}\u{00e8}\u{00e9}\u{00ea}\u{00eb}\u{00ec}\u{00ed}\u{00ee}\u{00ef}\u{00f0}\u{00f1}\u{00f2}\u{00f3}\u{00f4}\u{00f5}\u{00f6}\u{00f7}\u{00f8}\u{00f9}\u{00fa}\u{00fb}\u{00fc}\u{00fd}\u{00fe}\u{00ff}";
const CP850: &str = "\u{00c7}\u{00fc}\u{00e9}\u{00e2}\u{00e4}\u{00e0}\u{00e5}\u{00e7}\u{00ea}\u{00eb}\u{00e8}\u{00ef}\u{00ee}\u{00ec}\u{00c4}\u{00c5}\u{00c9}\u{00e6}\u{00c6}\u{00f4}\u{00f6}\u{00f2}\u{00fb}\u{00f9}\u{00ff}\u{00d6}\u{00dc}\u{00f8}\u{00a3}\u{00d8}\u{00d7}\u{0192}\u{00e1}\u{00ed}\u{00f3}\u{00fa}\u{00f1}\u{00d1}\u{00aa}\u{00ba}\u{00bf}\u{00ae}\u{00ac}\u{00bd}\u{00bc}\u{00a1}\u{00ab}\u{00bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{00c1}\u{00c2}\u{00c0}\u{00a9}\u{2563}\u{2551}\u{2557}\u{255d}\u{00a2}\u{00a5}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{00e3}\u{00c3}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{00a4}\u{00f0}\u{00d0}\u{00ca}\u{00cb}\u{00c8}\u{0131}\u{00cd}\u{00ce}\u{00cf}\u{2518}\u{250c}\u{2588}\u{2584}\u{00a6}\u{00cc}\u{2580}\u{00d3}\u{00df}\u{00d4}\u{00d2}\u{00f5}\u{00d5}\u{00b5}\u{00fe}\u{00de}\u{00da}\u{00db}\u{00d9}\u{00fd}\u{00dd}\u{00af}\u{00b4}\u{00ad}\u{00b1}\u{2017}\u{00be}\u{00b6}\u{00a7}\u{00f7}\u{00b8}\u{00b0}\u{00a8}\u{00b7}\u{00b9}\u{00b3}\u{00b2}\u{25a0}\u{00a0}";

#[derive(Clone, Debug)]
pub enum PreviewBody {
    Text(String),
    Image { width: u32, height: u32, pixels: Vec<u8> },
}

impl Default for PreviewBody {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

#[derive(Clone, Debug, Default)]
pub struct PreviewSlot {
    pub generation: u64,
    pub rev: u64,
    pub body: PreviewBody,
}

impl PreviewSlot {
    pub fn set_body(&mut self, body: PreviewBody) {
        self.body = body;
        self.rev = self.rev.wrapping_add(1);
    }

    pub fn text(&self) -> &str {
        match &self.body {
            PreviewBody::Text(text) => text,
            PreviewBody::Image { .. } => "",
        }
    }
}

pub fn apply_preview(slot: &mut PreviewSlot, generation: u64, body: PreviewBody) -> bool {
    if slot.generation != generation {
        return false;
    }
    slot.set_body(body);
    true
}
