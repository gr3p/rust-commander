use std::path::Path;
use std::process::Command;

fn main() {
    let version = display_version();
    println!("cargo:rustc-env=RC_VERSION={version}");
    println!("cargo:rustc-env=RC_TITLE=Rust Commander {version}");
}

fn display_version() -> String {
    let base = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into());
    let Some((prefix, _)) = base.rsplit_once('.') else {
        return base;
    };
    if prefix.is_empty() {
        return base;
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    watch_git(&manifest);
    let Some(count) = git(&manifest, &["rev-list", "--count", "HEAD"]) else {
        return base;
    };
    let count = count.trim();
    if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
        return base;
    }
    let dirty = git(&manifest, &["status", "--porcelain"]).is_some_and(|out| !out.trim().is_empty());
    if dirty {
        format!("{prefix}.{count}-dirty")
    } else {
        format!("{prefix}.{count}")
    }
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
    println!("cargo:rerun-if-changed={}", git.join("index").display());
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
