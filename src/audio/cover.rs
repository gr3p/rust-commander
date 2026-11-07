use std::fs;
use std::path::{Path, PathBuf};

/// A folder this big is not an album. Stop looking rather than read it all.
const MAX_ENTRIES: usize = 20_000;
const MIN_BYTES: u64 = 512;
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Picture files the player can show. Case does not matter.
fn is_picture(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "bmp" | "gif")
}

// How much a file name says "this is the album cover". Zero is not a candidate.
fn name_score(stem: &str) -> u32 {
    let stem = stem.to_ascii_lowercase();
    // The back of a sleeve, a disc label or a booklet is a picture of the album, not its cover.
    if ["back", "rear", "inlay", "booklet", "disc", "cd1", "cd2", "tray"].iter().any(|bad| stem.contains(bad)) {
        return 0;
    }
    match stem.as_str() {
        "cover" => 100,
        "folder" => 95,
        "front" => 90,
        "albumart" | "album" => 85,
        "albumartlarge" => 80,
        "poster" => 70,
        "thumb" | "albumartsmall" => 30,
        _ if ["cover", "front", "folder", "albumart", "album"].iter().any(|key| stem.contains(key)) => 60,
        _ => 0,
    }
}

/// The picture that most likely is the cover of the music in `dir`: a file called cover,
/// folder, front or album, or else the only picture there is. None when it cannot tell.
pub fn folder_cover(dir: &Path) -> Option<PathBuf> {
    let mut pictures: Vec<(PathBuf, u32, u64, bool)> = Vec::new();
    for entry in fs::read_dir(dir).ok()?.take(MAX_ENTRIES).flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !is_picture(&ext) {
            continue;
        }
        // A folder named cover.jpg, or a link to nowhere, is not a picture.
        let Ok(meta) = fs::metadata(&path) else { continue };
        if !meta.is_file() || meta.len() < MIN_BYTES || meta.len() > MAX_BYTES {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        pictures.push((path.clone(), name_score(stem), meta.len(), ext == "jpg" || ext == "jpeg" || ext == "png"));
    }
    // A named cover wins. Among equals a jpg or png is the likelier cover, and then the bigger file.
    let best = pictures
        .iter()
        .filter(|(_, score, _, _)| *score > 0)
        .max_by(|a, b| (a.1, a.3, a.2, std::cmp::Reverse(&a.0)).cmp(&(b.1, b.3, b.2, std::cmp::Reverse(&b.0))));
    if let Some((path, ..)) = best {
        return Some(path.clone());
    }
    // One picture and nothing else to choose from is almost surely it, unless its name says otherwise.
    let unnamed: Vec<&(PathBuf, u32, u64, bool)> = pictures
        .iter()
        .filter(|(path, ..)| {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
            !["back", "rear", "inlay", "booklet", "disc", "cd1", "cd2", "tray"].iter().any(|bad| stem.contains(bad))
        })
        .collect();
    match unnamed.as_slice() {
        [only] => Some(only.0.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-commander-cover-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, name: &str, bytes: usize) {
        fs::write(dir.join(name), vec![7u8; bytes]).unwrap();
    }

    fn found(dir: &Path) -> Option<String> {
        folder_cover(dir).map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
    }

    #[test]
    fn a_file_named_cover_wins_over_the_rest() {
        let dir = scratch("named");
        put(&dir, "IMG_0001.jpg", 9000);
        put(&dir, "folder.jpg", 2000);
        put(&dir, "cover.jpg", 1000);
        put(&dir, "front.png", 5000);
        assert_eq!(found(&dir).as_deref(), Some("cover.jpg"));
        fs::remove_file(dir.join("cover.jpg")).unwrap();
        assert_eq!(found(&dir).as_deref(), Some("folder.jpg"));
        fs::remove_file(dir.join("folder.jpg")).unwrap();
        assert_eq!(found(&dir).as_deref(), Some("front.png"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn case_and_extension_do_not_matter() {
        let dir = scratch("case");
        put(&dir, "Folder.JPG", 3000);
        assert_eq!(found(&dir).as_deref(), Some("Folder.JPG"));
        let other = scratch("case2");
        put(&other, "AlbumArt.PNG", 3000);
        assert_eq!(found(&other).as_deref(), Some("AlbumArt.PNG"));
        let webp = scratch("case3");
        put(&webp, "cover.webp", 3000);
        assert_eq!(found(&webp).as_deref(), Some("cover.webp"));
        for d in [dir, other, webp] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn names_that_contain_a_keyword_count_a_little() {
        let dir = scratch("keyword");
        put(&dir, "Pink Floyd - The Wall (front).jpg", 4000);
        put(&dir, "party.jpg", 4000);
        assert_eq!(found(&dir).as_deref(), Some("Pink Floyd - The Wall (front).jpg"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_back_of_the_sleeve_is_not_the_cover() {
        let dir = scratch("back");
        put(&dir, "back.jpg", 5000);
        put(&dir, "cover-back.jpg", 5000);
        put(&dir, "cd1.png", 5000);
        assert_eq!(found(&dir), None);
        put(&dir, "front.jpg", 100_000);
        assert_eq!(found(&dir).as_deref(), Some("front.jpg"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_lone_unnamed_picture_is_taken_but_two_are_not() {
        let dir = scratch("lone");
        put(&dir, "IMG_0042.jpg", 4000);
        assert_eq!(found(&dir).as_deref(), Some("IMG_0042.jpg"));
        put(&dir, "IMG_0043.jpg", 4000);
        assert_eq!(found(&dir), None);
        let _ = fs::remove_dir_all(dir);
        // A lone picture that says it is the back of something is not taken either.
        let back = scratch("lone-back");
        put(&back, "back.jpg", 4000);
        assert_eq!(found(&back), None);
        let _ = fs::remove_dir_all(back);
    }

    #[test]
    fn ties_go_to_jpg_and_png_and_then_to_the_larger_file() {
        let dir = scratch("tie");
        put(&dir, "cover.bmp", 90_000);
        put(&dir, "folder.jpg", 5_000);
        // Both score on their names, and cover scores higher regardless of size.
        assert_eq!(found(&dir).as_deref(), Some("cover.bmp"));
        let same = scratch("tie2");
        put(&same, "cover.gif", 90_000);
        put(&same, "Cover.jpg", 5_000);
        assert_eq!(found(&same).as_deref(), Some("Cover.jpg"));
        for d in [dir, same] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn non_pictures_tiny_files_directories_and_missing_folders_give_nothing() {
        let dir = scratch("none");
        put(&dir, "cover.txt", 3000);
        put(&dir, "folder.mp3", 3000);
        put(&dir, "cover.jpg", 10);
        fs::create_dir(dir.join("front.jpg")).unwrap();
        assert_eq!(found(&dir), None);
        assert_eq!(found(&dir.join("does-not-exist")), None);
        let empty = scratch("empty");
        assert_eq!(found(&empty), None);
        for d in [dir, empty] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn a_broken_link_is_skipped() {
        let dir = scratch("link");
        std::os::unix::fs::symlink(dir.join("nowhere.jpg"), dir.join("cover.jpg")).unwrap();
        assert_eq!(found(&dir), None);
        let _ = fs::remove_dir_all(dir);
    }
}
