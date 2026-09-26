use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const WINDOW: usize = 64 * 1024;
pub const EDIT_MAX: u64 = 1024 * 1024;

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
    Json,
}

const JSON_MAX: u64 = 1024 * 1024;

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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_name(&self) -> &str {
        self.path.file_name().and_then(|name| name.to_str()).unwrap_or("")
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

    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let file = File::open(&self.path)?;
        let mut buf = Vec::new();
        file.take(JSON_MAX + 1).read_to_end(&mut buf)?;
        self.bytes_read += buf.len() as u64;
        Ok(buf)
    }
}

pub fn render_reader(reader: &mut Reader, mode: ViewMode, charset: Charset) -> io::Result<String> {
    if mode == ViewMode::Text && charset == Charset::Json {
        if reader.len > JSON_MAX {
            return Ok(json_too_large());
        }
        let bytes = reader.read_all()?;
        if bytes.len() as u64 > JSON_MAX {
            return Ok(json_too_large());
        }
        return Ok(render(ViewMode::Text, Charset::Json, &bytes, 0));
    }
    let bytes = reader.read_window()?;
    let chosen = choose_charset(reader.file_name(), &bytes, charset);
    Ok(render(mode, chosen, &bytes, reader.offset))
}

fn json_too_large() -> String {
    format!("JSON file is larger than {JSON_MAX} bytes")
}

pub fn read_for_edit(path: &Path, charset: Charset) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|err| err.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > EDIT_MAX {
        return Err("File too large to edit".into());
    }
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if choose_charset(name, &bytes, charset) != Charset::Utf8 || std::str::from_utf8(&bytes).is_err() {
        return Err("Edit is UTF-8".into());
    }
    Ok(format_text(&bytes))
}

pub fn write_text(path: &Path, text: &str) -> io::Result<()> {
    fs::write(path, text.as_bytes())
}

pub fn format_text(bytes: &[u8]) -> String {
    // GTK TextBuffer rejects embedded NUL even though it is valid UTF-8.
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(text.as_ref());
    normalize_newlines(text).replace('\0', ".")
}

// Scene .nfo is CP437 and is not valid UTF-8. A Kodi movie.nfo is UTF-8 and stays UTF-8.
pub fn choose_charset(name: &str, bytes: &[u8], charset: Charset) -> Charset {
    if charset != Charset::Utf8 {
        return charset;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return Charset::Utf8;
    }
    if crate::listing::extension(name).eq_ignore_ascii_case("nfo") {
        return Charset::Ascii;
    }
    Charset::Utf8
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
            Charset::Json => format_json(bytes),
        },
        ViewMode::Binary => format_binary(bytes, 64, charset),
        ViewMode::Hex => format_hex(bytes, offset, charset),
    }
}

fn format_json(bytes: &[u8]) -> String {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return "Not valid JSON".into();
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match pretty_json(text) {
        Ok(pretty) => pretty,
        Err(()) => format!("Not valid JSON\n\n{}", format_text(bytes)),
    }
}

fn pretty_json(src: &str) -> Result<String, ()> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = String::new();
    write_value(src, b, &mut i, &mut out, 0)?;
    i = skip_ws(b, i);
    if i != b.len() {
        return Err(());
    }
    Ok(out)
}

fn write_value(src: &str, b: &[u8], i: &mut usize, out: &mut String, depth: usize) -> Result<(), ()> {
    // Cap nesting so a hostile file cannot blow the stack.
    if depth > 64 {
        return Err(());
    }
    *i = skip_ws(b, *i);
    if *i >= b.len() {
        return Err(());
    }
    match b[*i] {
        b'{' => write_collection(src, b, i, out, depth, b'}', "{}"),
        b'[' => write_collection(src, b, i, out, depth, b']', "[]"),
        b'"' => write_span(src, i, out, skip_string(b, *i)?),
        _ => write_span(src, i, out, skip_literal(b, *i)?),
    }
}

