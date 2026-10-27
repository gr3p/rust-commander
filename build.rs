use std::path::Path;
use std::process::Command;

fn main() {
    let version = display_version();
    println!("cargo:rustc-env=RC_VERSION={version}");
    println!("cargo:rustc-env=RC_TITLE=Rust Commander {version}");
}

fn display_version() -> String {
    let base = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into());
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    watch_git(&manifest);
    println!("cargo:rerun-if-env-changed=RC_PKG_VERSION");

    // Packagers can pin the release version without a git checkout.
    if let Ok(v) = std::env::var("RC_PKG_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            return v.into();
        }
    }
    if let Some(v) = git_version(&manifest, &base) {
        return v;
    }
    // GitHub source archives unpack as rust-commander-<tag> with no .git.
    version_from_dirname(&manifest).unwrap_or(base)
}

fn git_version(manifest: &str, base: &str) -> Option<String> {
    let (prefix, _) = base.rsplit_once('.')?;
    if prefix.is_empty() {
        return None;
    }
    // A tarball unpacked inside another repo (an AUR clone, a dotfiles home) must not count that repo.
    let top = git(manifest, &["rev-parse", "--show-toplevel"])?;
    if std::fs::canonicalize(top.trim()).ok()? != std::fs::canonicalize(manifest).ok()? {
        return None;
    }
    let count = git(manifest, &["rev-list", "--count", "HEAD"])?;
    let count = count.trim();
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{prefix}.{count}"))
}

fn version_from_dirname(manifest: &str) -> Option<String> {
    let name = Path::new(manifest).file_name()?.to_str()?;
    let ver = name.strip_prefix("rust-commander-")?;
    if ver.is_empty() || !ver.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    if !ver.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    Some(ver.into())
}

fn watch_git(manifest: &str) {
    let git = Path::new(manifest).join(".git");
    let head = git.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    // HEAD points at refs/heads/<branch>. The commit count lives in that file.
    if let Ok(text) = std::fs::read_to_string(&head) {
        if let Some(path) = text.strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed={}", git.join(path.trim()).display());
        }
    }
    println!("cargo:rerun-if-changed={manifest}/src");
    println!("cargo:rerun-if-changed={manifest}/Cargo.toml");
    println!("cargo:rerun-if-changed={manifest}/build.rs");
}

fn git(manifest: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(manifest).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
