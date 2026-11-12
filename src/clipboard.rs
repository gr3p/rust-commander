use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

pub struct DesktopFiles {
    pub cut: bool,
    pub paths: Vec<PathBuf>,
}

// GNOME, Xfce and Cinnamon write "copy" or "cut", then one file URI per line.
// text/uri-list is the same URIs without that first word, so a cut looks like a copy.
pub fn desktop_files(mime: &str, bytes: &[u8]) -> DesktopFiles {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let mut cut = false;
    let mut paths = Vec::new();
    if copied_files(mime) {
        match lines.next() {
            Some("cut") => cut = true,
            Some("copy") => {}
            Some(line) => {
                if let Some(path) = local_file(line) {
                    paths.push(path);
                }
            }
            None => {}
        }
    }
    paths.extend(lines.filter_map(local_file));
    let mut seen = std::collections::BTreeSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    DesktopFiles { cut, paths }
}

fn copied_files(mime: &str) -> bool {
    matches!(mime, "x-special/gnome-copied-files" | "x-special/mate-copied-files")
}

// file:// and file://localhost only. %HH is one raw path byte.
fn local_file(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let (host, path) = rest.split_once('/')?;
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let bytes = percent_decode(path)?;
    if bytes.is_empty() {
        return None;
    }
    let mut full = Vec::with_capacity(bytes.len() + 1);
    full.push(b'/');
    full.extend(bytes);
    Some(PathBuf::from(OsStr::from_bytes(&full)))
}

fn percent_decode(input: &str) -> Option<Vec<u8>> {
    let raw = input.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            if i + 2 >= raw.len() {
                return None;
            }
            let hi = hex(raw[i + 1])?;
            let lo = hex(raw[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    Some(out)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_copy_decodes_a_space() {
        let files = desktop_files("x-special/gnome-copied-files", b"copy\nfile:///tmp/a%20b\nfile:///tmp/c\n");
        assert!(!files.cut);
        assert_eq!(files.paths, vec![PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c")]);
    }

    #[test]
    fn gnome_cut_is_a_move() {
        let files = desktop_files("x-special/gnome-copied-files", b"cut\r\nfile://localhost/tmp/a");
        assert!(files.cut);
        assert_eq!(files.paths, vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn uri_list_skips_comments_and_remote_files() {
        let raw = b"# comment\r\nfile:///tmp/a\r\nfile://other/tmp/b\r\nhttps://example.com/a\r\n";
        let files = desktop_files("text/uri-list", raw);
        assert!(!files.cut);
        assert_eq!(files.paths, vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn a_broken_escape_is_skipped() {
        let files = desktop_files("text/uri-list", b"file:///tmp/%ZZ\nfile:///tmp/ok");
        assert_eq!(files.paths, vec![PathBuf::from("/tmp/ok")]);
    }

    #[test]
    fn repeated_paths_are_kept_once() {
        let files = desktop_files("x-special/gnome-copied-files", b"copy\nfile:///tmp/a\nfile:///tmp/a");
        assert_eq!(files.paths, vec![PathBuf::from("/tmp/a")]);
    }
}