fn write_collection(src: &str, b: &[u8], i: &mut usize, out: &mut String, depth: usize, end: u8, empty: &str) -> Result<(), ()> {
    *i += 1;
    *i = skip_ws(b, *i);
    if b.get(*i) == Some(&end) {
        out.push_str(empty);
        *i += 1;
        return Ok(());
    }
    out.push(if end == b'}' { '{' } else { '[' });
    out.push('\n');
    loop {
        push_indent(out, depth + 1);
        if end == b'}' {
            *i = skip_ws(b, *i);
            write_span(src, i, out, skip_string(b, *i)?)?;
            *i = skip_ws(b, *i);
            if b.get(*i) != Some(&b':') {
                return Err(());
            }
            *i += 1;
            out.push_str(": ");
        }
        write_value(src, b, i, out, depth + 1)?;
        *i = skip_ws(b, *i);
        match b.get(*i) {
            Some(&b',') => {
                out.push_str(",\n");
                *i += 1;
            }
            Some(found) if *found == end => {
                out.push('\n');
                push_indent(out, depth);
                out.push(*found as char);
                *i += 1;
                return Ok(());
            }
            _ => return Err(()),
        }
    }
}

fn write_span(src: &str, i: &mut usize, out: &mut String, end: usize) -> Result<(), ()> {
    let Some(text) = src.get(*i..end) else {
        return Err(());
    };
    out.push_str(text);
    *i = end;
    Ok(())
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

// JSON whitespace is only space, tab, CR and LF.
fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

fn skip_string(b: &[u8], mut i: usize) -> Result<usize, ()> {
    if b.get(i) != Some(&b'"') {
        return Err(());
    }
    i += 1;
    while i < b.len() {
        match b[i] {
            b'"' => return Ok(i + 1),
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    return Err(());
                }
                if b[i] == b'u' {
                    let hex = b.get(i + 1..i + 5).ok_or(())?;
                    if !hex.iter().all(|c| c.is_ascii_hexdigit()) {
                        return Err(());
                    }
                    i += 5;
                } else if matches!(b[i], b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') {
                    i += 1;
                } else {
                    return Err(());
                }
            }
            c if c < 0x20 => return Err(()),
            _ => i += 1,
        }
    }
    Err(())
}

fn skip_literal(b: &[u8], i: usize) -> Result<usize, ()> {
    if b[i..].starts_with(b"true") {
        return Ok(i + 4);
    }
    if b[i..].starts_with(b"false") {
        return Ok(i + 5);
    }
    if b[i..].starts_with(b"null") {
        return Ok(i + 4);
    }
    skip_number(b, i)
}

fn skip_number(b: &[u8], mut i: usize) -> Result<usize, ()> {
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    if i >= b.len() {
        return Err(());
    }
    match b[i] {
        b'0' => i += 1,
        b'1'..=b'9' => {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
        _ => return Err(()),
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return Err(());
        }
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        i += 1;
        if i < b.len() && matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return Err(());
        }
    }
    Ok(i)
}

