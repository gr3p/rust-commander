use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The last page of an Ogg file is at most this big.
const TAIL: u64 = 70_000;

/// The granule position of the last page that finishes a packet. For Opus that is the
/// sample count at 48 kHz, pre-skip included, which is how long the stream really is.
pub fn last_granule(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let take = len.min(TAIL);
    file.seek(SeekFrom::Start(len - take)).ok()?;
    let mut tail = vec![0u8; take as usize];
    file.read_exact(&mut tail).ok()?;
    // A page that runs to the very end of the file is surely real. Trailing tags make a second pass necessary.
    find_page(&tail, true).or_else(|| find_page(&tail, false))
}

fn find_page(tail: &[u8], must_end_file: bool) -> Option<u64> {
    let mut at = tail.len().checked_sub(27)?;
    loop {
        if tail[at..].starts_with(b"OggS") && tail[at + 4] == 0 && tail[at + 5] <= 7 {
            let granule = i64::from_le_bytes(tail[at + 6..at + 14].try_into().ok()?);
            // -1 means no packet ends on this page.
            if granule >= 0 && (!must_end_file || page_end(tail, at) == Some(tail.len())) {
                return Some(granule as u64);
            }
        }
        if at == 0 {
            return None;
        }
        at -= 1;
    }
}

fn page_end(tail: &[u8], at: usize) -> Option<usize> {
    let segments = *tail.get(at + 26)? as usize;
    let table = tail.get(at + 27..at + 27 + segments)?;
    Some(at + 27 + segments + table.iter().map(|&n| n as usize).sum::<usize>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio").join(name)
    }

    #[test]
    fn reads_the_length_of_ogg_files() {
        // Two seconds at 48 kHz plus the 312 samples of pre-skip.
        assert_eq!(last_granule(&fixture("chirp.opus")), Some(96_312));
        assert_eq!(last_granule(&fixture("surround.opus")), Some(24_312));
        assert_eq!(last_granule(&fixture("chirp.ogg")), Some(88_200));
    }

    #[test]
    fn other_files_have_no_granule() {
        assert_eq!(last_granule(&fixture("chirp.flac")), None);
        assert_eq!(last_granule(&fixture("chirp.mp3")), None);
        assert_eq!(last_granule(&fixture("missing.opus")), None);
    }

    #[test]
    fn trailing_bytes_after_the_last_page_are_tolerated() {
        let dir = std::env::temp_dir().join(format!("rust-commander-ogg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tagged.opus");
        let mut bytes = std::fs::read(fixture("chirp.opus")).unwrap();
        bytes.extend_from_slice(b"APETAGEX trailing junk that is not a page");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(last_granule(&path), Some(96_312));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tiny_and_cut_files_do_not_panic() {
        let dir = std::env::temp_dir().join(format!("rust-commander-ogg-cut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = std::fs::read(fixture("chirp.opus")).unwrap();
        for len in [0, 1, 4, 26, 27, 28, 100, bytes.len() / 2] {
            let path = dir.join(format!("{len}.opus"));
            std::fs::write(&path, &bytes[..len]).unwrap();
            let _ = last_granule(&path);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
