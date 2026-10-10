use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

// File lengths under `root`. Symlinks are not followed, so a link cannot walk out of the tree.
pub fn folder_bytes(root: &Path, cancel: &AtomicBool) -> io::Result<u64> {
    let meta = fs::symlink_metadata(root)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "symlink"));
    }
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        // An unreadable subfolder counts as empty; only the root itself is an error.
        let read = match fs::read_dir(&dir) {
            Ok(read) => read,
            Err(err) if dir == root => return Err(err),
            Err(_) => continue,
        };
        for entry in read.flatten() {
            let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}

// Under 1 GB the number is still readable in MB. From 1 GB, GB.
pub fn folder_size_text(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB { tenths(bytes, GB, "GB") } else { tenths(bytes, MB, "MB") }
}

fn tenths(bytes: u64, unit: u64, name: &str) -> String {
    let scaled = (bytes as u128 * 10 + unit as u128 / 2) / unit as u128;
    let (whole, frac) = (scaled / 10, scaled % 10);
    if frac == 0 { format!("{whole} {name}") } else { format!("{whole}.{frac} {name}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_files_and_skips_symlink() {
        let dir = std::env::temp_dir().join(format!("rc-size-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/a.txt"), b"abcd").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", dir.join("link")).unwrap();
        let n = folder_bytes(&dir, &AtomicBool::new(false)).unwrap();
        assert_eq!(n, 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_subfolder_does_not_abort_the_count() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rc-size-denied-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("locked")).unwrap();
        fs::write(dir.join("a.txt"), b"abc").unwrap();
        fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        let n = folder_bytes(&dir, &AtomicBool::new(false)).unwrap();
        fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(n, 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_text_uses_mb_until_one_gb() {
        const MB: u64 = 1024 * 1024;
        assert_eq!(folder_size_text(2 * MB), "2 MB");
        assert_eq!(folder_size_text(512 * MB), "512 MB");
        assert_eq!(folder_size_text(1024 * MB), "1 GB");
        assert_eq!(folder_size_text(3 * 1024 * MB), "3 GB");
        assert_eq!(folder_size_text(3 * MB / 2), "1.5 MB");
    }
}