fn glyph(charset: Charset, b: u8) -> char {
    if matches!(b, 0x00..=0x1F | 0x7F) {
        return '.';
    }
    match charset {
        Charset::Utf8 | Charset::Json => {
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
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // DOS .nfo uses CR LF. A lone CR is a line break too.
            b'\r' => {
                out.push('\n');
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' | b'\t' => out.push(bytes[i] as char),
            0x00..=0x1F | 0x7F => out.push('.'),
            b => out.push(page_char(charset, b)),
        }
        i += 1;
    }
    out
}

fn normalize_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            out.push('\n');
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn page_char(charset: Charset, b: u8) -> char {
    if b < 0x80 {
        return b as char;
    }
    // 0x80..=0xFF. Undefined Windows-1252 bytes are '.'. ASCII is CP437, the .nfo page.
    let table = match charset {
        Charset::Ansi => CP1252,
        _ => CP437,
    };
    table.chars().nth((b - 0x80) as usize).unwrap_or('.')
}

const CP1252: &str = "\u{20ac}\u{002e}\u{201a}\u{0192}\u{201e}\u{2026}\u{2020}\u{2021}\u{02c6}\u{2030}\u{0160}\u{2039}\u{0152}\u{002e}\u{017d}\u{002e}\u{002e}\u{2018}\u{2019}\u{201c}\u{201d}\u{2022}\u{2013}\u{2014}\u{02dc}\u{2122}\u{0161}\u{203a}\u{0153}\u{002e}\u{017e}\u{0178}\u{00a0}\u{00a1}\u{00a2}\u{00a3}\u{00a4}\u{00a5}\u{00a6}\u{00a7}\u{00a8}\u{00a9}\u{00aa}\u{00ab}\u{00ac}\u{00ad}\u{00ae}\u{00af}\u{00b0}\u{00b1}\u{00b2}\u{00b3}\u{00b4}\u{00b5}\u{00b6}\u{00b7}\u{00b8}\u{00b9}\u{00ba}\u{00bb}\u{00bc}\u{00bd}\u{00be}\u{00bf}\u{00c0}\u{00c1}\u{00c2}\u{00c3}\u{00c4}\u{00c5}\u{00c6}\u{00c7}\u{00c8}\u{00c9}\u{00ca}\u{00cb}\u{00cc}\u{00cd}\u{00ce}\u{00cf}\u{00d0}\u{00d1}\u{00d2}\u{00d3}\u{00d4}\u{00d5}\u{00d6}\u{00d7}\u{00d8}\u{00d9}\u{00da}\u{00db}\u{00dc}\u{00dd}\u{00de}\u{00df}\u{00e0}\u{00e1}\u{00e2}\u{00e3}\u{00e4}\u{00e5}\u{00e6}\u{00e7}\u{00e8}\u{00e9}\u{00ea}\u{00eb}\u{00ec}\u{00ed}\u{00ee}\u{00ef}\u{00f0}\u{00f1}\u{00f2}\u{00f3}\u{00f4}\u{00f5}\u{00f6}\u{00f7}\u{00f8}\u{00f9}\u{00fa}\u{00fb}\u{00fc}\u{00fd}\u{00fe}\u{00ff}";
const CP437: &str = "\u{00c7}\u{00fc}\u{00e9}\u{00e2}\u{00e4}\u{00e0}\u{00e5}\u{00e7}\u{00ea}\u{00eb}\u{00e8}\u{00ef}\u{00ee}\u{00ec}\u{00c4}\u{00c5}\u{00c9}\u{00e6}\u{00c6}\u{00f4}\u{00f6}\u{00f2}\u{00fb}\u{00f9}\u{00ff}\u{00d6}\u{00dc}\u{00a2}\u{00a3}\u{00a5}\u{20a7}\u{0192}\u{00e1}\u{00ed}\u{00f3}\u{00fa}\u{00f1}\u{00d1}\u{00aa}\u{00ba}\u{00bf}\u{2310}\u{00ac}\u{00bd}\u{00bc}\u{00a1}\u{00ab}\u{00bb}\u{2591}\u{2592}\u{2593}\u{2502}\u{2524}\u{2561}\u{2562}\u{2556}\u{2555}\u{2563}\u{2551}\u{2557}\u{255d}\u{255c}\u{255b}\u{2510}\u{2514}\u{2534}\u{252c}\u{251c}\u{2500}\u{253c}\u{255e}\u{255f}\u{255a}\u{2554}\u{2569}\u{2566}\u{2560}\u{2550}\u{256c}\u{2567}\u{2568}\u{2564}\u{2565}\u{2559}\u{2558}\u{2552}\u{2553}\u{256b}\u{256a}\u{2518}\u{250c}\u{2588}\u{2584}\u{258c}\u{2590}\u{2580}\u{03b1}\u{00df}\u{0393}\u{03c0}\u{03a3}\u{03c3}\u{00b5}\u{03c4}\u{03a6}\u{0398}\u{03a9}\u{03b4}\u{221e}\u{03c6}\u{03b5}\u{2229}\u{2261}\u{00b1}\u{2265}\u{2264}\u{2320}\u{2321}\u{00f7}\u{2248}\u{00b0}\u{2219}\u{00b7}\u{221a}\u{207f}\u{00b2}\u{25a0}\u{00a0}";

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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("comarchy-edit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn edit_roundtrip_utf8() {
        let dir = scratch("ok");
        let path = dir.join("note.txt");
        write_text(&path, "hello\n").unwrap();
        assert_eq!(read_for_edit(&path, Charset::Utf8).unwrap(), "hello\n");
        write_text(&path, "changed\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"changed\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_refuses_charset_and_size() {
        let dir = scratch("no");
        let path = dir.join("note.txt");
        write_text(&path, "hello\n").unwrap();
        assert_eq!(read_for_edit(&path, Charset::Ansi).unwrap_err(), "Edit is UTF-8");
        fs::write(&path, [0xff]).unwrap();
        assert_eq!(read_for_edit(&path, Charset::Utf8).unwrap_err(), "Edit is UTF-8");
        let big = dir.join("big.txt");
        fs::write(&big, vec![b'a'; EDIT_MAX as usize + 1]).unwrap();
        assert_eq!(read_for_edit(&big, Charset::Utf8).unwrap_err(), "File too large to edit");
        let _ = fs::remove_dir_all(&dir);
    }
}
